//! Translation of checked programs to a WebAssembly module.
//!
//! Functions are generated on demand: the exported ones first, then every
//! function (or generic instance, by monomorphization) they call.

use std::collections::HashMap;
use std::rc::Rc;

use belsk2_syntax::ast::{BinOp, Block, Expr, ExprKind, FnDecl, Stmt, StmtKind, UnaryOp, VarDecl};
use belsk2_syntax::{Error, FloatKind, IntKind, NumTy, Span, Ty};
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, DataCountSection, DataSection, EntityType, ExportKind,
    ExportSection, Function, FunctionSection, GlobalSection, GlobalType, ImportSection,
    Instruction as I, MemorySection, MemoryType, Module, StartSection, ValType,
};

use crate::helpers::{self, Asm, Helper};
use crate::types::{nullable, Types};
use crate::Unit;

pub type R<T> = Result<T, Error>;

pub fn unsupported(what: impl Into<String>, span: Span) -> Error {
    Error::compile(format!("{} not supported in WASM builds yet", what.into())).at(span)
}

fn no_io(what: &str, span: Span) -> Error {
    let hint = if what.starts_with("idb") || what.starts_with("reab") {
        "the id bank is not supported in WASM builds yet"
    } else {
        "the wasm32 target has no input or output; build for `--target wasi0.2` to use it"
    };
    Error::compile(format!("{what} is not available: {hint}")).at(span)
}

/// The numeric view of a type for arithmetic; `char` counts as `ushort`.
fn arith(ty: &Ty) -> Option<NumTy> {
    match ty {
        Ty::Char => Some(NumTy::Int(IntKind::U16)),
        t => t.num(),
    }
}

fn is_text(ty: &Ty) -> bool {
    matches!(ty, Ty::String | Ty::Ster)
}

/// Whether a function body returns a value anywhere.
fn returns_value(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|s| match &s.kind {
        StmtKind::Return(v) => v.is_some(),
        StmtKind::If {
            then, otherwise, ..
        } => {
            returns_value(&then.stmts)
                || otherwise
                    .as_ref()
                    .is_some_and(|o| returns_value(std::slice::from_ref(o.as_ref())))
        }
        StmtKind::While { body, .. } | StmtKind::For { body, .. } | StmtKind::Block(body) => {
            returns_value(&body.stmts)
        }
        _ => false,
    })
}

/// The return type of a function, `None` if it returns nothing.
fn ret_type(decl: &FnDecl) -> R<Option<Ty>> {
    if decl.ret.is_none() && decl.ret_resolved.is_any() {
        if returns_value(&decl.body.stmts) {
            return Err(Error::compile(format!(
                "'{}' returns a value; declare its return type for WASM builds",
                decl.name
            ))
            .at(decl.span));
        }
        return Ok(None);
    }
    Ok(Some(decl.ret_resolved.clone()))
}

#[derive(Clone)]
enum Item {
    Fn(usize, Rc<FnDecl>),
    Global(u32, Ty),
    /// A function of an imported WIT interface.
    Extern(FnRef),
}

/// A function that can be called: its index and (concrete) signature.
#[derive(Clone)]
pub struct FnRef {
    pub index: u32,
    pub params: Vec<Ty>,
    pub ret: Option<Ty>,
}

enum Job {
    User {
        unit: usize,
        decl: Rc<FnDecl>,
        subst: HashMap<String, Ty>,
        ret: Option<Ty>,
    },
    Init(Vec<(usize, VarDecl, u32)>),
}

struct FuncDef {
    ty: u32,
    body: Option<Asm>,
    export: Option<String>,
}

#[derive(Default)]
pub struct Ctx {
    pub types: Types,
    /// Names visible in each unit: its own declarations, then those of
    /// the units it uses.
    scopes: Vec<HashMap<String, Item>>,
    funcs: Vec<FuncDef>,
    imports: Vec<(String, String, u32)>,
    keys: HashMap<String, FnRef>,
    pending: Vec<(u32, Job)>,
    globals: Vec<(ValType, i32)>,
    data: Vec<Vec<u8>>,
    data_ids: HashMap<String, u32>,
    start: Option<u32>,
    pub errors: Vec<Error>,
    /// Functions the host provides (component imports), callable by name.
    pub externs: HashMap<String, FnRef>,
    /// Whether the module has a linear memory (for the canonical ABI).
    pub memory: bool,
    /// `cabi_realloc`, for the canonical ABI.
    pub realloc: Option<u32>,
    /// The allocator's next free address.
    pub heap: Option<u32>,
    /// Functions of imported WIT interfaces, by interface module name.
    pub wit_externs: HashMap<String, HashMap<String, FnRef>>,
    /// Host functions that cannot be called yet, with the reason.
    pub unavailable: HashMap<String, String>,
    pub extra_exports: Vec<(String, ExportKind, u32)>,
    /// Set for the wasi0.2 target.
    pub wasi: Option<crate::wasi::Wasi>,
}

impl Ctx {
    /// Adds an imported function. All imports must be added before any
    /// function is defined.
    pub fn import(&mut self, module: &str, name: &str, ty: u32) -> u32 {
        self.imports
            .push((module.to_string(), name.to_string(), ty));
        self.imports.len() as u32 - 1
    }

    fn add_def(&mut self, ty: u32) -> u32 {
        self.funcs.push(FuncDef {
            ty,
            body: None,
            export: None,
        });
        (self.imports.len() + self.funcs.len() - 1) as u32
    }

    fn def_mut(&mut self, index: u32) -> Option<&mut FuncDef> {
        let i = (index as usize).checked_sub(self.imports.len())?;
        self.funcs.get_mut(i)
    }

    /// Defines a function whose body is already built.
    pub fn define(&mut self, asm: Asm) -> u32 {
        let ty = self.types.func(asm.params.clone(), asm.results.clone());
        let index = self.add_def(ty);
        if let Some(def) = self.def_mut(index) {
            def.body = Some(asm);
        }
        index
    }

    pub fn export(&mut self, index: u32, name: &str) {
        if let Some(def) = self.def_mut(index) {
            def.export = Some(name.to_string());
        }
    }

    pub fn add_global(&mut self, t: ValType) -> u32 {
        self.add_global_init(t, 0)
    }

    /// A global whose initial value is `init` (`0` or `null` for types
    /// other than `i32`).
    pub fn add_global_init(&mut self, t: ValType, init: i32) -> u32 {
        self.globals.push((t, init));
        self.globals.len() as u32 - 1
    }

    /// The function index of a helper, adding it to the module.
    pub fn helper(&mut self, h: Helper) -> u32 {
        let key = format!("{h:?}");
        if let Some(f) = self.keys.get(&key) {
            return f.index;
        }
        // Reserved before building, so helpers may use each other.
        let index = self.add_def(0);
        self.keys.insert(
            key,
            FnRef {
                index,
                params: Vec::new(),
                ret: None,
            },
        );
        match helpers::build(self, &h) {
            Ok(asm) => {
                let ty = self.types.func(asm.params.clone(), asm.results.clone());
                if let Some(def) = self.def_mut(index) {
                    def.ty = ty;
                    def.body = Some(asm);
                }
            }
            Err(e) => self.errors.push(e),
        }
        index
    }

    /// Instructions that create the string `s` (from a data segment).
    pub fn string_lit(&mut self, s: &str) -> Vec<I<'static>> {
        let seg = match self.data_ids.get(s) {
            Some(&seg) => seg,
            None => {
                let bytes: Vec<u8> = s.chars().flat_map(|c| u32::from(c).to_le_bytes()).collect();
                self.data.push(bytes);
                let seg = self.data.len() as u32 - 1;
                self.data_ids.insert(s.to_string(), seg);
                seg
            }
        };
        let array = self.types.string();
        vec![
            I::I32Const(0),
            I::I32Const(s.chars().count() as i32),
            I::ArrayNewData {
                array_type_index: array,
                array_data_index: seg,
            },
        ]
    }

    /// The function for `decl` with type arguments `inst`, generating it
    /// if needed.
    pub fn user_fn(&mut self, unit: usize, decl: &Rc<FnDecl>, inst: &[Ty]) -> R<FnRef> {
        let names: Vec<String> = inst.iter().map(|t| t.to_string()).collect();
        let key = format!("{unit}:{}<{}>", decl.name, names.join(","));
        if let Some(f) = self.keys.get(&key) {
            return Ok(f.clone());
        }
        let subst: HashMap<String, Ty> = decl
            .type_params
            .iter()
            .cloned()
            .zip(inst.iter().cloned())
            .collect();
        let sub = |t: &Ty| t.substitute(&|n| subst.get(n).cloned());
        let mut params = Vec::new();
        let mut vals = Vec::new();
        for p in &decl.params {
            if p.resolved.is_any() {
                return Err(Error::compile(format!(
                    "parameter '{}' needs a type in WASM builds",
                    p.name
                ))
                .at(p.span));
            }
            let t = sub(&p.resolved);
            let v = self.types.val(&t).ok_or_else(|| {
                unsupported(format!("parameter '{}' of type {t} is", p.name), p.span)
            })?;
            params.push(t);
            vals.push(v);
        }
        let ret = ret_type(decl)?.map(|t| sub(&t));
        let results = match &ret {
            Some(t) => vec![self
                .types
                .val(t)
                .ok_or_else(|| unsupported(format!("returning {t} is"), decl.span))?],
            None => Vec::new(),
        };
        let ty = self.types.func(vals, results);
        let index = self.add_def(ty);
        let f = FnRef {
            index,
            params,
            ret: ret.clone(),
        };
        self.keys.insert(key, f.clone());
        self.pending.push((
            index,
            Job::User {
                unit,
                decl: Rc::clone(decl),
                subst,
                ret,
            },
        ));
        Ok(f)
    }

    /// Looks up a top-level function declared in a unit.
    pub fn unit_fn(&self, unit: usize, name: &str) -> Option<Rc<FnDecl>> {
        match self.scopes.get(unit)?.get(name)? {
            Item::Fn(u, decl) if *u == unit => Some(Rc::clone(decl)),
            _ => None,
        }
    }
}

/// Registers the declarations of every unit: functions, globals and the
/// start function that initializes the globals.
pub fn declare(ctx: &mut Ctx, units: &[Unit]) {
    let mut own: Vec<HashMap<String, Item>> = Vec::new();
    let mut inits = Vec::new();
    for (u, unit) in units.iter().enumerate() {
        let mut scope = HashMap::new();
        if unit.interface {
            for (name, f) in ctx.wit_externs.get(&unit.name).into_iter().flatten() {
                scope.insert(name.clone(), Item::Extern(f.clone()));
            }
        }
        for stmt in &unit.program.stmts {
            let span = stmt.span;
            match &stmt.kind {
                StmtKind::FnDecl(d) => {
                    scope.insert(d.name.clone(), Item::Fn(u, Rc::clone(d)));
                }
                StmtKind::VarDecl(v) => match ctx.types.val(&v.resolved) {
                    Some(t) => {
                        let g = ctx.add_global(t);
                        scope.insert(v.name.clone(), Item::Global(g, v.resolved.clone()));
                        inits.push((u, v.clone(), g));
                    }
                    None => ctx.errors.push(unsupported(
                        format!("a global variable of type {} is", v.resolved),
                        span,
                    )),
                },
                StmtKind::Idb { .. } => ctx.errors.push(no_io("idb", span)),
                StmtKind::Type(_) => ctx.errors.push(unsupported("structs are", span)),
                StmtKind::Using(_) => {}
                _ => ctx.errors.push(
                    Error::compile("only declarations may appear outside functions").at(span),
                ),
            }
        }
        own.push(scope);
    }
    for (u, unit) in units.iter().enumerate() {
        let mut scope = HashMap::new();
        for v in std::iter::once(u).chain(unit.uses.iter().copied()) {
            for (name, item) in own.get(v).into_iter().flatten() {
                scope.entry(name.clone()).or_insert_with(|| item.clone());
            }
        }
        ctx.scopes.push(scope);
    }
    if !inits.is_empty() {
        let ty = ctx.types.func(Vec::new(), Vec::new());
        let index = ctx.add_def(ty);
        ctx.start = Some(index);
        ctx.pending.push((index, Job::Init(inits)));
    }
}

/// Exports every non-generic top-level function of `unit` by name.
pub fn export_unit(ctx: &mut Ctx, units: &[Unit], unit: usize) {
    let Some(u) = units.get(unit) else {
        return;
    };
    for stmt in &u.program.stmts {
        if let StmtKind::FnDecl(d) = &stmt.kind {
            if d.type_params.is_empty() {
                match ctx.user_fn(unit, d, &[]) {
                    Ok(f) => ctx.export(f.index, &d.name),
                    Err(e) => ctx.errors.push(e),
                }
            }
        }
    }
}

/// Generates every pending function and encodes the module.
pub fn finish(mut ctx: Ctx) -> Result<Vec<u8>, Vec<Error>> {
    while let Some((index, job)) = ctx.pending.pop() {
        match generate(&mut ctx, job) {
            Ok(asm) => {
                if let Some(def) = ctx.def_mut(index) {
                    def.body = Some(asm);
                }
            }
            Err(e) => ctx.errors.push(e),
        }
    }
    if !ctx.errors.is_empty() {
        let mut errors = std::mem::take(&mut ctx.errors);
        errors.sort_by_key(|e| e.span);
        errors.dedup();
        return Err(errors);
    }
    Ok(assemble(&ctx))
}

fn generate(ctx: &mut Ctx, job: Job) -> R<Asm> {
    match job {
        Job::User {
            unit,
            decl,
            subst,
            ret,
        } => {
            let sub = |t: &Ty| t.substitute(&|n| subst.get(n).cloned());
            let params: Vec<Ty> = decl.params.iter().map(|p| sub(&p.resolved)).collect();
            let vals = params.iter().filter_map(|t| ctx.types.val(t)).collect();
            let results = ret.iter().filter_map(|t| ctx.types.val(t)).collect();
            let mut g = FnGen::new(ctx, unit, subst.clone(), ret.clone(), vals, results);
            for (i, (p, t)) in decl.params.iter().zip(params).enumerate() {
                g.declare(&p.name, i as u32, t);
            }
            g.block(&decl.body)?;
            if ret.is_some() {
                // The checker made sure every path returns.
                g.emit(I::Unreachable);
            }
            Ok(g.a)
        }
        Job::Init(globals) => {
            let mut g = FnGen::new(ctx, 0, HashMap::new(), None, Vec::new(), Vec::new());
            for (unit, v, index) in globals {
                g.unit = unit;
                g.value(&v.value, &v.resolved)?;
                g.emit(I::GlobalSet(index));
            }
            Ok(g.a)
        }
    }
}

fn assemble(ctx: &Ctx) -> Vec<u8> {
    let mut imports = ImportSection::new();
    for (module, name, ty) in &ctx.imports {
        imports.import(module, name, EntityType::Function(*ty));
    }
    let mut functions = FunctionSection::new();
    for def in &ctx.funcs {
        functions.function(def.ty);
    }
    let mut memories = MemorySection::new();
    if ctx.memory {
        memories.memory(MemoryType {
            minimum: 1,
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        });
    }
    let mut globals = GlobalSection::new();
    for (t, init) in &ctx.globals {
        let zero = match t {
            ValType::I64 => ConstExpr::i64_const(0),
            ValType::F32 => ConstExpr::f32_const(0.0.into()),
            ValType::F64 => ConstExpr::f64_const(0.0.into()),
            ValType::Ref(r) => ConstExpr::ref_null(r.heap_type),
            _ => ConstExpr::i32_const(*init),
        };
        let ty = GlobalType {
            val_type: *t,
            mutable: true,
            shared: false,
        };
        globals.global(ty, &zero);
    }
    let mut exports = ExportSection::new();
    let base = ctx.imports.len() as u32;
    for (i, def) in ctx.funcs.iter().enumerate() {
        if let Some(name) = &def.export {
            exports.export(name, ExportKind::Func, base + i as u32);
        }
    }
    for (name, kind, index) in &ctx.extra_exports {
        exports.export(name, *kind, *index);
    }
    let mut code = CodeSection::new();
    for def in &ctx.funcs {
        let locals: Vec<(u32, ValType)> = def
            .body
            .iter()
            .flat_map(|b| b.locals.iter().map(|t| (1, *t)))
            .collect();
        let mut f = Function::new(locals);
        for ins in def.body.iter().flat_map(|b| b.code.iter()) {
            f.instruction(ins);
        }
        f.instruction(&I::End);
        code.function(&f);
    }
    let mut data = DataSection::new();
    for seg in &ctx.data {
        data.passive(seg.iter().copied());
    }

    let mut module = Module::new();
    module.section(&ctx.types.section());
    if !ctx.imports.is_empty() {
        module.section(&imports);
    }
    module.section(&functions);
    if ctx.memory {
        module.section(&memories);
    }
    if !ctx.globals.is_empty() {
        module.section(&globals);
    }
    module.section(&exports);
    if let Some(start) = ctx.start {
        module.section(&StartSection {
            function_index: start,
        });
    }
    if !ctx.data.is_empty() {
        module.section(&DataCountSection {
            count: ctx.data.len() as u32,
        });
    }
    module.section(&code);
    if !ctx.data.is_empty() {
        module.section(&data);
    }
    module.finish()
}

enum Res {
    Local(u32, Ty),
    Global(u32, Ty),
    Fn(usize, Rc<FnDecl>),
    Extern(FnRef),
}

/// An assignment target.
enum Place {
    Local(u32),
    Global(u32),
    /// `a[i]`: the array and index are kept in locals.
    Elem {
        arr: u32,
        idx: u32,
        elem: Ty,
    },
}

struct FnGen<'c> {
    ctx: &'c mut Ctx,
    unit: usize,
    /// Type arguments of the generic instance being generated.
    subst: HashMap<String, Ty>,
    a: Asm,
    scopes: Vec<HashMap<String, (u32, Ty)>>,
    /// Number of enclosing blocks, loops and ifs.
    depth: u32,
    /// For each enclosing loop: the depth of its `break` and `continue`
    /// targets.
    loops: Vec<(u32, u32)>,
    ret: Option<Ty>,
}

impl<'c> FnGen<'c> {
    fn new(
        ctx: &'c mut Ctx,
        unit: usize,
        subst: HashMap<String, Ty>,
        ret: Option<Ty>,
        params: Vec<ValType>,
        results: Vec<ValType>,
    ) -> Self {
        FnGen {
            ctx,
            unit,
            subst,
            a: Asm::new(params, results),
            scopes: vec![HashMap::new()],
            depth: 0,
            loops: Vec::new(),
            ret,
        }
    }

    fn emit(&mut self, ins: I<'static>) {
        self.a.op(ins);
    }

    /// `ty` with the type parameters of this instance replaced.
    fn t(&self, ty: &Ty) -> Ty {
        if self.subst.is_empty() || !ty.has_params() {
            ty.clone()
        } else {
            ty.substitute(&|n| self.subst.get(n).cloned())
        }
    }

    fn vt(&mut self, ty: &Ty, span: Span) -> R<ValType> {
        let t = self.t(ty);
        self.ctx
            .types
            .val(&t)
            .ok_or_else(|| unsupported(format!("a value of type {t} is"), span))
    }

    fn helper(&mut self, h: Helper) {
        let index = self.ctx.helper(h);
        self.emit(I::Call(index));
    }

    fn declare(&mut self, name: &str, index: u32, ty: Ty) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), (index, ty));
        }
    }

    fn resolve(&self, name: &str) -> Option<Res> {
        for scope in self.scopes.iter().rev() {
            if let Some((i, ty)) = scope.get(name) {
                return Some(Res::Local(*i, ty.clone()));
            }
        }
        match self.ctx.scopes.get(self.unit).and_then(|s| s.get(name)) {
            Some(Item::Global(i, ty)) => Some(Res::Global(*i, ty.clone())),
            Some(Item::Fn(u, decl)) => Some(Res::Fn(*u, Rc::clone(decl))),
            Some(Item::Extern(f)) => Some(Res::Extern(f.clone())),
            None => self.ctx.externs.get(name).cloned().map(Res::Extern),
        }
    }

    // ---- statements ----------------------------------------------------

    fn block(&mut self, block: &Block) -> R<()> {
        self.scopes.push(HashMap::new());
        let result = block.stmts.iter().try_for_each(|s| self.stmt(s));
        self.scopes.pop();
        result
    }

    fn stmt(&mut self, stmt: &Stmt) -> R<()> {
        let span = stmt.span;
        match &stmt.kind {
            StmtKind::Type(_) => return Err(unsupported("structs are", span)),
            StmtKind::Expr(e) => {
                if self.expr(e)?.is_some() {
                    self.emit(I::Drop);
                }
            }
            StmtKind::VarDecl(d) => {
                let ty = self.t(&d.resolved);
                self.value(&d.value, &ty)?;
                let t = self.vt(&ty, span)?;
                let index = self.a.local(t);
                self.emit(I::LocalSet(index));
                self.declare(&d.name, index, ty);
            }
            StmtKind::Assign { target, value } => {
                let (place, ty) = self.place(target)?;
                self.place_begin(&place);
                self.value(value, &ty)?;
                self.place_set(&place);
            }
            StmtKind::CompoundAssign {
                op,
                target,
                value,
                cast,
            } => {
                let cast = self.t(cast);
                self.compound_assign(*op, target, value, &cast, span)?;
            }
            StmtKind::Return(value) => {
                if let Some(e) = value {
                    let Some(ret) = self.ret.clone() else {
                        return Err(Error::compile("this function returns nothing").at(span));
                    };
                    self.value(e, &ret)?;
                }
                self.emit(I::Return);
            }
            StmtKind::Break | StmtKind::Continue => {
                let Some((brk, cont)) = self.loops.last().copied() else {
                    return Err(Error::compile("not inside a loop").at(span));
                };
                let target = if matches!(stmt.kind, StmtKind::Break) {
                    brk
                } else {
                    cont
                };
                self.emit(I::Br(self.depth - target));
            }
            StmtKind::If {
                cond,
                then,
                otherwise,
            } => {
                self.cond(cond)?;
                self.emit(I::If(BlockType::Empty));
                self.depth += 1;
                self.block(then)?;
                if let Some(other) = otherwise {
                    self.emit(I::Else);
                    self.stmt(other)?;
                }
                self.emit(I::End);
                self.depth -= 1;
            }
            StmtKind::While { cond, body } => {
                self.emit(I::Block(BlockType::Empty));
                self.depth += 1;
                let brk = self.depth;
                self.emit(I::Loop(BlockType::Empty));
                self.depth += 1;
                let cont = self.depth;
                self.cond(cond)?;
                self.emit(I::I32Eqz);
                self.emit(I::BrIf(1));
                self.loops.push((brk, cont));
                self.block(body)?;
                self.loops.pop();
                self.emit(I::Br(0));
                self.emit(I::End);
                self.emit(I::End);
                self.depth -= 2;
            }
            StmtKind::For {
                var,
                var_ty,
                iter,
                body,
            } => self.for_loop(var, var_ty, iter, body, span)?,
            StmtKind::Block(b) => self.block(b)?,
            StmtKind::FnDecl(_) => return Err(unsupported("nested functions are", span)),
            StmtKind::Idb { .. } => return Err(no_io("idb", span)),
            StmtKind::Using(_) => return Err(unsupported("'using' here is", span)),
        }
        Ok(())
    }

    /// `for x in s` (characters) and `for x in a` (elements), over the
    /// length at the start, as in the interpreter.
    fn for_loop(&mut self, var: &str, var_ty: &Ty, iter: &Expr, body: &Block, span: Span) -> R<()> {
        let iter_ty = self.t(&iter.ty).canonical();
        let var_ty = self.t(var_ty);
        let (n, i) = (self.a.local(ValType::I32), self.a.local(ValType::I32));
        let get: Vec<I<'static>> = match &iter_ty {
            Ty::String => {
                let s = self.ctx.types.string();
                let data = self.a.local(nullable(s));
                self.expr(iter)?;
                self.emit(I::LocalTee(data));
                self.emit(I::ArrayLen);
                self.emit(I::LocalSet(n));
                vec![I::LocalGet(data), I::LocalGet(i), I::ArrayGet(s)]
            }
            Ty::Array(elem) => {
                let at = self
                    .ctx
                    .types
                    .array(elem)
                    .ok_or_else(|| unsupported(format!("arrays of {elem} are"), span))?;
                let (obj, data) = (
                    self.a.local(nullable(at.obj)),
                    self.a.local(nullable(at.raw)),
                );
                self.expr(iter)?;
                self.emit(I::LocalTee(obj));
                self.emit(I::StructGet {
                    struct_type_index: at.obj,
                    field_index: 1,
                });
                self.emit(I::LocalSet(n));
                self.emit(I::LocalGet(obj));
                self.emit(I::StructGet {
                    struct_type_index: at.obj,
                    field_index: 0,
                });
                self.emit(I::LocalSet(data));
                vec![I::LocalGet(data), I::LocalGet(i), I::ArrayGet(at.raw)]
            }
            other => return Err(unsupported(format!("iterating over {other} is"), iter.span)),
        };
        let x = {
            let t = self.vt(&var_ty, span)?;
            self.a.local(t)
        };
        self.emit(I::Block(BlockType::Empty));
        self.depth += 1;
        let brk = self.depth;
        self.emit(I::Loop(BlockType::Empty));
        self.depth += 1;
        self.a
            .ops([I::LocalGet(i), I::LocalGet(n), I::I32GeU, I::BrIf(1)]);
        self.a.ops(get);
        self.emit(I::LocalSet(x));
        self.emit(I::Block(BlockType::Empty));
        self.depth += 1;
        let cont = self.depth;
        self.scopes.push(HashMap::new());
        self.declare(var, x, var_ty);
        self.loops.push((brk, cont));
        let result = self.block(body);
        self.loops.pop();
        self.scopes.pop();
        result?;
        self.emit(I::End);
        self.depth -= 1;
        self.a.ops([
            I::LocalGet(i),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(i),
            I::Br(0),
            I::End,
            I::End,
        ]);
        self.depth -= 2;
        Ok(())
    }

    fn place(&mut self, target: &Expr) -> R<(Place, Ty)> {
        match &target.kind {
            ExprKind::Ident(name) => match self.resolve(name) {
                Some(Res::Local(i, ty)) => Ok((Place::Local(i), ty)),
                Some(Res::Global(i, ty)) => Ok((Place::Global(i), ty)),
                _ => Err(Error::compile(format!("cannot assign to '{name}'")).at(target.span)),
            },
            ExprKind::Index { object, index } => {
                let Ty::Array(elem) = self.t(&object.ty).canonical() else {
                    return Err(unsupported("assigning to this place is", target.span));
                };
                let obj = self.vt(&object.ty, object.span)?;
                let (arr, idx) = (self.a.local(obj), self.a.local(ValType::I32));
                self.expr(object)?;
                self.emit(I::LocalSet(arr));
                self.index_i32(index)?;
                self.emit(I::LocalSet(idx));
                let elem = (*elem).clone();
                Ok((
                    Place::Elem {
                        arr,
                        idx,
                        elem: elem.clone(),
                    },
                    elem,
                ))
            }
            _ => Err(unsupported("assigning to this place is", target.span)),
        }
    }

    fn place_begin(&mut self, place: &Place) {
        if let Place::Elem { arr, idx, .. } = place {
            self.a.ops([I::LocalGet(*arr), I::LocalGet(*idx)]);
        }
    }

    fn place_get(&mut self, place: &Place) {
        match place {
            Place::Local(i) => self.emit(I::LocalGet(*i)),
            Place::Global(i) => self.emit(I::GlobalGet(*i)),
            Place::Elem { arr, idx, elem } => {
                self.a.ops([I::LocalGet(*arr), I::LocalGet(*idx)]);
                self.helper(Helper::ArrGet(elem.clone()));
            }
        }
    }

    fn place_set(&mut self, place: &Place) {
        match place {
            Place::Local(i) => self.emit(I::LocalSet(*i)),
            Place::Global(i) => self.emit(I::GlobalSet(*i)),
            Place::Elem { elem, .. } => self.helper(Helper::ArrSet(elem.clone())),
        }
    }

    /// `x op= value`: computed as `x op value` and converted back to the
    /// type of `x` (`cast`).
    fn compound_assign(
        &mut self,
        op: BinOp,
        target: &Expr,
        value: &Expr,
        cast: &Ty,
        span: Span,
    ) -> R<()> {
        let (place, ty) = self.place(target)?;
        self.place_begin(&place);
        if op == BinOp::Add && is_text(&ty) {
            self.place_get(&place);
            self.helper(Helper::StrOrNull);
            self.emit_str(value)?;
            self.helper(Helper::Concat);
            self.place_set(&place);
            return Ok(());
        }
        let value_ty = self.t(&value.ty);
        let shift = matches!(op, BinOp::Shl | BinOp::Shr);
        let op_ty = if matches!((&ty, &value_ty), (Ty::Bool, Ty::Bool)) {
            Ty::Bool
        } else {
            match (arith(&ty), arith(&value_ty)) {
                (Some(NumTy::Int(k)), _) if shift => Ty::Int(NumTy::int_promoted(k)),
                (Some(a), Some(b)) => match NumTy::promote(a, b) {
                    Some(n) => n.ty(),
                    None => return Err(unsupported(format!("'{}=' here is", op.symbol()), span)),
                },
                _ => return Err(unsupported(format!("'{}=' on {ty} is", op.symbol()), span)),
            }
        };
        self.place_get(&place);
        self.convert(&ty, &op_ty, span)?;
        self.expr(value)?;
        let rhs_ty = if shift { Ty::INT } else { op_ty.clone() };
        self.convert(&value_ty, &rhs_ty, span)?;
        self.binop(op, &op_ty, span)?;
        self.convert(&op_ty, cast, span)?;
        if matches!(cast, Ty::Bel) {
            self.helper(Helper::Bel);
        }
        self.place_set(&place);
        Ok(())
    }

    // ---- expressions ---------------------------------------------------

    /// Emits `e` as a value stored in a place of type `target`.
    fn value(&mut self, e: &Expr, target: &Ty) -> R<()> {
        let target = self.t(target);
        match &e.kind {
            ExprKind::Null => {
                let ValType::Ref(r) = self.vt(&target, e.span)? else {
                    return Err(Error::compile(format!("{target} cannot be null")).at(e.span));
                };
                self.emit(I::RefNull(r.heap_type));
            }
            ExprKind::Array(items) if items.is_empty() => {
                let Ty::Array(elem) = target.canonical() else {
                    return Err(unsupported(
                        format!("converting an array to {target} is"),
                        e.span,
                    ));
                };
                self.array_lit(items, &elem, e.span)?;
            }
            _ => {
                let got = self.expr(e)?;
                let want = self.vt(&target, e.span)?;
                match got {
                    Some(t) if t == want => {}
                    Some(_) => {
                        return Err(unsupported(
                            format!("converting {} to {target} is", self.t(&e.ty)),
                            e.span,
                        ))
                    }
                    None => return Err(Error::compile("this expression has no value").at(e.span)),
                }
            }
        }
        if matches!(target, Ty::Bel) {
            self.helper(Helper::Bel);
        }
        Ok(())
    }

    /// Emits a `bool` condition.
    fn cond(&mut self, e: &Expr) -> R<()> {
        if !matches!(self.t(&e.ty), Ty::Bool) {
            return Err(unsupported(
                format!("a condition of type {} is", e.ty),
                e.span,
            ));
        }
        self.expr(e)?;
        Ok(())
    }

    /// Emits `e`; returns the type of the value left on the stack, `None`
    /// for calls of functions that return nothing.
    fn expr(&mut self, e: &Expr) -> R<Option<ValType>> {
        let span = e.span;
        match &e.kind {
            ExprKind::Int(v, k) => {
                let v = k.wrap(*v);
                if k.bits() == 64 {
                    self.emit(I::I64Const(v as i64));
                } else {
                    self.emit(I::I32Const(v as i32));
                }
            }
            ExprKind::Float(v, FloatKind::F32) => self.emit(I::F32Const((*v as f32).into())),
            ExprKind::Float(v, FloatKind::F64) => self.emit(I::F64Const((*v).into())),
            ExprKind::Bool(b) => self.emit(I::I32Const(i32::from(*b))),
            ExprKind::String(s) => {
                let ops = self.ctx.string_lit(s);
                self.a.ops(ops);
            }
            ExprKind::Ident(name) => match self.resolve(name) {
                Some(Res::Local(i, _)) => self.emit(I::LocalGet(i)),
                Some(Res::Global(i, _)) => self.emit(I::GlobalGet(i)),
                Some(Res::Fn(..) | Res::Extern(_)) => {
                    return Err(unsupported("functions as values are", span))
                }
                None => return Err(Error::compile(format!("undefined variable '{name}'")).at(span)),
            },
            ExprKind::Array(items) => {
                let Ty::Array(elem) = self.t(&e.ty).canonical() else {
                    return Err(unsupported("arrays of mixed types are", span));
                };
                self.array_lit(items, &elem, span)?;
            }
            ExprKind::Unary { op, expr } => {
                let ty = self.t(&e.ty);
                self.unary(*op, expr, &ty, span)?;
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let ty = self.t(&e.ty);
                self.binary(*op, lhs, rhs, &ty, span)?;
            }
            ExprKind::Call {
                callee, args, inst, ..
            } => return self.call(callee, args, inst, span),
            ExprKind::Convert { expr, to } => {
                let to = self.t(to);
                self.convert_expr(expr, &to, span)?;
            }
            ExprKind::Index { object, index } => self.index_get(object, index, span)?,
            ExprKind::Null => return Err(unsupported("null without a known type is", span)),
            ExprKind::Map(_) => return Err(unsupported("dictionaries are", span)),
            ExprKind::Member { .. } => return Err(unsupported("members are", span)),
            ExprKind::This => return Err(unsupported("structs are", span)),
        }
        self.vt(&e.ty, span).map(Some)
    }

    fn array_lit(&mut self, items: &[Expr], elem: &Ty, span: Span) -> R<()> {
        let at = self
            .ctx
            .types
            .array(elem)
            .ok_or_else(|| unsupported(format!("arrays of {elem} are"), span))?;
        for item in items {
            self.value(item, elem)?;
        }
        self.emit(I::ArrayNewFixed {
            array_type_index: at.raw,
            array_size: items.len() as u32,
        });
        self.emit(I::I32Const(items.len() as i32));
        self.emit(I::StructNew(at.obj));
        Ok(())
    }

    /// Emits an index as an `i32`, trapping if a `long` index is out of
    /// range.
    fn index_i32(&mut self, index: &Expr) -> R<()> {
        self.expr(index)?;
        if let Ty::Int(k) = self.t(&index.ty) {
            if k.bits() == 64 {
                let tmp = self.a.local(ValType::I64);
                self.a.ops([
                    I::LocalTee(tmp),
                    I::I64Const(i64::from(i32::MAX)),
                    I::I64GtU,
                    I::If(BlockType::Empty),
                    I::Unreachable,
                    I::End,
                    I::LocalGet(tmp),
                    I::I32WrapI64,
                ]);
            }
        }
        Ok(())
    }

    fn index_get(&mut self, object: &Expr, index: &Expr, span: Span) -> R<()> {
        match self.t(&object.ty).canonical() {
            Ty::String => {
                self.expr(object)?;
                self.index_i32(index)?;
                let s = self.ctx.types.string();
                self.emit(I::ArrayGet(s));
            }
            Ty::Array(elem) => {
                self.expr(object)?;
                self.index_i32(index)?;
                self.helper(Helper::ArrGet((*elem).clone()));
            }
            other => return Err(unsupported(format!("indexing {other} is"), span)),
        }
        Ok(())
    }

    fn call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        inst: &[Ty],
        span: Span,
    ) -> R<Option<ValType>> {
        let ExprKind::Ident(name) = &callee.kind else {
            return Err(unsupported("calling this expression is", span));
        };
        if belsk2_typeck::is_builtin(name) {
            return self.builtin(name, args, span);
        }
        let f = match self.resolve(name) {
            Some(Res::Fn(unit, decl)) => {
                let inst: Vec<Ty> = inst.iter().map(|t| self.t(t)).collect();
                if inst.iter().any(|t| t.is_any() || t.has_params()) {
                    return Err(unsupported(
                        "calls whose type arguments cannot be inferred are",
                        span,
                    ));
                }
                self.ctx.user_fn(unit, &decl, &inst)?
            }
            Some(Res::Extern(f)) => f,
            Some(_) => return Err(unsupported("calling a variable is", callee.span)),
            None => {
                let msg = match self.ctx.unavailable.get(name.as_str()) {
                    Some(why) => format!("'{name}' cannot be used in Belsk2 yet: {why}"),
                    None => format!("undefined function '{name}'"),
                };
                return Err(Error::compile(msg).at(span));
            }
        };
        for (arg, param) in args.iter().zip(&f.params) {
            self.value(arg, param)?;
        }
        self.emit(I::Call(f.index));
        Ok(match &f.ret {
            Some(t) => self.ctx.types.val(t),
            None => None,
        })
    }

    fn builtin(&mut self, name: &str, args: &[Expr], span: Span) -> R<Option<ValType>> {
        let sref = nullable(self.ctx.types.string());
        let arg = |i: usize| {
            args.get(i)
                .ok_or_else(|| Error::compile(format!("{name}() needs more arguments")).at(span))
        };
        match name {
            "prinb" | "input" if self.ctx.wasi.is_some() => {
                // `input(prompt)` writes the prompt without a newline.
                match args.first() {
                    Some(a) => self.emit_str(a)?,
                    None => {
                        let ops = self.ctx.string_lit("");
                        self.a.ops(ops);
                    }
                }
                if name == "prinb" {
                    let newline = self.ctx.string_lit("\n");
                    self.a.ops(newline);
                    self.helper(Helper::Concat);
                }
                self.helper(Helper::WasiWrite);
                if name == "prinb" {
                    return Ok(None);
                }
                self.helper(Helper::WasiReadLine);
                return Ok(Some(sref));
            }
            "prinb" | "input" | "reab" => return Err(no_io(&format!("{name}()"), span)),
            "len" => {
                let a = arg(0)?;
                let ty = self.t(&a.ty).canonical();
                self.expr(a)?;
                match ty {
                    Ty::String => self.emit(I::ArrayLen),
                    Ty::Array(elem) => {
                        let at =
                            self.ctx.types.array(&elem).ok_or_else(|| {
                                unsupported(format!("arrays of {elem} are"), span)
                            })?;
                        self.emit(I::StructGet {
                            struct_type_index: at.obj,
                            field_index: 1,
                        });
                    }
                    other => return Err(unsupported(format!("len() of {other} is"), span)),
                }
                return Ok(Some(ValType::I32));
            }
            "str" => {
                self.emit_str(arg(0)?)?;
                return Ok(Some(sref));
            }
            "substr" => {
                let s = arg(0)?;
                if !is_text(&self.t(&s.ty)) {
                    return Err(unsupported(format!("substr() of {} is", s.ty), span));
                }
                self.expr(s)?;
                for i in [1, 2] {
                    let a = arg(i)?;
                    self.expr(a)?;
                    let ty = self.t(&a.ty);
                    let n = arith(&ty)
                        .ok_or_else(|| unsupported(format!("substr() with {ty} is"), a.span))?;
                    self.emit_to_i64(n);
                }
                self.helper(Helper::Substr);
                return Ok(Some(sref));
            }
            "push" | "pop" => {
                let a = arg(0)?;
                let ty = self.t(&a.ty).canonical();
                let Ty::Array(elem) = ty else {
                    return Err(unsupported(format!("{name}() on {ty} is"), span));
                };
                let elem = (*elem).clone();
                let obj = self.vt(&a.ty, span)?;
                self.expr(a)?;
                if name == "push" {
                    self.value(arg(1)?, &elem)?;
                    self.helper(Helper::ArrPush(elem));
                    return Ok(Some(obj));
                }
                let t = self.vt(&elem, span)?;
                self.helper(Helper::ArrPop(elem));
                return Ok(Some(t));
            }
            _ => {}
        }
        let target = match name {
            "num" => Some(Ty::DOUBLE),
            _ => Ty::from_name(name).filter(|t| t.num().is_some() || *t == Ty::Char),
        };
        let Some(target) = target else {
            return Err(unsupported(format!("{name}() is"), span));
        };
        self.convert_expr(arg(0)?, &target, span)?;
        self.vt(&target, span).map(Some)
    }

    /// Emits `e` as a non-null string, as `str(e)` and `+` format it.
    fn emit_str(&mut self, e: &Expr) -> R<()> {
        let ty = self.t(&e.ty).canonical();
        let s = self.ctx.types.string();
        match ty {
            Ty::String => {
                self.expr(e)?;
                self.helper(Helper::StrOrNull);
            }
            Ty::Null => {
                let ops = self.ctx.string_lit("null");
                self.a.ops(ops);
            }
            Ty::Char => {
                self.expr(e)?;
                self.emit(I::ArrayNewFixed {
                    array_type_index: s,
                    array_size: 1,
                });
            }
            Ty::Bool => {
                self.expr(e)?;
                self.emit(I::If(BlockType::Result(nullable(s))));
                let yes = self.ctx.string_lit("true");
                self.a.ops(yes);
                self.emit(I::Else);
                let no = self.ctx.string_lit("false");
                self.a.ops(no);
                self.emit(I::End);
            }
            Ty::Int(k) => {
                self.expr(e)?;
                self.emit_to_i64(NumTy::Int(k));
                self.helper(Helper::IntToStr {
                    unsigned: k == IntKind::U64,
                });
            }
            other => {
                return Err(unsupported(
                    format!("converting {other} to a string is"),
                    e.span,
                ))
            }
        }
        Ok(())
    }

    /// Emits `e` converted to `to`.
    fn convert_expr(&mut self, e: &Expr, to: &Ty, span: Span) -> R<()> {
        // A one-character string constant used as a `char`.
        if let (ExprKind::String(s), Ty::Char) = (&e.kind, to) {
            let mut chars = s.chars();
            if let (Some(c), None) = (chars.next(), chars.next()) {
                self.emit(I::I32Const(u32::from(c) as i32));
                return Ok(());
            }
        }
        let from = self.t(&e.ty);
        if is_text(&to.canonical()) && !is_text(&from) {
            return self.emit_str(e);
        }
        if self.expr(e)?.is_none() {
            return Err(Error::compile("this expression has no value").at(e.span));
        }
        self.convert(&from, to, span)
    }

    /// Converts the value on the stack from `from` to `to`, with the
    /// semantics of the interpreter's explicit conversions.
    fn convert(&mut self, from: &Ty, to: &Ty, span: Span) -> R<()> {
        let (f, t) = (from.canonical(), to.canonical());
        if f == t {
            return Ok(());
        }
        let source = match f {
            Ty::Bool => Some(NumTy::Int(IntKind::I32)),
            ref f => arith(f),
        };
        let fail = || unsupported(format!("converting {from} to {to} is"), span);
        let source = source.ok_or_else(fail)?;
        if t == Ty::Char {
            self.emit_to_i64(source);
            self.helper(Helper::Char);
            return Ok(());
        }
        let target = t.num().ok_or_else(fail)?;
        self.num_convert(source, target);
        Ok(())
    }

    fn emit_to_i64(&mut self, n: NumTy) {
        match n {
            NumTy::Int(k) if k.bits() == 64 => {}
            NumTy::Int(k) if k.signed() => self.emit(I::I64ExtendI32S),
            NumTy::Int(_) => self.emit(I::I64ExtendI32U),
            NumTy::Float(k) => {
                if k == FloatKind::F32 {
                    self.emit(I::F64PromoteF32);
                }
                self.emit(I::I64TruncSatF64S);
            }
        }
    }

    /// Reduces an `i32` to the range of a narrow integer type.
    fn narrow(&mut self, k: IntKind) {
        match k {
            IntKind::I8 => self.emit(I::I32Extend8S),
            IntKind::I16 => self.emit(I::I32Extend16S),
            IntKind::U8 | IntKind::U16 => {
                let mask = if k == IntKind::U8 { 0xFF } else { 0xFFFF };
                self.emit(I::I32Const(mask));
                self.emit(I::I32And);
            }
            _ => {}
        }
    }

    fn num_convert(&mut self, from: NumTy, to: NumTy) {
        use NumTy::{Float, Int};
        match (from, to) {
            (Int(a), Int(b)) => match (a.bits() == 64, b.bits() == 64) {
                (false, true) => {
                    self.emit(if a.signed() {
                        I::I64ExtendI32S
                    } else {
                        I::I64ExtendI32U
                    });
                }
                (true, false) => {
                    self.emit(I::I32WrapI64);
                    self.narrow(b);
                }
                _ => self.narrow(b),
            },
            (Int(a), Float(g)) => {
                self.emit(match (a.bits() == 64, a.signed()) {
                    (true, true) => I::F64ConvertI64S,
                    (true, false) => I::F64ConvertI64U,
                    (false, true) => I::F64ConvertI32S,
                    (false, false) => I::F64ConvertI32U,
                });
                if g == FloatKind::F32 {
                    self.emit(I::F32DemoteF64);
                }
            }
            (Float(f), Float(g)) => match (f, g) {
                (FloatKind::F32, FloatKind::F64) => self.emit(I::F64PromoteF32),
                (FloatKind::F64, FloatKind::F32) => self.emit(I::F32DemoteF64),
                _ => {}
            },
            (Float(_), Int(b)) => {
                self.emit_to_i64(from);
                if b.bits() != 64 {
                    self.emit(I::I32WrapI64);
                    self.narrow(b);
                }
            }
        }
    }

    fn unary(&mut self, op: UnaryOp, inner: &Expr, ty: &Ty, span: Span) -> R<()> {
        let inner_ty = self.t(&inner.ty);
        match op {
            UnaryOp::Not => {
                self.cond(inner)?;
                self.emit(I::I32Eqz);
            }
            UnaryOp::Neg => match arith(ty) {
                Some(NumTy::Int(k)) => {
                    self.emit(if k.bits() == 64 {
                        I::I64Const(0)
                    } else {
                        I::I32Const(0)
                    });
                    self.expr(inner)?;
                    self.convert(&inner_ty, ty, span)?;
                    self.emit(if k.bits() == 64 { I::I64Sub } else { I::I32Sub });
                }
                Some(NumTy::Float(k)) => {
                    self.expr(inner)?;
                    self.convert(&inner_ty, ty, span)?;
                    self.emit(if k == FloatKind::F32 {
                        I::F32Neg
                    } else {
                        I::F64Neg
                    });
                }
                None => return Err(unsupported(format!("negating {inner_ty} is"), span)),
            },
            UnaryOp::BitNot => {
                let Some(NumTy::Int(k)) = arith(ty) else {
                    return Err(unsupported(format!("'~' on {inner_ty} is"), span));
                };
                self.expr(inner)?;
                self.convert(&inner_ty, ty, span)?;
                if k.bits() == 64 {
                    self.emit(I::I64Const(-1));
                    self.emit(I::I64Xor);
                } else {
                    self.emit(I::I32Const(-1));
                    self.emit(I::I32Xor);
                }
            }
        }
        Ok(())
    }

    fn binary(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr, ty: &Ty, span: Span) -> R<()> {
        use BinOp::*;
        match op {
            And | Or => {
                self.cond(lhs)?;
                self.emit(I::If(BlockType::Result(ValType::I32)));
                self.depth += 1;
                if op == And {
                    self.cond(rhs)?;
                    self.emit(I::Else);
                    self.emit(I::I32Const(0));
                } else {
                    self.emit(I::I32Const(1));
                    self.emit(I::Else);
                    self.cond(rhs)?;
                }
                self.emit(I::End);
                self.depth -= 1;
                return Ok(());
            }
            Add if is_text(ty) => {
                self.emit_str(lhs)?;
                self.emit_str(rhs)?;
                self.helper(Helper::Concat);
                return Ok(());
            }
            Eq | Ne => return self.equality(op, lhs, rhs, span),
            _ => {}
        }
        let lt = self.t(&lhs.ty).canonical();
        if matches!(op, Lt | Gt | Le | Ge) {
            self.expr(lhs)?;
            self.expr(rhs)?;
            if lt == Ty::String {
                self.helper(Helper::StrCmp);
                self.emit(I::I32Const(0));
                self.emit(match op {
                    Lt => I::I32LtS,
                    Gt => I::I32GtS,
                    Le => I::I32LeS,
                    _ => I::I32GeS,
                });
                return Ok(());
            }
            return self.compare(op, &lt, span);
        }
        self.expr(lhs)?;
        self.expr(rhs)?;
        self.binop(op, ty, span).map(|_| ())
    }

    /// `==` and `!=`: by value for numbers, strings and arrays.
    fn equality(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr, span: Span) -> R<()> {
        let (lt, rt) = (self.t(&lhs.ty).canonical(), self.t(&rhs.ty).canonical());
        let ne = op == BinOp::Ne;
        match (&lt, &rt) {
            (Ty::Null, Ty::Null) => self.emit(I::I32Const(i32::from(!ne))),
            (Ty::Null, _) | (_, Ty::Null) => {
                let other = if lt == Ty::Null { rhs } else { lhs };
                match self.expr(other)? {
                    Some(ValType::Ref(_)) => {
                        self.emit(I::RefIsNull);
                        if ne {
                            self.emit(I::I32Eqz);
                        }
                    }
                    Some(_) => {
                        // Numbers, bools and chars are never null.
                        self.emit(I::Drop);
                        self.emit(I::I32Const(i32::from(ne)));
                    }
                    None => return Err(Error::compile("this expression has no value").at(span)),
                }
            }
            (Ty::String, Ty::String) => {
                self.expr(lhs)?;
                self.expr(rhs)?;
                self.helper(Helper::StrEq);
                if ne {
                    self.emit(I::I32Eqz);
                }
            }
            (Ty::Array(a), Ty::Array(b)) if a == b => {
                self.expr(lhs)?;
                self.expr(rhs)?;
                self.helper(Helper::ArrEq((**a).clone()));
                if ne {
                    self.emit(I::I32Eqz);
                }
            }
            _ => {
                if lt != rt && !(arith(&lt).is_some() && arith(&rt).is_some()) {
                    return Err(unsupported(format!("comparing {lt} with {rt} is"), span));
                }
                self.expr(lhs)?;
                self.expr(rhs)?;
                self.compare(op, &lt, span)?;
            }
        }
        Ok(())
    }

    /// Compares two values of type `ty` on the stack.
    fn compare(&mut self, op: BinOp, ty: &Ty, span: Span) -> R<()> {
        use BinOp::*;
        let ins = match ty.canonical() {
            Ty::Bool => match op {
                Eq => I::I32Eq,
                Ne => I::I32Ne,
                _ => return Err(unsupported("ordering bools is", span)),
            },
            Ty::Char | Ty::Int(_) => {
                let (wide, signed) = match ty {
                    Ty::Int(k) => (k.bits() == 64, k.signed()),
                    _ => (false, false),
                };
                match (op, wide, signed) {
                    (Eq, false, _) => I::I32Eq,
                    (Ne, false, _) => I::I32Ne,
                    (Lt, false, true) => I::I32LtS,
                    (Lt, false, false) => I::I32LtU,
                    (Gt, false, true) => I::I32GtS,
                    (Gt, false, false) => I::I32GtU,
                    (Le, false, true) => I::I32LeS,
                    (Le, false, false) => I::I32LeU,
                    (Ge, false, true) => I::I32GeS,
                    (Eq, true, _) => I::I64Eq,
                    (Ne, true, _) => I::I64Ne,
                    (Lt, true, true) => I::I64LtS,
                    (Lt, true, false) => I::I64LtU,
                    (Gt, true, true) => I::I64GtS,
                    (Gt, true, false) => I::I64GtU,
                    (Le, true, true) => I::I64LeS,
                    (Le, true, false) => I::I64LeU,
                    (Ge, true, true) => I::I64GeS,
                    (_, true, _) => I::I64GeU,
                    (_, false, _) => I::I32GeU,
                }
            }
            Ty::Float(k) => match (op, k == FloatKind::F32) {
                (Eq, true) => I::F32Eq,
                (Ne, true) => I::F32Ne,
                (Lt, true) => I::F32Lt,
                (Gt, true) => I::F32Gt,
                (Le, true) => I::F32Le,
                (_, true) => I::F32Ge,
                (Eq, false) => I::F64Eq,
                (Ne, false) => I::F64Ne,
                (Lt, false) => I::F64Lt,
                (Gt, false) => I::F64Gt,
                (Le, false) => I::F64Le,
                (_, false) => I::F64Ge,
            },
            other => return Err(unsupported(format!("comparing {other} values is"), span)),
        };
        self.emit(ins);
        Ok(())
    }

    /// Applies an arithmetic, bitwise or shift operator to two values of
    /// type `ty` on the stack (for shifts, the count is an `int`).
    fn binop(&mut self, op: BinOp, ty: &Ty, span: Span) -> R<ValType> {
        use BinOp::*;
        let fail = || unsupported(format!("'{}' on {ty} is", op.symbol()), span);
        if *ty == Ty::Bool {
            self.emit(match op {
                BitAnd => I::I32And,
                BitOr => I::I32Or,
                BitXor => I::I32Xor,
                _ => return Err(fail()),
            });
            return Ok(ValType::I32);
        }
        match arith(ty).ok_or_else(fail)? {
            NumTy::Int(k) => {
                let wide = k.bits() == 64;
                let s = k.signed();
                if wide && matches!(op, Shl | Shr) {
                    self.emit(I::I64ExtendI32U);
                }
                let ins = match (op, wide) {
                    (Add, false) => I::I32Add,
                    (Sub, false) => I::I32Sub,
                    (Mul, false) => I::I32Mul,
                    (Div, false) if s => I::Call(self.ctx.helper(Helper::DivI32)),
                    (Div, false) => I::I32DivU,
                    (Rem, false) if s => I::I32RemS,
                    (Rem, false) => I::I32RemU,
                    (BitAnd, false) => I::I32And,
                    (BitOr, false) => I::I32Or,
                    (BitXor, false) => I::I32Xor,
                    (Shl, false) => I::I32Shl,
                    (Shr, false) if s => I::I32ShrS,
                    (Shr, false) => I::I32ShrU,
                    (Add, true) => I::I64Add,
                    (Sub, true) => I::I64Sub,
                    (Mul, true) => I::I64Mul,
                    (Div, true) if s => I::Call(self.ctx.helper(Helper::DivI64)),
                    (Div, true) => I::I64DivU,
                    (Rem, true) if s => I::I64RemS,
                    (Rem, true) => I::I64RemU,
                    (BitAnd, true) => I::I64And,
                    (BitOr, true) => I::I64Or,
                    (BitXor, true) => I::I64Xor,
                    (Shl, true) => I::I64Shl,
                    (Shr, true) if s => I::I64ShrS,
                    (Shr, true) => I::I64ShrU,
                    _ => return Err(fail()),
                };
                self.emit(ins);
                Ok(if wide { ValType::I64 } else { ValType::I32 })
            }
            NumTy::Float(FloatKind::F64) => {
                let ins = match op {
                    Add => I::F64Add,
                    Sub => I::F64Sub,
                    Mul => I::F64Mul,
                    Div => I::F64Div,
                    Rem => I::Call(self.ctx.helper(Helper::Fmod)),
                    _ => return Err(fail()),
                };
                self.emit(ins);
                Ok(ValType::F64)
            }
            NumTy::Float(FloatKind::F32) => {
                if op == Rem {
                    // Computed in double precision, as the interpreter does.
                    let b = self.a.local(ValType::F32);
                    self.emit(I::LocalSet(b));
                    self.emit(I::F64PromoteF32);
                    self.emit(I::LocalGet(b));
                    self.emit(I::F64PromoteF32);
                    self.helper(Helper::Fmod);
                    self.emit(I::F32DemoteF64);
                    return Ok(ValType::F32);
                }
                self.emit(match op {
                    Add => I::F32Add,
                    Sub => I::F32Sub,
                    Mul => I::F32Mul,
                    Div => I::F32Div,
                    _ => return Err(fail()),
                });
                Ok(ValType::F32)
            }
        }
    }
}
