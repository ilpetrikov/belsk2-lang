//! Translation of a checked program to a WebAssembly module.
//!
//! Values are represented as follows: `bool`, `char` and integers up to 32
//! bits are `i32` (kept within the range of their type), `long` and
//! `ulong` are `i64`, `float` is `f32`, `double` and `bel` are `f64`.

use std::collections::HashMap;

use belsk2_syntax::ast::{
    BinOp, Block, Expr, ExprKind, FnDecl, Program, Stmt, StmtKind, UnaryOp, VarDecl,
};
use belsk2_syntax::{Error, FloatKind, IntKind, NumTy, Span, Ty};
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, ExportKind, ExportSection, Function, FunctionSection,
    GlobalSection, GlobalType, Instruction as I, Module, StartSection, TypeSection, ValType,
};

use crate::helpers::Helper;

type R<T> = Result<T, Error>;

fn unsupported(what: impl Into<String>, span: Span) -> Error {
    Error::compile(format!("{} not supported in WASM builds yet", what.into())).at(span)
}

fn no_io(what: &str, span: Span) -> Error {
    Error::compile(format!(
        "{what} is not available in WASM builds: they have no input or output"
    ))
    .at(span)
}

/// The WebAssembly type of values of `ty`, if it has one yet.
fn val_type(ty: &Ty) -> Option<ValType> {
    Some(match ty {
        Ty::Bool | Ty::Char => ValType::I32,
        Ty::Int(k) if k.bits() == 64 => ValType::I64,
        Ty::Int(_) => ValType::I32,
        Ty::Float(FloatKind::F32) => ValType::F32,
        Ty::Float(FloatKind::F64) | Ty::Bel => ValType::F64,
        _ => return None,
    })
}

fn val_type_at(ty: &Ty, span: Span, what: &str) -> R<ValType> {
    val_type(ty).ok_or_else(|| unsupported(format!("{what} of type {ty} is"), span))
}

/// The numeric view of a type for arithmetic; `char` counts as `ushort`.
fn arith(ty: &Ty) -> Option<NumTy> {
    match ty {
        Ty::Char => Some(NumTy::Int(IntKind::U16)),
        t => t.num(),
    }
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

struct FnInfo {
    index: u32,
    params: Vec<Ty>,
    /// `None` for functions that return nothing.
    ret: Option<Ty>,
    generic: bool,
}

#[derive(Default)]
struct Ctx {
    fns: HashMap<String, FnInfo>,
    globals: HashMap<String, (u32, Ty)>,
    helpers: Vec<Helper>,
    user_fns: u32,
}

impl Ctx {
    /// The function index of a helper, adding it to the module.
    fn helper(&mut self, h: Helper) -> u32 {
        let pos = match self.helpers.iter().position(|x| *x == h) {
            Some(pos) => pos,
            None => {
                self.helpers.push(h);
                self.helpers.len() - 1
            }
        };
        self.user_fns + pos as u32
    }
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
    val_type_at(&decl.ret_resolved, decl.span, "a return value")?;
    Ok(Some(decl.ret_resolved.clone()))
}

struct Body {
    locals: Vec<ValType>,
    code: Vec<I<'static>>,
}

pub fn compile(program: &Program) -> Result<Vec<u8>, Vec<Error>> {
    let mut errors = Vec::new();
    let mut ctx = Ctx::default();
    let mut decls: Vec<&FnDecl> = Vec::new();
    let mut globals: Vec<&VarDecl> = Vec::new();
    for stmt in &program.stmts {
        let span = stmt.span;
        match &stmt.kind {
            StmtKind::FnDecl(d) => {
                let ret = ret_type(d).unwrap_or_else(|e| {
                    errors.push(e);
                    None
                });
                let info = FnInfo {
                    index: decls.len() as u32,
                    params: d.params.iter().map(|p| p.resolved.clone()).collect(),
                    ret,
                    generic: !d.type_params.is_empty(),
                };
                ctx.fns.insert(d.name.clone(), info);
                decls.push(d);
            }
            StmtKind::VarDecl(v) => match val_type(&v.resolved) {
                Some(_) => {
                    let index = globals.len() as u32;
                    ctx.globals
                        .insert(v.name.clone(), (index, v.resolved.clone()));
                    globals.push(v);
                }
                None => errors.push(unsupported(
                    format!("a global variable of type {} is", v.resolved),
                    span,
                )),
            },
            StmtKind::Idb { .. } => errors.push(no_io("idb", span)),
            StmtKind::Using(_) => errors.push(unsupported("'using' is", span)),
            _ => errors
                .push(Error::compile("only declarations may appear outside functions").at(span)),
        }
    }
    ctx.user_fns = decls.len() as u32;

    let mut bodies = Vec::new();
    for decl in &decls {
        if !decl.type_params.is_empty() {
            errors.push(unsupported("generic functions are", decl.span));
            continue;
        }
        match function(&mut ctx, decl) {
            Ok(body) => bodies.push(body),
            Err(e) => errors.push(e),
        }
    }
    let init = if globals.is_empty() {
        None
    } else {
        match init_function(&mut ctx, &globals) {
            Ok(body) => Some(body),
            Err(e) => {
                errors.push(e);
                None
            }
        }
    };
    if !errors.is_empty() {
        errors.sort_by_key(|e| e.span);
        return Err(errors);
    }
    Ok(assemble(&ctx, &decls, &bodies, &globals, init.as_ref()))
}

fn assemble(
    ctx: &Ctx,
    decls: &[&FnDecl],
    bodies: &[Body],
    globals: &[&VarDecl],
    init: Option<&Body>,
) -> Vec<u8> {
    let mut types = TypeSection::new();
    let mut type_ids: HashMap<(Vec<ValType>, Vec<ValType>), u32> = HashMap::new();
    let mut type_of = |params: Vec<ValType>, results: Vec<ValType>| -> u32 {
        let next = type_ids.len() as u32;
        *type_ids
            .entry((params.clone(), results.clone()))
            .or_insert_with(|| {
                types.ty().function(params, results);
                next
            })
    };

    let mut functions = FunctionSection::new();
    for decl in decls {
        let params = decl
            .params
            .iter()
            .filter_map(|p| val_type(&p.resolved))
            .collect();
        let results = ctx
            .fns
            .get(&decl.name)
            .and_then(|f| f.ret.as_ref())
            .and_then(val_type)
            .into_iter()
            .collect();
        functions.function(type_of(params, results));
    }
    for helper in &ctx.helpers {
        let (params, results) = helper.signature();
        functions.function(type_of(params, results));
    }
    if init.is_some() {
        functions.function(type_of(Vec::new(), Vec::new()));
    }

    let mut global_section = GlobalSection::new();
    for v in globals {
        let val_type = val_type(&v.resolved).unwrap_or(ValType::I32);
        let zero = match val_type {
            ValType::I64 => ConstExpr::i64_const(0),
            ValType::F32 => ConstExpr::f32_const(0.0.into()),
            ValType::F64 => ConstExpr::f64_const(0.0.into()),
            _ => ConstExpr::i32_const(0),
        };
        let ty = GlobalType {
            val_type,
            mutable: true,
            shared: false,
        };
        global_section.global(ty, &zero);
    }

    let mut exports = ExportSection::new();
    for decl in decls {
        if let Some(f) = ctx.fns.get(&decl.name).filter(|f| !f.generic) {
            exports.export(&decl.name, ExportKind::Func, f.index);
        }
    }

    let mut code = CodeSection::new();
    let mut emit = |body: &Body| {
        let mut f = Function::new(body.locals.iter().map(|t| (1, *t)));
        for ins in &body.code {
            f.instruction(ins);
        }
        code.function(&f);
    };
    for body in bodies {
        emit(body);
    }
    for helper in &ctx.helpers {
        let mut code = helper.body();
        code.push(I::End);
        emit(&Body {
            locals: Vec::new(),
            code,
        });
    }
    if let Some(init) = init {
        emit(init);
    }

    let mut module = Module::new();
    module.section(&types);
    module.section(&functions);
    if !globals.is_empty() {
        module.section(&global_section);
    }
    module.section(&exports);
    if init.is_some() {
        module.section(&StartSection {
            function_index: ctx.user_fns + ctx.helpers.len() as u32,
        });
    }
    module.section(&code);
    module.finish()
}

fn function(ctx: &mut Ctx, decl: &FnDecl) -> R<Body> {
    let ret = ctx.fns.get(&decl.name).and_then(|f| f.ret.clone());
    let mut g = FnGen::new(ctx, decl.params.len() as u32, ret.clone());
    for (i, p) in decl.params.iter().enumerate() {
        if p.resolved.is_any() {
            return Err(Error::compile(format!(
                "parameter '{}' needs a type in WASM builds",
                p.name
            ))
            .at(p.span));
        }
        val_type_at(&p.resolved, p.span, &format!("parameter '{}'", p.name))?;
        g.declare(&p.name, i as u32, p.resolved.clone());
    }
    g.block(&decl.body)?;
    if ret.is_some() {
        // The checker made sure every path returns.
        g.emit(I::Unreachable);
    }
    g.emit(I::End);
    Ok(g.finish())
}

/// The start function: computes the initial values of the globals.
fn init_function(ctx: &mut Ctx, globals: &[&VarDecl]) -> R<Body> {
    let mut g = FnGen::new(ctx, 0, None);
    for (i, v) in globals.iter().enumerate() {
        g.value(&v.value, &v.resolved)?;
        g.emit(I::GlobalSet(i as u32));
    }
    g.emit(I::End);
    Ok(g.finish())
}

enum Var {
    Local(u32, Ty),
    Global(u32, Ty),
}

struct FnGen<'c> {
    ctx: &'c mut Ctx,
    nparams: u32,
    locals: Vec<ValType>,
    scopes: Vec<HashMap<String, (u32, Ty)>>,
    code: Vec<I<'static>>,
    /// Number of enclosing blocks, loops and ifs.
    depth: u32,
    /// For each enclosing loop: the depth of its `break` and `continue`
    /// targets.
    loops: Vec<(u32, u32)>,
    ret: Option<Ty>,
}

impl<'c> FnGen<'c> {
    fn new(ctx: &'c mut Ctx, nparams: u32, ret: Option<Ty>) -> Self {
        FnGen {
            ctx,
            nparams,
            locals: Vec::new(),
            scopes: vec![HashMap::new()],
            code: Vec::new(),
            depth: 0,
            loops: Vec::new(),
            ret,
        }
    }

    fn finish(self) -> Body {
        Body {
            locals: self.locals,
            code: self.code,
        }
    }

    fn emit(&mut self, ins: I<'static>) {
        self.code.push(ins);
    }

    fn new_local(&mut self, t: ValType) -> u32 {
        self.locals.push(t);
        self.nparams + self.locals.len() as u32 - 1
    }

    fn declare(&mut self, name: &str, index: u32, ty: Ty) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), (index, ty));
        }
    }

    fn lookup(&self, name: &str) -> Option<Var> {
        for scope in self.scopes.iter().rev() {
            if let Some((i, ty)) = scope.get(name) {
                return Some(Var::Local(*i, ty.clone()));
            }
        }
        self.ctx
            .globals
            .get(name)
            .map(|(i, ty)| Var::Global(*i, ty.clone()))
    }

    fn variable(&self, name: &str, span: Span) -> R<Var> {
        match self.lookup(name) {
            Some(v) => Ok(v),
            None if self.ctx.fns.contains_key(name) => {
                Err(unsupported("functions as values are", span))
            }
            None => Err(Error::compile(format!("undefined variable '{name}'")).at(span)),
        }
    }

    fn get(&mut self, var: &Var) {
        match var {
            Var::Local(i, _) => self.emit(I::LocalGet(*i)),
            Var::Global(i, _) => self.emit(I::GlobalGet(*i)),
        }
    }

    fn set(&mut self, var: &Var) {
        match var {
            Var::Local(i, _) => self.emit(I::LocalSet(*i)),
            Var::Global(i, _) => self.emit(I::GlobalSet(*i)),
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
            StmtKind::Expr(e) => {
                if self.expr(e)?.is_some() {
                    self.emit(I::Drop);
                }
            }
            StmtKind::VarDecl(d) => {
                self.value(&d.value, &d.resolved)?;
                let t = val_type_at(&d.resolved, span, &format!("variable '{}'", d.name))?;
                let index = self.new_local(t);
                self.emit(I::LocalSet(index));
                self.declare(&d.name, index, d.resolved.clone());
            }
            StmtKind::Assign { target, value } => {
                let ExprKind::Ident(name) = &target.kind else {
                    return Err(unsupported("assigning to this place is", target.span));
                };
                let var = self.variable(name, target.span)?;
                let ty = match &var {
                    Var::Local(_, t) | Var::Global(_, t) => t.clone(),
                };
                self.value(value, &ty)?;
                self.set(&var);
            }
            StmtKind::CompoundAssign {
                op,
                target,
                value,
                cast,
            } => self.compound_assign(*op, target, value, cast, span)?,
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
            StmtKind::Block(b) => self.block(b)?,
            StmtKind::For { .. } => return Err(unsupported("'for' loops are", span)),
            StmtKind::FnDecl(_) => return Err(unsupported("nested functions are", span)),
            StmtKind::Idb { .. } => return Err(no_io("idb", span)),
            StmtKind::Using(_) => return Err(unsupported("'using' is", span)),
        }
        Ok(())
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
        let ExprKind::Ident(name) = &target.kind else {
            return Err(unsupported("assigning to this place is", target.span));
        };
        let var = self.variable(name, target.span)?;
        let ty = match &var {
            Var::Local(_, t) | Var::Global(_, t) => t.clone(),
        };
        let shift = matches!(op, BinOp::Shl | BinOp::Shr);
        let op_ty = if matches!((&ty, &value.ty), (Ty::Bool, Ty::Bool)) {
            Ty::Bool
        } else {
            match (arith(&ty), arith(&value.ty)) {
                (Some(NumTy::Int(k)), _) if shift => Ty::Int(NumTy::int_promoted(k)),
                (Some(a), Some(b)) => match NumTy::promote(a, b) {
                    Some(n) => n.ty(),
                    None => return Err(unsupported(format!("'{}=' here is", op.symbol()), span)),
                },
                _ => return Err(unsupported(format!("'{}=' on {ty} is", op.symbol()), span)),
            }
        };
        self.get(&var);
        self.convert(&ty, &op_ty, span)?;
        self.expr(value)?;
        let rhs_ty = if shift { Ty::INT } else { op_ty.clone() };
        self.convert(&value.ty, &rhs_ty, span)?;
        self.binop(op, &op_ty, span)?;
        self.convert(&op_ty, cast, span)?;
        if matches!(cast, Ty::Bel) {
            let h = self.ctx.helper(Helper::Bel);
            self.emit(I::Call(h));
        }
        self.set(&var);
        Ok(())
    }

    // ---- expressions ---------------------------------------------------

    /// Emits `e` as a value stored in a place of type `target`.
    fn value(&mut self, e: &Expr, target: &Ty) -> R<()> {
        let got = self.expr(e)?;
        let want = val_type_at(target, e.span, "a value")?;
        match got {
            Some(t) if t == want => {}
            Some(_) => {
                return Err(unsupported(
                    format!("converting {} to {target} is", e.ty),
                    e.span,
                ))
            }
            None => return Err(Error::compile("this expression has no value").at(e.span)),
        }
        if matches!(target, Ty::Bel) {
            let h = self.ctx.helper(Helper::Bel);
            self.emit(I::Call(h));
        }
        Ok(())
    }

    /// Emits a `bool` condition.
    fn cond(&mut self, e: &Expr) -> R<()> {
        if !matches!(e.ty, Ty::Bool) {
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
            ExprKind::Ident(name) => {
                let var = self.variable(name, span)?;
                self.get(&var);
            }
            ExprKind::Unary { op, expr } => self.unary(*op, expr, &e.ty, span)?,
            ExprKind::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs, &e.ty, span)?,
            ExprKind::Call { callee, args, .. } => {
                let ExprKind::Ident(name) = &callee.kind else {
                    return Err(unsupported("calling this expression is", span));
                };
                if belsk2_typeck::is_builtin(name) {
                    return self.builtin(name, args, span).map(Some);
                }
                if self.lookup(name).is_some() {
                    return Err(unsupported("functions as values are", callee.span));
                }
                let Some(f) = self.ctx.fns.get(name.as_str()) else {
                    return Err(Error::compile(format!("undefined function '{name}'")).at(span));
                };
                if f.generic {
                    return Err(unsupported("generic functions are", span));
                }
                let (index, params, ret) = (f.index, f.params.clone(), f.ret.clone());
                for (arg, param) in args.iter().zip(&params) {
                    self.value(arg, param)?;
                }
                self.emit(I::Call(index));
                return Ok(ret.as_ref().and_then(val_type));
            }
            ExprKind::Convert { expr, to } => {
                self.convert_expr(expr, to, span)?;
            }
            ExprKind::String(_) => return Err(unsupported("strings are", span)),
            ExprKind::Null => return Err(unsupported("null is", span)),
            ExprKind::Array(_) => return Err(unsupported("arrays are", span)),
            ExprKind::Map(_) => return Err(unsupported("dictionaries are", span)),
            ExprKind::Member { .. } => return Err(unsupported("members are", span)),
            ExprKind::Index { .. } => return Err(unsupported("indexing is", span)),
        }
        val_type(&e.ty)
            .map(Some)
            .ok_or_else(|| unsupported(format!("a value of type {} is", e.ty), span))
    }

    fn builtin(&mut self, name: &str, args: &[Expr], span: Span) -> R<ValType> {
        let target = match name {
            "prinb" | "input" | "reab" => return Err(no_io(&format!("{name}()"), span)),
            "num" => Some(Ty::DOUBLE),
            _ => Ty::from_name(name).filter(|t| t.num().is_some() || *t == Ty::Char),
        };
        let (Some(target), Some(arg)) = (target, args.first()) else {
            return Err(unsupported(format!("{name}() is"), span));
        };
        self.convert_expr(arg, &target, span)?;
        val_type_at(&target, span, "a value")
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
        if self.expr(e)?.is_none() {
            return Err(Error::compile("this expression has no value").at(e.span));
        }
        self.convert(&e.ty, to, span)
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
            let h = self.ctx.helper(Helper::Char);
            self.emit(I::Call(h));
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
                    self.convert(&inner.ty, ty, span)?;
                    self.emit(if k.bits() == 64 { I::I64Sub } else { I::I32Sub });
                }
                Some(NumTy::Float(k)) => {
                    self.expr(inner)?;
                    self.convert(&inner.ty, ty, span)?;
                    self.emit(if k == FloatKind::F32 {
                        I::F32Neg
                    } else {
                        I::F64Neg
                    });
                }
                None => return Err(unsupported(format!("negating {} is", inner.ty), span)),
            },
            UnaryOp::BitNot => {
                let Some(NumTy::Int(k)) = arith(ty) else {
                    return Err(unsupported(format!("'~' on {} is", inner.ty), span));
                };
                self.expr(inner)?;
                self.convert(&inner.ty, ty, span)?;
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
        match op {
            BinOp::And | BinOp::Or => {
                self.cond(lhs)?;
                self.emit(I::If(BlockType::Result(ValType::I32)));
                self.depth += 1;
                if op == BinOp::And {
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
            _ => {}
        }
        let text = |t: &Ty| matches!(t, Ty::String | Ty::Ster);
        if text(&lhs.ty) || text(&rhs.ty) || (op == BinOp::Add && *ty == Ty::String) {
            return Err(unsupported("string operations are", span));
        }
        self.expr(lhs)?;
        self.expr(rhs)?;
        if matches!(
            op,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge
        ) {
            return self.compare(op, &lhs.ty, span);
        }
        self.binop(op, ty, span).map(|_| ())
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
                    (Ge, false, false) => I::I32GeU,
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
                    let b = self.new_local(ValType::F32);
                    self.emit(I::LocalSet(b));
                    self.emit(I::F64PromoteF32);
                    self.emit(I::LocalGet(b));
                    self.emit(I::F64PromoteF32);
                    let h = self.ctx.helper(Helper::Fmod);
                    self.emit(I::Call(h));
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
