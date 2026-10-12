use std::collections::HashMap;
use std::rc::Rc;

use belsk2_syntax::ast::*;
use belsk2_syntax::{
    Access, Error, FieldInfo, IntKind, MemberInfo, MethodInfo, NumTy, PropInfo, Sig, Span, Ty,
    TypeBody, TypeExpr, TypeExprKind, TypeInfo, TypeKind, TypeRef, VariantInfo, Virt,
};

use crate::builtins;

struct FnCtx {
    name: String,
    ret: Ty,
}

/// A method (or `init`) being checked: its members can be used without
/// `this.`.
struct TypeCtx {
    ty: TypeRef,
    /// The index in `scopes` of the method's parameters: names declared
    /// from there on are locals and hide the members.
    depth: usize,
    /// A static method or field value: there is no `this`.
    is_static: bool,
    /// `init`: read-only fields may be assigned.
    in_init: bool,
}

/// Walks a program, infers and records types, inserts implicit conversions
/// and collects every error it finds.
pub struct Checker {
    scopes: Vec<HashMap<String, Ty>>,
    /// Names imported with `using`, and the module each comes from. They
    /// live in the outermost scope and are read-only.
    imports: HashMap<String, String>,
    /// Whether statements may appear outside functions (only in the REPL).
    allow_statements: bool,
    /// Type parameters of the generic functions being checked.
    type_params: Vec<Vec<Rc<str>>>,
    fns: Vec<FnCtx>,
    types: Vec<TypeCtx>,
    errors: Vec<Error>,
}

/// Grows the stack on demand so deep (but bounded) recursion never overflows.
fn with_stack<T>(f: impl FnOnce() -> T) -> T {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, f)
}

/// A value known at compile time.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Const {
    Int(i128),
    Float(f64),
    Str(String),
}

pub(crate) fn const_of(e: &Expr) -> Option<Const> {
    match &e.kind {
        ExprKind::Int(v, _) => Some(Const::Int(*v)),
        ExprKind::Float(v, _) => Some(Const::Float(*v)),
        ExprKind::String(s) => Some(Const::Str(s.clone())),
        ExprKind::Unary {
            op: UnaryOp::Neg,
            expr,
        } => match const_of(expr)? {
            Const::Int(v) => Some(Const::Int(-v)),
            Const::Float(v) => Some(Const::Float(-v)),
            Const::Str(_) => None,
        },
        ExprKind::Convert { expr, .. } => const_of(expr),
        _ => None,
    }
}

/// The signature of a function whose types have been resolved.
pub(crate) fn sig_of(decl: &FnDecl) -> Sig {
    Sig {
        name: decl.name.clone(),
        type_params: decl
            .type_params
            .iter()
            .map(|t| Rc::from(t.as_str()))
            .collect(),
        params: decl.params.iter().map(|p| p.resolved.clone()).collect(),
        ret: decl.ret_resolved.clone(),
    }
}

/// Wraps `e` in an implicit conversion to `to`.
fn insert_convert(e: &mut Expr, to: &Ty) {
    let span = e.span;
    let inner = std::mem::replace(e, Expr::new(ExprKind::Null, span));
    *e = Expr::new(
        ExprKind::Convert {
            expr: Box::new(inner),
            to: to.clone(),
        },
        span,
    );
    e.ty = to.clone();
}

/// How a value of one type becomes another.
enum Conv {
    /// Already the right type (or checked at run time).
    Keep,
    /// An implicit conversion node is needed.
    Convert,
    /// Not allowed.
    No,
}

/// The implicit conversions of Belsk2: C#'s widening conversions, constants
/// that fit the target type, plus `char` to `string` and one-character
/// string constants to `char`.
fn conversion(from: &Ty, to: &Ty, cnst: Option<&Const>) -> Conv {
    if from.is_any() || to.is_any() {
        return Conv::Keep;
    }
    if matches!(from, Ty::Null) {
        return if to.nullable() { Conv::Keep } else { Conv::No };
    }
    let (f, t) = (from.canonical(), to.canonical());
    if f == t {
        return Conv::Keep;
    }
    if let (Some(fnum), Some(tnum)) = (f.num(), t.num()) {
        if fnum.widens_to(tnum) {
            return Conv::Convert;
        }
        // C# lets an `int` constant (or a non-negative `long` constant, for
        // `ulong`) convert to any integer type that can hold it.
        let int_const = fnum == NumTy::Int(IntKind::I32)
            || (fnum == NumTy::Int(IntKind::I64) && tnum == NumTy::Int(IntKind::U64));
        return match (cnst, tnum) {
            (Some(Const::Int(v)), NumTy::Int(k)) if int_const && k.fits(*v) => Conv::Convert,
            // `float x = 1.5;` is accepted (C# would require `1.5f`).
            (Some(Const::Float(_)), NumTy::Float(_)) => Conv::Convert,
            _ => Conv::No,
        };
    }
    match (&f, &t) {
        (Ty::Char, Ty::String) => Conv::Convert,
        (Ty::Char, _)
            if t.num()
                .is_some_and(|n| NumTy::Int(IntKind::U16).widens_to(n)) =>
        {
            Conv::Convert
        }
        (Ty::String, Ty::Char) => match cnst {
            Some(Const::Str(s)) if s.chars().count() == 1 => Conv::Convert,
            _ => Conv::No,
        },
        (Ty::Array(a), Ty::Array(b)) if a.is_any() || b.is_any() || a == b => Conv::Keep,
        (Ty::Map(ka, va), Ty::Map(kb, vb))
            if (ka.is_any() || kb.is_any() || ka == kb)
                && (va.is_any() || vb.is_any() || va == vb) =>
        {
            Conv::Keep
        }
        (Ty::Fn(_), Ty::Fn(_)) => Conv::Keep,
        // A `Dog` is an `Animal`; a class is any interface it implements.
        (Ty::Named(a), Ty::Named(b)) if a.is_subtype_of(b) => Conv::Keep,
        _ => Conv::No,
    }
}

/// Numeric view of a type for arithmetic; `char` counts as `ushort`.
fn arith(t: &Ty) -> Option<NumTy> {
    match t {
        Ty::Char => Some(NumTy::Int(IntKind::U16)),
        t => t.num(),
    }
}

fn is_integer(t: &Ty) -> bool {
    matches!(arith(t), Some(NumTy::Int(_)))
}

fn is_stringy(t: &Ty) -> bool {
    matches!(t, Ty::String | Ty::Ster)
}

/// Whether control can never fall off the end of these statements.
fn always_returns(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|s| match &s.kind {
        StmtKind::Return(_) => true,
        StmtKind::Block(b) => always_returns(&b.stmts),
        StmtKind::If {
            then,
            otherwise: Some(other),
            ..
        } => always_returns(&then.stmts) && always_returns(std::slice::from_ref(other)),
        StmtKind::While { cond, body } => {
            matches!(cond.kind, ExprKind::Bool(true)) && !contains_break(&body.stmts)
        }
        _ => false,
    })
}

fn contains_break(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|s| match &s.kind {
        StmtKind::Break => true,
        StmtKind::Block(b) => contains_break(&b.stmts),
        StmtKind::If {
            then, otherwise, ..
        } => {
            contains_break(&then.stmts)
                || otherwise
                    .as_deref()
                    .is_some_and(|o| contains_break(std::slice::from_ref(o)))
        }
        // A `break` inside a nested loop belongs to that loop.
        _ => false,
    })
}

impl Default for Checker {
    fn default() -> Self {
        Self::new()
    }
}

impl Checker {
    pub fn new() -> Self {
        Checker {
            scopes: vec![HashMap::new()],
            imports: HashMap::new(),
            allow_statements: false,
            type_params: Vec::new(),
            fns: Vec::new(),
            types: Vec::new(),
            errors: Vec::new(),
        }
    }

    /// Makes a name known before checking (host-defined globals, or
    /// declarations from earlier REPL input).
    pub fn declare_global(&mut self, name: &str, ty: Ty) {
        if let Some(outer) = self.scopes.first_mut() {
            outer.insert(name.to_string(), ty);
        }
    }

    /// Lets a program run statements at its top level. Programs keep their
    /// code in functions; this is for interactive input such as a REPL.
    pub fn allow_top_level_statements(&mut self) {
        self.allow_statements = true;
    }

    /// Makes a declaration of another module visible (read-only), as
    /// `using` does.
    pub fn declare_import(&mut self, name: &str, ty: Ty, module: &str) {
        self.declare_global(name, ty);
        self.imports.insert(name.to_string(), module.to_string());
    }

    /// Checks a program, filling in types and implicit conversions.
    /// Returns all errors, in source order.
    pub fn check_program(self, program: &mut Program) -> Vec<Error> {
        self.check_module(program).0
    }

    /// Like [`Checker::check_program`], also returning the types of the
    /// program's top-level declarations, which other modules can import.
    pub fn check_module(mut self, program: &mut Program) -> (Vec<Error>, Vec<(String, Ty)>) {
        // The program gets its own scope so it may shadow predeclared globals.
        self.scopes.push(HashMap::new());
        if !self.allow_statements {
            for stmt in &program.stmts {
                let declaration = matches!(
                    stmt.kind,
                    StmtKind::Using(_)
                        | StmtKind::FnDecl(_)
                        | StmtKind::Type(_)
                        | StmtKind::VarDecl(_)
                        | StmtKind::Idb { .. }
                );
                if !declaration {
                    self.type_error(
                        "code must be inside a function; put it in `fn main() { ... }`",
                        stmt.span,
                    );
                }
            }
        }
        self.check_stmts(&mut program.stmts);
        for stmt in &program.stmts {
            if let StmtKind::FnDecl(decl) = &stmt.kind {
                if decl.name == "main" {
                    self.check_main(decl);
                }
            }
        }
        self.errors.sort_by_key(|e| e.span);
        // `private` declarations stay in their module.
        let private: Vec<&str> = program
            .stmts
            .iter()
            .filter_map(|s| match &s.kind {
                StmtKind::FnDecl(d) if d.access == Access::Private => Some(d.name.as_str()),
                StmtKind::VarDecl(d) if d.access == Access::Private => Some(d.name.as_str()),
                StmtKind::Type(d) if d.access == Access::Private => Some(d.name.as_str()),
                _ => None,
            })
            .collect();
        let exports = self
            .scopes
            .pop()
            .map(|s| {
                s.into_iter()
                    .filter(|(name, _)| !private.contains(&name.as_str()))
                    .collect()
            })
            .unwrap_or_default();
        (self.errors, exports)
    }

    fn error(&mut self, e: Error) {
        self.errors.push(e);
    }

    fn type_error(&mut self, msg: impl Into<String>, span: Span) {
        self.error(Error::type_error(msg).at(span));
    }

    /// Whether `name` currently refers to an imported declaration.
    fn is_import(&self, name: &str) -> bool {
        let innermost = self.scopes.iter().rposition(|s| s.contains_key(name));
        innermost == Some(0) && self.imports.contains_key(name)
    }

    fn lookup(&self, name: &str) -> Option<&Ty> {
        self.scopes.iter().rev().find_map(|s| s.get(name))
    }

    fn declare(&mut self, name: &str, ty: Ty, span: Span) {
        let Some(scope) = self.scopes.last_mut() else {
            return;
        };
        if scope.contains_key(name) {
            self.type_error(format!("'{name}' is already declared in this scope"), span);
            return;
        }
        scope.insert(name.to_string(), ty);
    }

    fn with_scope(&mut self, f: impl FnOnce(&mut Self)) {
        self.scopes.push(HashMap::new());
        f(self);
        self.scopes.pop();
    }

    fn resolve(&mut self, t: &TypeExpr) -> Ty {
        let t_span = t.span;
        match &t.kind {
            TypeExprKind::Array(elem) => Ty::array(self.resolve(elem)),
            TypeExprKind::Named { name, args } => {
                let params = self.type_params.iter().rev().flatten();
                if let Some(p) = params.into_iter().find(|p| &***p == name.as_str()).cloned() {
                    if !args.is_empty() {
                        self.type_error(
                            format!("type parameter '{name}' does not take type arguments"),
                            t.span,
                        );
                    }
                    return Ty::Param(p);
                }
                if let Some(Ty::Type(t)) = self.lookup(name) {
                    let named = Ty::Named(t.clone());
                    if !args.is_empty() {
                        self.type_error(format!("'{name}' does not take type arguments"), t_span);
                    }
                    return named;
                }
                let expected = match name.as_str() {
                    "List" => 1,
                    "Dictionary" | "Map" => 2,
                    _ => 0,
                };
                if args.len() != expected {
                    let msg = match expected {
                        0 => format!("'{name}' does not take type arguments"),
                        1 => format!("'{name}' needs 1 type argument, as in {name}<int>"),
                        _ => format!(
                            "'{name}' needs {expected} type arguments, as in {name}<string, int>"
                        ),
                    };
                    self.type_error(msg, t.span);
                    return Ty::Any;
                }
                let mut args = args
                    .iter()
                    .map(|a| (self.resolve(a), a.span))
                    .collect::<Vec<_>>()
                    .into_iter();
                match (name.as_str(), args.next(), args.next()) {
                    ("List", Some((elem, _)), _) => return Ty::array(elem),
                    (_, Some((key, kspan)), Some((value, _))) => {
                        if !key.is_key() {
                            self.type_error(
                                format!("{key} cannot be a dictionary key; use string, an integer type, char or bool"),
                                kspan,
                            );
                        }
                        return Ty::map(key, value);
                    }
                    _ => {}
                }
                match Ty::from_name(name) {
                    Some(ty) => ty,
                    None => {
                        self.type_error(format!("unknown type '{name}'"), t.span);
                        Ty::Any
                    }
                }
            }
        }
    }

    /// Checks that `e` can be used where `to` is expected, inserting an
    /// implicit conversion if needed. `what` describes the place for errors.
    fn coerce(&mut self, e: &mut Expr, to: &Ty, what: &dyn Fn() -> String) {
        // Literals take the type they are used as: `double[] a = [1, 2];`,
        // `int[] empty = [];`, `Dictionary<string, long> m = {"a": 1};`.
        match (&mut e.kind, to) {
            (ExprKind::Array(items), Ty::Array(elem)) => {
                for item in items.iter_mut() {
                    self.coerce(item, elem, &|| format!("an element of {}", what()));
                }
                e.ty = to.clone();
                return;
            }
            (ExprKind::Map(entries), Ty::Map(k, v)) => {
                for (key, value) in entries.iter_mut() {
                    self.coerce(key, k, &|| format!("a key of {}", what()));
                    self.coerce(value, v, &|| format!("a value of {}", what()));
                }
                e.ty = to.clone();
                return;
            }
            _ => {}
        }
        let from = e.ty.clone();
        let cnst = const_of(e);
        match conversion(&from, to, cnst.as_ref()) {
            Conv::Keep => {}
            Conv::Convert => insert_convert(e, to),
            Conv::No => {
                let msg = if matches!(from, Ty::Null) {
                    format!("{} of type {to} cannot be null", what())
                } else if to.num().is_some() && arith(&from).is_some() {
                    format!(
                        "cannot implicitly convert {from} to {to} in {}; use {}(...) to convert",
                        what(),
                        to.canonical()
                    )
                } else {
                    format!("cannot use {from} as {to} in {}", what())
                };
                self.type_error(msg, e.span);
                return;
            }
        }
        if matches!(to, Ty::Bel) {
            let v = match cnst {
                Some(Const::Int(v)) => Some(v as f64),
                Some(Const::Float(v)) => Some(v),
                _ => None,
            };
            if let Some(v) = v.filter(|v| *v > 1000.0) {
                self.type_error(
                    format!("bel value {v} exceeds the maximum of 1000 in {}", what()),
                    e.span,
                );
            }
        }
    }

    // ---- statements ----------------------------------------------------

    /// Functions are visible in their whole block, so their signatures are
    /// resolved and declared before any statement is checked.
    fn check_stmts(&mut self, stmts: &mut [Stmt]) {
        self.declare_types(stmts);
        for stmt in stmts.iter_mut() {
            let span = stmt.span;
            if let StmtKind::FnDecl(decl) = &mut stmt.kind {
                let Some(decl) = Rc::get_mut(decl) else {
                    self.type_error("internal error: function is shared before checking", span);
                    continue;
                };
                self.resolve_signature(decl);
                if builtins::is_builtin(&decl.name) {
                    self.type_error(
                        format!(
                            "'{}' is a built-in function and cannot be redefined",
                            decl.name
                        ),
                        span,
                    );
                }
                let ty = Ty::Fn(Some(Rc::new(sig_of(decl))));
                self.declare(&decl.name, ty, span);
            }
        }
        for stmt in stmts.iter_mut() {
            with_stack(|| self.check_stmt(stmt));
        }
    }

    /// Resolves the parameter and return types of a function.
    fn resolve_signature(&mut self, decl: &mut FnDecl) {
        self.type_params.push(
            decl.type_params
                .iter()
                .map(|t| Rc::from(t.as_str()))
                .collect(),
        );
        for p in &mut decl.params {
            p.resolved = match &p.ty {
                Some(t) => self.resolve(t),
                None => Ty::Any,
            };
        }
        decl.ret_resolved = match &decl.ret {
            Some(t) => self.resolve(t),
            None => Ty::Any,
        };
        self.type_params.pop();
    }

    /// Types are visible in their whole block, like functions: first every
    /// name is declared, then the members are resolved (they may mention
    /// any of the types).
    fn declare_types(&mut self, stmts: &mut [Stmt]) {
        let mut any = false;
        for stmt in stmts.iter_mut() {
            let span = stmt.span;
            if let StmtKind::Type(decl) = &mut stmt.kind {
                let Some(decl) = Rc::get_mut(decl) else {
                    self.type_error("internal error: type is shared before checking", span);
                    continue;
                };
                let info = TypeInfo::new(&decl.name, decl.kind, decl.is_abstract, span);
                if builtins::is_builtin(&decl.name) || Ty::from_name(&decl.name).is_some() {
                    self.type_error(
                        format!(
                            "'{}' is a built-in name and cannot be a type name",
                            decl.name
                        ),
                        span,
                    );
                }
                self.declare(&decl.name, Ty::Type(info.reference()), span);
                decl.info = Some(info);
                any = true;
            }
        }
        if !any {
            return;
        }
        for stmt in stmts.iter_mut() {
            if let StmtKind::Type(decl) = &mut stmt.kind {
                if let Some(decl) = Rc::get_mut(decl) {
                    self.resolve_members(decl);
                }
            }
        }
        for stmt in stmts.iter() {
            if let StmtKind::Type(decl) = &stmt.kind {
                self.validate_type(decl);
                if let Some(info) = &decl.info {
                    let t = info.reference();
                    if t.kind == TypeKind::Struct && contains_struct(&t, &t, 0) {
                        self.type_error(
                            format!(
                                "struct '{}' contains itself, so it would be infinitely large; use an array or a nullable type for the field",
                                decl.name
                            ),
                            decl.span,
                        );
                    }
                }
            }
        }
    }

    /// Resolves the fields, methods and constructor of a type.
    fn resolve_members(&mut self, decl: &mut TypeDecl) {
        let Some(info) = decl.info.clone() else {
            return;
        };
        let mut body = TypeBody::default();
        let interface = decl.kind == TypeKind::Interface;
        for (i, b) in decl.bases.iter().enumerate() {
            let ty = self.resolve(b);
            let Ty::Named(t) = ty else {
                if !ty.is_any() {
                    self.type_error(format!("cannot inherit from {ty}"), b.span);
                }
                continue;
            };
            match t.kind {
                TypeKind::Interface => body.interfaces.push(t),
                TypeKind::Class if decl.kind == TypeKind::Class && i == 0 => body.base = Some(t),
                TypeKind::Class if decl.kind == TypeKind::Class => self.type_error(
                    format!(
                        "the base class {} must come first, before the interfaces",
                        t.name
                    ),
                    b.span,
                ),
                TypeKind::Class => self.type_error(
                    format!(
                        "a {} cannot inherit from a class; it can only implement interfaces",
                        decl.kind.keyword()
                    ),
                    b.span,
                ),
                kind => self.type_error(
                    format!(
                        "cannot inherit from {} {}; only from a class or an interface",
                        kind.keyword(),
                        t.name
                    ),
                    b.span,
                ),
            }
        }
        let mut names: Vec<String> = Vec::new();
        let mut next_value: i128 = 0;
        for m in &mut decl.members {
            let span = m.span;
            let access = m.mods.access();
            let is_static = m.mods.is_static;
            let virt = if interface {
                Virt::Abstract
            } else {
                m.mods.virt
            };
            if interface && (is_static || m.mods.access.is_some() || m.mods.virt != Virt::None) {
                self.type_error(
                    "members of an interface are public and have no modifiers",
                    span,
                );
            }
            let name = match &m.kind {
                MemberKind::Field(f) => f.name.clone(),
                MemberKind::Prop(p) => p.name.clone(),
                MemberKind::Method(d) => d.name.clone(),
                MemberKind::Init(_) => "init".to_string(),
                MemberKind::Case { name, .. } => name.clone(),
            };
            if names.contains(&name) {
                let what = if name == "init" {
                    format!("'{}' already has an init", decl.name)
                } else {
                    format!("'{}' already has a member '{name}'", decl.name)
                };
                self.type_error(what, span);
                continue;
            }
            names.push(name.clone());
            match &mut m.kind {
                MemberKind::Field(f) => {
                    f.resolved = match (&f.ty, &mut f.value) {
                        (Some(t), _) => self.resolve(t),
                        (None, Some(value)) => {
                            let ty = self.expr(value);
                            if matches!(ty, Ty::Null | Ty::Type(_)) {
                                self.type_error(
                                    format!("cannot infer the type of field '{name}'; write it as `{name}: type`"),
                                    span,
                                );
                                Ty::Any
                            } else {
                                ty
                            }
                        }
                        (None, None) => Ty::Any,
                    };
                    // `Name: T { get; }` in an interface asks for a property.
                    if interface {
                        if !f.property || f.value.is_some() {
                            self.type_error(
                                "an interface has no fields; ask for a property: `Name: T { get; }`",
                                span,
                            );
                        }
                        body.props.push(PropInfo {
                            name,
                            ty: f.resolved.clone(),
                            access: Access::Public,
                            get: Some(Access::Public),
                            set: (!f.readonly).then_some(Access::Public),
                            is_static: false,
                            virt: Virt::Abstract,
                            span,
                        });
                        continue;
                    }
                    if m.mods.virt != Virt::None {
                        self.type_error("fields cannot be virtual", span);
                    }
                    let field = FieldInfo {
                        name,
                        ty: f.resolved.clone(),
                        access,
                        set: (!f.readonly).then(|| f.set_access.unwrap_or(access)),
                        is_static,
                        has_value: f.value.is_some(),
                        span,
                    };
                    if is_static {
                        body.statics.push(field);
                    } else {
                        body.fields.push(field);
                    }
                }
                MemberKind::Prop(p) => {
                    p.resolved = self.resolve(&p.ty);
                    for accessor in p.get.iter_mut().chain(p.set.iter_mut()) {
                        if let Some(d) = Rc::get_mut(&mut accessor.decl) {
                            self.resolve_signature(d);
                            if d.ret.is_none() {
                                d.ret_resolved = Ty::Null;
                            }
                        }
                    }
                    if interface {
                        self.type_error(
                            "an interface property has no bodies: `Name: T { get; set; }`",
                            span,
                        );
                    }
                    if m.mods.virt != Virt::None {
                        self.type_error("properties with code cannot be virtual yet", span);
                    }
                    body.props.push(PropInfo {
                        name,
                        ty: p.resolved.clone(),
                        access,
                        get: p.get.as_ref().map(|a| a.access.unwrap_or(access)),
                        set: p.set.as_ref().map(|a| a.access.unwrap_or(access)),
                        is_static,
                        virt: Virt::None,
                        span,
                    });
                }
                MemberKind::Method(d) => {
                    let Some(d) = Rc::get_mut(d) else {
                        continue;
                    };
                    self.resolve_signature(d);
                    body.methods.push(MethodInfo {
                        sig: Rc::new(sig_of(d)),
                        access,
                        is_static,
                        virt,
                        span,
                    });
                }
                MemberKind::Init(d) => {
                    if is_static || m.mods.virt != Virt::None || interface {
                        self.type_error(
                            "'init' cannot be static, virtual or in an interface",
                            span,
                        );
                    }
                    let Some(d) = Rc::get_mut(d) else {
                        continue;
                    };
                    self.resolve_signature(d);
                    d.ret_resolved = Ty::Null;
                    body.ctor = d
                        .params
                        .iter()
                        .map(|p| (p.name.clone(), p.resolved.clone()))
                        .collect();
                    body.explicit_ctor = true;
                    body.ctor_access = access;
                }
                MemberKind::Case { value, .. } => {
                    if let Some(e) = value {
                        self.expr(e);
                        match const_of(e) {
                            Some(Const::Int(v)) if is_integer(&e.ty) => next_value = v,
                            _ => self.type_error(
                                "the value of an enum case must be a whole number, like `High = 10`",
                                e.span,
                            ),
                        }
                    }
                    body.variants.push(VariantInfo {
                        name,
                        fields: Vec::new(),
                        value: next_value,
                        span,
                    });
                    next_value = next_value.saturating_add(1);
                }
            }
        }
        if !body.explicit_ctor {
            body.ctor = body
                .fields
                .iter()
                .filter(|f| !f.has_value)
                .map(|f| (f.name.clone(), f.ty.clone()))
                .collect();
        }
        info.set_body(body);
    }

    /// Checks how a type fits with its base class and interfaces:
    /// overrides, abstract methods, implemented interfaces, `base(...)`.
    fn validate_type(&mut self, decl: &TypeDecl) {
        let Some(info) = &decl.info else {
            return;
        };
        let t = info.reference();
        let base = t.base();
        if let Some(b) = &base {
            if b.chain().contains(&t) {
                self.type_error(format!("class {} inherits from itself", t.name), decl.span);
                return;
            }
        }
        for m in &decl.members {
            let (name, is_method) = match &m.kind {
                MemberKind::Method(d) => (&d.name, true),
                MemberKind::Field(f) => (&f.name, false),
                MemberKind::Prop(p) => (&p.name, false),
                _ => continue,
            };
            let virt = m.mods.virt;
            if virt != Virt::None && decl.kind != TypeKind::Class {
                self.type_error(
                    "'virtual', 'override' and 'abstract' are for members of classes",
                    m.span,
                );
                continue;
            }
            if virt != Virt::None && m.mods.is_static {
                self.type_error("a static method cannot be virtual", m.span);
            }
            if virt == Virt::Abstract && !decl.is_abstract {
                self.type_error(
                    format!(
                        "'{name}' is abstract, so class {} must be `abstract class`",
                        t.name
                    ),
                    m.span,
                );
            }
            let inherited = base.as_ref().and_then(|b| b.member(name));
            match (virt, inherited) {
                (Virt::Override, Some((owner, MemberInfo::Method(bm))))
                    if is_method && bm.virt != Virt::None =>
                {
                    let own = t.with_body(|b| {
                        b.methods.iter().find(|x| x.sig.name == *name).cloned()
                    });
                    if let Some(Some(own)) = own {
                        if own.sig.params != bm.sig.params || own.sig.ret != bm.sig.ret {
                            self.type_error(
                                format!(
                                    "'{name}' must take and return the same types as in {}",
                                    owner.name
                                ),
                                m.span,
                            );
                        }
                        if own.access != bm.access {
                            self.type_error(
                                format!(
                                    "'{name}' must be {} as in {}",
                                    bm.access.keyword(),
                                    owner.name
                                ),
                                m.span,
                            );
                        }
                    }
                }
                (Virt::Override, _) => self.type_error(
                    format!("there is no virtual method '{name}' in a base class to override"),
                    m.span,
                ),
                (_, Some((owner, _))) => self.type_error(
                    format!(
                        "'{name}' is already a member of {}; to replace a method, make it virtual there and use override",
                        owner.name
                    ),
                    m.span,
                ),
                _ => {}
            }
        }
        if decl.kind == TypeKind::Interface || decl.is_abstract {
            return;
        }
        // Every abstract method needs an override.
        for (owner, name, m) in t.members() {
            if let MemberInfo::Method(mi) = &m {
                if mi.virt == Virt::Abstract && owner.kind == TypeKind::Class {
                    self.type_error(
                        format!(
                            "{} must override the abstract method '{name}' of {}",
                            t.name, owner.name
                        ),
                        decl.span,
                    );
                }
            }
        }
        // Every member of every interface needs an implementation.
        for iface in all_interfaces(&t) {
            let required = iface
                .with_body(|b| {
                    let mut v: Vec<MemberInfo> =
                        b.methods.iter().cloned().map(MemberInfo::Method).collect();
                    v.extend(b.props.iter().cloned().map(MemberInfo::Prop));
                    v
                })
                .unwrap_or_default();
            for req in required {
                let (name, ok) = match &req {
                    MemberInfo::Method(want) => {
                        let found = class_member(&t, &want.sig.name);
                        let ok = matches!(&found, Some(MemberInfo::Method(have))
                            if !have.is_static
                                && have.access == Access::Public
                                && have.sig.params == want.sig.params
                                && have.sig.ret == want.sig.ret);
                        (want.sig.name.clone(), ok)
                    }
                    MemberInfo::Prop(want) => {
                        let found = class_member(&t, &want.name);
                        let ok = match &found {
                            Some(MemberInfo::Field(f)) => {
                                !f.is_static
                                    && f.ty == want.ty
                                    && f.access == Access::Public
                                    && (want.set.is_none() || f.set == Some(Access::Public))
                            }
                            Some(MemberInfo::Prop(p)) => {
                                !p.is_static
                                    && p.ty == want.ty
                                    && p.get == Some(Access::Public)
                                    && (want.set.is_none() || p.set == Some(Access::Public))
                            }
                            _ => false,
                        };
                        (want.name.clone(), ok)
                    }
                    _ => continue,
                };
                if !ok {
                    let shape = match &req {
                        MemberInfo::Method(want) => format!(
                            "fn {name}({}){}",
                            want.sig
                                .params
                                .iter()
                                .map(|p| p.to_string())
                                .collect::<Vec<_>>()
                                .join(", "),
                            if want.sig.ret.is_any() {
                                String::new()
                            } else {
                                format!(": {}", want.sig.ret)
                            }
                        ),
                        MemberInfo::Prop(want) => format!(
                            "{name}: {} {{ get;{} }}",
                            want.ty,
                            if want.set.is_some() { " set;" } else { "" }
                        ),
                        _ => name.clone(),
                    };
                    self.type_error(
                        format!(
                            "{} does not implement `{shape}` of interface {} (it must be public)",
                            t.name, iface.name
                        ),
                        decl.span,
                    );
                }
            }
        }
        // The base class's init runs first; it needs its arguments.
        if let Some(b) = &base {
            let needs_args = b.with_body(|x| !x.ctor.is_empty()).unwrap_or(false);
            let init = decl.members.iter().find_map(|m| match &m.kind {
                MemberKind::Init(d) => Some(d),
                _ => None,
            });
            let calls_base = init.is_some_and(|d| {
                matches!(d.body.stmts.first().map(|s| &s.kind),
                    Some(StmtKind::Expr(Expr { kind: ExprKind::Call { callee, .. }, .. }))
                        if matches!(callee.kind, ExprKind::Base))
            });
            if needs_args && !calls_base {
                self.type_error(
                    format!(
                        "the init of {} takes arguments, so {} needs `init(...) : base(...)`",
                        b.name, t.name
                    ),
                    decl.span,
                );
            }
        }
    }

    /// Checks field values, method bodies and property accessors.
    fn check_type(&mut self, decl: &mut TypeDecl) {
        let Some(info) = decl.info.clone() else {
            return;
        };
        let ty = info.reference();
        for m in &mut decl.members {
            let span = m.span;
            let is_static = m.mods.is_static;
            let in_init = matches!(m.kind, MemberKind::Init(_));
            let ctx = |c: &Self, is_static: bool, in_init: bool| TypeCtx {
                ty: ty.clone(),
                depth: c.scopes.len(),
                is_static,
                in_init,
            };
            match &mut m.kind {
                MemberKind::Field(f) => {
                    if let (Some(_), Some(value)) = (&f.ty, &mut f.value) {
                        let c = ctx(self, true, false);
                        self.types.push(c);
                        self.expr(value);
                        let name = f.name.clone();
                        self.coerce(value, &f.resolved, &|| format!("field '{name}'"));
                        self.types.pop();
                    }
                }
                MemberKind::Case { .. } => {}
                MemberKind::Prop(p) => {
                    for accessor in p.get.iter_mut().chain(p.set.iter_mut()) {
                        if let Some(d) = Rc::get_mut(&mut accessor.decl) {
                            let c = ctx(self, is_static, false);
                            self.types.push(c);
                            self.check_fn(d);
                            self.types.pop();
                        }
                    }
                }
                MemberKind::Method(_)
                    if decl.kind == TypeKind::Interface || m.mods.virt == Virt::Abstract => {}
                MemberKind::Method(d) | MemberKind::Init(d) => match Rc::get_mut(d) {
                    Some(d) => {
                        let c = ctx(self, is_static, in_init);
                        self.types.push(c);
                        self.check_fn(d);
                        self.types.pop();
                    }
                    None => {
                        self.type_error("internal error: method is shared before checking", span)
                    }
                },
            }
        }
    }

    /// Whether code here may use a member that `owner` declares with
    /// `access`.
    fn can_access(&self, owner: &TypeRef, access: Access) -> bool {
        match access {
            Access::Public => true,
            Access::Private => self.types.iter().any(|c| c.ty == *owner),
            Access::Protected => self.types.iter().any(|c| c.ty.is_subtype_of(owner)),
        }
    }

    fn check_access(&mut self, owner: &TypeRef, access: Access, what: &str, span: Span) {
        if !self.can_access(owner, access) {
            self.type_error(
                format!("{what} is {} in {}", access.keyword(), owner.name),
                span,
            );
        }
    }

    /// Finds `name` on a value (`Ty::Named`) or a type (`Ty::Type`) and
    /// checks that it is used the right way (static or not) and may be
    /// used here.
    fn find_member(
        &mut self,
        obj: &Ty,
        name: &str,
        span: Span,
    ) -> Option<(TypeRef, TypeRef, MemberInfo)> {
        let (t, of_type) = match obj {
            Ty::Type(t) => (t, true),
            Ty::Named(t) => (t, false),
            other => {
                self.type_error(format!("{other} has no member '{name}'"), span);
                return None;
            }
        };
        let Some((owner, m)) = t.member(name) else {
            self.type_error(format!("{} has no member '{name}'", t.name), span);
            return None;
        };
        if m.is_static() != of_type {
            let msg = match &m {
                _ if of_type => {
                    format!("'{name}' belongs to a value of {}, not to the type", t.name)
                }
                MemberInfo::Variant(_) => {
                    format!("a case is used through the type: {}.{name}", t.name)
                }
                _ => format!(
                    "'{name}' is static; use it through the type: {}.{name}",
                    t.name
                ),
            };
            self.type_error(msg, span);
            return None;
        }
        self.check_access(&owner, m.access(), &format!("'{name}'"), span);
        Some((t.clone(), owner, m))
    }

    /// Whether `name`, used inside a method    /// Whether `name`, used inside a method, means a member of `this`:
    /// it is not a local variable and the type has such a member.
    fn is_member_of_this(&self, name: &str) -> bool {
        let Some(ctx) = self.types.last() else {
            return false;
        };
        let local = self
            .scopes
            .iter()
            .skip(ctx.depth)
            .any(|s| s.contains_key(name));
        !local && ctx.ty.member(name).is_some()
    }

    /// Turns `x` into `this.x` when `x` is a member of `this` (or into
    /// `Type.x` for a static member).
    fn qualify(&self, e: &mut Expr) {
        if let ExprKind::Ident(name) = &e.kind {
            if self.is_member_of_this(name) {
                let name = name.clone();
                let is_static = self
                    .types
                    .last()
                    .and_then(|c| c.ty.member(&name))
                    .is_some_and(|(_, m)| m.is_static());
                let object = if is_static {
                    ExprKind::SelfType
                } else {
                    ExprKind::This
                };
                e.kind = ExprKind::Member {
                    object: Box::new(Expr::new(object, e.span)),
                    name,
                };
            }
        }
    }

    /// The type of `object.name`; `call` allows methods.
    fn member_type(&mut self, obj: &Ty, name: &str, span: Span, call: bool) -> Ty {
        if obj.is_any() {
            return Ty::Any;
        }
        let Some((t, owner, m)) = self.find_member(obj, name, span) else {
            return Ty::Any;
        };
        match m {
            MemberInfo::Variant(_) => Ty::Named(t),
            MemberInfo::Field(f) => f.ty,
            MemberInfo::Prop(p) => {
                match p.get {
                    Some(access) => {
                        self.check_access(&owner, access, &format!("the get of '{name}'"), span)
                    }
                    None => self.type_error(format!("property '{name}' has no get"), span),
                }
                p.ty
            }
            MemberInfo::Method(m) if call => Ty::Fn(Some(m.sig)),
            MemberInfo::Method(_) => {
                self.type_error(
                    format!("method '{name}' can only be called: {name}(...)"),
                    span,
                );
                Ty::Any
            }
        }
    }

    /// The `this` of the method being checked.
    fn this_type(&mut self, span: Span) -> Option<Ty> {
        match self.types.last() {
            Some(ctx) if ctx.is_static => {
                self.type_error(
                    "'this' (or an instance member) cannot be used in a static member",
                    span,
                );
                None
            }
            Some(ctx) => Some(Ty::Named(ctx.ty.clone())),
            None => {
                self.type_error("'this' can only be used in methods and init", span);
                None
            }
        }
    }

    fn check_block(&mut self, block: &mut Block) {
        self.with_scope(|c| c.check_stmts(&mut block.stmts));
    }

    fn check_stmt(&mut self, stmt: &mut Stmt) {
        let span = stmt.span;
        match &mut stmt.kind {
            // Resolved by the project loader before checking.
            StmtKind::Using(_) => {}
            StmtKind::Expr(e) => {
                self.expr(e);
            }
            StmtKind::VarDecl(d) => {
                self.expr(&mut d.value);
                let name = d.name.clone();
                d.resolved = match &d.ty {
                    Some(t) => {
                        let ty = self.resolve(t);
                        self.coerce(&mut d.value, &ty, &|| format!("'{name}'"));
                        ty
                    }
                    // `var x = null` has no useful type.
                    None if matches!(d.value.ty, Ty::Null) => Ty::Any,
                    None if matches!(d.value.ty, Ty::Type(_)) => {
                        self.type_error(
                            format!(
                                "'{}' is a type, not a value; call it to make one",
                                d.value.ty.to_string().trim_start_matches("type ")
                            ),
                            d.value.span,
                        );
                        Ty::Any
                    }
                    None => d.value.ty.clone(),
                };
                self.declare(&d.name, d.resolved.clone(), span);
            }
            StmtKind::Idb { value, .. } => {
                self.expr(value);
            }
            StmtKind::Assign { target, value } => {
                self.expr(value);
                if let Some(ty) = self.place(target) {
                    let what = describe_place(target);
                    self.coerce(value, &ty, &|| what.clone());
                }
            }
            StmtKind::CompoundAssign {
                op,
                target,
                value,
                cast,
            } => {
                let target_ty = self.place(target);
                self.expr(value);
                let Some(target_ty) = target_ty else {
                    return;
                };
                // `x op= y` is computed as `x op y` and converted back to the
                // type of `x`, which may narrow (`byte b; b += 1;`) as in C#.
                let mut lhs = Expr::new(ExprKind::Null, target.span);
                lhs.ty = target_ty.clone();
                let result = self.binary(*op, &mut lhs, value, span);
                *cast = target_ty.clone();
                let narrowing_ok = is_integer(&result) && is_integer(&target_ty);
                if !narrowing_ok && matches!(conversion(&result, &target_ty, None), Conv::No) {
                    self.type_error(
                        format!(
                            "cannot store the {result} result of '{}=' in {} of type {target_ty}",
                            op.symbol(),
                            describe_place(target)
                        ),
                        span,
                    );
                }
            }
            StmtKind::FnDecl(decl) => match Rc::get_mut(decl) {
                Some(decl) => self.check_fn(decl),
                None => self.type_error("internal error: function is shared before checking", span),
            },
            StmtKind::Type(decl) => match Rc::get_mut(decl) {
                Some(decl) => self.check_type(decl),
                None => self.type_error("internal error: type is shared before checking", span),
            },
            StmtKind::Return(value) => {
                let Some(ctx) = self.fns.last() else {
                    return;
                };
                let (ret, name) = (ctx.ret.clone(), ctx.name.clone());
                match value {
                    Some(e) => {
                        self.expr(e);
                        self.coerce(e, &ret, &|| format!("the return value of '{name}'"));
                    }
                    None if !ret.nullable() => {
                        self.type_error(format!("'{name}' must return a value of type {ret}"), span)
                    }
                    None => {}
                }
            }
            StmtKind::Break | StmtKind::Continue => {}
            StmtKind::Block(b) => self.check_block(b),
            StmtKind::If {
                cond,
                then,
                otherwise,
            } => {
                self.expr(cond);
                self.check_block(then);
                if let Some(other) = otherwise {
                    self.check_stmt(other);
                }
            }
            StmtKind::While { cond, body } => {
                self.expr(cond);
                self.check_block(body);
            }
            StmtKind::For {
                var,
                var_ty,
                iter,
                body,
            } => {
                let iter_ty = self.expr(iter);
                *var_ty = match iter_ty {
                    Ty::String | Ty::Ster => Ty::Char,
                    Ty::Array(elem) => (*elem).clone(),
                    // Iterating a dictionary yields its keys.
                    Ty::Map(key, _) => (*key).clone(),
                    Ty::Any => Ty::Any,
                    other => {
                        self.type_error(format!("cannot iterate over {other}"), iter.span);
                        Ty::Any
                    }
                };
                let (var, var_ty) = (var.clone(), var_ty.clone());
                self.with_scope(|c| {
                    c.declare(&var, var_ty, span);
                    c.check_stmts(&mut body.stmts);
                });
            }
        }
    }

    /// `main` is where a program starts: `fn main()`, optionally with
    /// `args: string[]` and an `int` exit code.
    fn check_main(&mut self, decl: &FnDecl) {
        let args_ok = match decl.params.as_slice() {
            [] => true,
            [p] => matches!(&p.resolved, Ty::Array(e) if matches!(**e, Ty::String | Ty::Any)),
            _ => false,
        };
        let ret_ok = matches!(decl.ret_resolved, Ty::Any | Ty::Int(_));
        if !args_ok || !ret_ok || !decl.type_params.is_empty() {
            self.type_error(
                "`main` must be declared as `fn main()`, `fn main(args: string[])`, optionally returning an int exit code",
                decl.span,
            );
        }
    }

    fn check_fn(&mut self, decl: &mut FnDecl) {
        self.type_params.push(
            decl.type_params
                .iter()
                .map(|t| Rc::from(t.as_str()))
                .collect(),
        );
        self.fns.push(FnCtx {
            name: decl.name.clone(),
            ret: decl.ret_resolved.clone(),
        });
        let params: Vec<(String, Ty, Span)> = decl
            .params
            .iter()
            .map(|p| (p.name.clone(), p.resolved.clone(), p.span))
            .collect();
        self.with_scope(|c| {
            for (name, ty, span) in params {
                c.declare(&name, ty, span);
            }
            c.check_stmts(&mut decl.body.stmts);
        });
        self.fns.pop();
        self.type_params.pop();
        let ret = &decl.ret_resolved;
        if !ret.nullable() && !always_returns(&decl.body.stmts) {
            self.type_error(
                format!(
                    "not all code paths of '{}' return a value of type {ret}",
                    decl.name
                ),
                decl.span,
            );
        }
    }

    /// Checks an assignment target and returns the type it holds.
    fn place(&mut self, target: &mut Expr) -> Option<Ty> {
        let span = target.span;
        self.qualify(target);
        let ty = match &mut target.kind {
            ExprKind::Ident(name) => {
                match self.lookup(name).cloned() {
                    Some(_) if self.is_import(name) => {
                        let module = self.imports.get(name.as_str()).cloned().unwrap_or_default();
                        let msg = format!("'{name}' is imported from module '{module}' and cannot be assigned here");
                        self.type_error(msg, span);
                        None
                    }
                    Some(ty) => Some(ty),
                    None => {
                        let msg = format!("undefined variable '{name}'");
                        self.type_error(msg, span);
                        None
                    }
                }
            }
            ExprKind::Index { object, index } => {
                let obj = self.expr(object);
                if matches!(obj, Ty::String | Ty::Ster) {
                    self.expr(index);
                    self.type_error("strings are immutable; build a new string instead", span);
                    None
                } else {
                    self.index(&obj, index, span)
                }
            }
            ExprKind::Member { object, name } => {
                let name = name.clone();
                let obj = self.expr(object);
                if obj.is_any() {
                    return Some(Ty::Any);
                }
                let temporary = !object.is_place()
                    && !matches!(object.kind, ExprKind::SelfType)
                    && matches!(&obj, Ty::Named(t) if t.kind == TypeKind::Struct);
                let in_init = self
                    .types
                    .last()
                    .filter(|c| c.in_init)
                    .map(|c| c.ty.clone());
                match self.find_member(&obj, &name, span) {
                    None => None,
                    Some((t, owner, m)) => {
                        let (ty, set) = match m {
                            MemberInfo::Field(f) => {
                                if f.set.is_none() && in_init.as_ref() != Some(&owner) {
                                    self.type_error(
                                        format!("'{name}' is read-only; it is set only by its value and by init"),
                                        span,
                                    );
                                }
                                (f.ty, f.set)
                            }
                            MemberInfo::Prop(p) => {
                                if p.set.is_none() {
                                    self.type_error(format!("property '{name}' has no set"), span);
                                }
                                (p.ty, p.set)
                            }
                            _ => {
                                self.type_error(
                                    format!(
                                        "cannot assign to '{name}': it is not a field or property"
                                    ),
                                    span,
                                );
                                return None;
                            }
                        };
                        if let Some(access) = set {
                            self.check_access(&owner, access, &format!("setting '{name}'"), span);
                        }
                        if temporary {
                            self.type_error(
                                format!("cannot change a field of a temporary {} value; store it in a variable first", t.name),
                                span,
                            );
                        }
                        Some(ty)
                    }
                }
            }
            ExprKind::This => match self.types.last() {
                Some(ctx) if ctx.ty.kind == TypeKind::Struct && !ctx.is_static => {
                    Some(Ty::Named(ctx.ty.clone()))
                }
                Some(ctx) if !ctx.is_static => {
                    self.type_error(
                        format!("'this' cannot be replaced in a {}", ctx.ty.kind.keyword()),
                        span,
                    );
                    None
                }
                _ => {
                    self.this_type(span);
                    None
                }
            },
            _ => {
                self.type_error("invalid assignment target", span);
                None
            }
        };
        if let Some(ty) = &ty {
            target.ty = ty.clone();
        }
        ty
    }

    // ---- expressions ---------------------------------------------------

    /// Infers the type of `e`, records it in `e.ty` and returns it.
    fn expr(&mut self, e: &mut Expr) -> Ty {
        let ty = with_stack(|| self.expr_inner(e));
        // Binary operators may have wrapped `e`'s operands, but never `e`
        // itself, so recording the type here is always right.
        e.ty = ty.clone();
        ty
    }

    fn expr_inner(&mut self, e: &mut Expr) -> Ty {
        let span = e.span;
        self.qualify(e);
        match &mut e.kind {
            ExprKind::Int(_, k) => Ty::Int(*k),
            ExprKind::Float(_, k) => Ty::Float(*k),
            ExprKind::String(_) => Ty::String,
            ExprKind::Bool(_) => Ty::Bool,
            ExprKind::Null => Ty::Null,
            ExprKind::Convert { expr, to } => {
                self.expr(expr);
                to.clone()
            }
            ExprKind::Ident(name) => match self.lookup(name) {
                Some(ty) => ty.clone(),
                None if builtins::is_builtin(name) => {
                    let msg = format!("built-in '{name}' can only be called, not used as a value");
                    self.type_error(msg, span);
                    Ty::Any
                }
                None => {
                    let msg = format!("undefined variable '{name}'");
                    self.type_error(msg, span);
                    Ty::Any
                }
            },
            ExprKind::Array(items) => {
                for item in items.iter_mut() {
                    self.expr(item);
                }
                Ty::array(self.common_type(items.iter_mut(), "array elements"))
            }
            ExprKind::Map(entries) => {
                for (k, v) in entries.iter_mut() {
                    self.expr(k);
                    self.expr(v);
                }
                let key = self.common_type(entries.iter_mut().map(|(k, _)| k), "dictionary keys");
                let value =
                    self.common_type(entries.iter_mut().map(|(_, v)| v), "dictionary values");
                if !key.is_key() {
                    self.type_error(
                        format!("{key} cannot be a dictionary key; use string, an integer type, char or bool"),
                        span,
                    );
                }
                Ty::map(key, value)
            }
            ExprKind::Unary { op, expr } => {
                let ty = self.expr(expr);
                self.unary(*op, &ty, span)
            }
            ExprKind::Binary { op, lhs, rhs } => {
                self.expr(lhs);
                self.expr(rhs);
                self.binary(*op, lhs, rhs, span)
            }
            ExprKind::Call {
                callee,
                type_args,
                args,
                inst,
            } => {
                let explicit: Vec<(Ty, Span)> = type_args
                    .iter()
                    .map(|t| (self.resolve(t), t.span))
                    .collect();
                self.call(callee, &explicit, args, inst, span)
            }
            ExprKind::Member { object, name } => {
                let name = name.clone();
                let obj = self.expr(object);
                self.member_type(&obj, &name, span, false)
            }
            ExprKind::This => self.this_type(span).unwrap_or(Ty::Any),
            ExprKind::Base => {
                let base = self
                    .types
                    .last()
                    .filter(|c| !c.is_static)
                    .and_then(|c| c.ty.base());
                match base {
                    Some(b) => Ty::Named(b),
                    None => {
                        self.type_error(
                            "'base' can only be used in methods of a class with a base class",
                            span,
                        );
                        Ty::Any
                    }
                }
            }
            ExprKind::Is { expr, ty, resolved } => {
                self.expr(expr);
                *resolved = self.resolve(ty);
                Ty::Bool
            }
            ExprKind::As { expr, ty, resolved } => {
                self.expr(expr);
                let to = self.resolve(ty);
                if !to.nullable() || matches!(to, Ty::Null) {
                    self.type_error(
                        format!("'as' needs a type that can be null, like a class or interface; use 'is' to test for {to}"),
                        span,
                    );
                }
                *resolved = to.clone();
                to
            }
            ExprKind::SelfType => match self.types.last() {
                Some(ctx) => Ty::Type(ctx.ty.clone()),
                None => Ty::Any,
            },
            ExprKind::Index { object, index } => {
                let obj = self.expr(object);
                self.index(&obj, index, span).unwrap_or(Ty::Any)
            }
        }
    }

    /// Checks `obj[index]` and returns the element type.
    fn index(&mut self, obj: &Ty, index: &mut Expr, span: Span) -> Option<Ty> {
        let idx = self.expr(index);
        if let Ty::Map(key, value) = obj {
            self.coerce(index, key, &|| "the dictionary key".to_string());
            return Some((**value).clone());
        }
        if !idx.is_any() && !matches!(idx, Ty::Int(_)) {
            self.type_error(format!("index must be an integer, got {idx}"), index.span);
        }
        match obj {
            Ty::Array(elem) => Some((**elem).clone()),
            Ty::Any => Some(Ty::Any),
            Ty::String | Ty::Ster => Some(Ty::Char),
            other => {
                self.type_error(format!("cannot index into {other}"), span);
                None
            }
        }
    }

    /// The type all of `items` can be converted to (the "best common type"
    /// of C#), converting them. `any` if there is none or `items` is empty.
    fn common_type<'a>(&mut self, items: impl Iterator<Item = &'a mut Expr>, what: &str) -> Ty {
        let mut items: Vec<&mut Expr> = items.collect();
        let mut common: Option<Ty> = None;
        for item in items.iter() {
            let ty = &item.ty;
            if matches!(ty, Ty::Null) {
                continue;
            }
            let Some(c) = &common else {
                common = Some(ty.clone());
                continue;
            };
            let cnst = const_of(item);
            if c == ty || !matches!(conversion(ty, c, cnst.as_ref()), Conv::No) {
                continue;
            }
            if !matches!(conversion(c, ty, None), Conv::No) {
                common = Some(ty.clone());
                continue;
            }
            if let Some(n) = arith(c)
                .zip(arith(ty))
                .and_then(|(a, b)| NumTy::promote(a, b))
            {
                common = Some(n.ty());
                continue;
            }
            // Mixed types (`[1, "two"]`) make an `any` collection.
            return Ty::Any;
        }
        let Some(common) = common else {
            return Ty::Any;
        };
        for item in items.iter_mut() {
            self.coerce(item, &common, &|| what.to_string());
        }
        common
    }

    fn unary(&mut self, op: UnaryOp, ty: &Ty, span: Span) -> Ty {
        if ty.is_any() {
            return if op == UnaryOp::Not {
                Ty::Bool
            } else {
                Ty::Any
            };
        }
        match op {
            UnaryOp::Not => Ty::Bool,
            UnaryOp::Neg => match arith(ty).map(NumTy::negated) {
                Some(Some(n)) => n.ty(),
                Some(None) => {
                    self.type_error(
                        format!("cannot negate {ty}: the result would not fit"),
                        span,
                    );
                    Ty::Any
                }
                None => {
                    self.type_error(format!("cannot negate {ty}"), span);
                    Ty::Any
                }
            },
            UnaryOp::BitNot => match arith(ty) {
                Some(NumTy::Int(k)) => Ty::Int(NumTy::int_promoted(k)),
                _ => {
                    self.type_error(format!("cannot apply '~' to {ty}"), span);
                    Ty::Any
                }
            },
        }
    }

    /// Types a binary operation and converts both operands to a common
    /// type where needed.
    fn binary(&mut self, op: BinOp, lhs: &mut Expr, rhs: &mut Expr, span: Span) -> Ty {
        let (l, r) = (lhs.ty.clone(), rhs.ty.clone());
        let mismatch = |c: &mut Self| {
            c.type_error(
                format!("cannot apply '{}' to {l} and {r}", op.symbol()),
                span,
            );
            Ty::Any
        };
        let any = l.is_any() || r.is_any();
        let relational = matches!(op, BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge);
        let bitwise = matches!(op, BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor);
        match op {
            BinOp::And | BinOp::Or => Ty::Bool,
            BinOp::Eq | BinOp::Ne => {
                if let (Some(a), Some(b)) = (arith(&l), arith(&r)) {
                    if !matches!((&l, &r), (Ty::Char, Ty::Char)) {
                        self.promote_operands(a, b, lhs, rhs);
                    }
                }
                Ty::Bool
            }
            BinOp::Add if is_stringy(&l) || is_stringy(&r) => Ty::String,
            // Two chars concatenate (`s[0] + s[1]`), as in older Belsk2.
            BinOp::Add if matches!((&l, &r), (Ty::Char, Ty::Char)) => Ty::String,
            _ if any => {
                // Checked at run time; reject only what can never work.
                let known = if l.is_any() { &r } else { &l };
                let ok = known.is_any()
                    || arith(known).is_some()
                    || (op == BinOp::Add && is_stringy(known))
                    || (bitwise && matches!(known, Ty::Bool))
                    || (relational && is_stringy(known));
                if !ok {
                    return mismatch(self);
                }
                if relational {
                    Ty::Bool
                } else {
                    Ty::Any
                }
            }
            _ if relational => {
                if (is_stringy(&l) && is_stringy(&r)) || matches!((&l, &r), (Ty::Char, Ty::Char)) {
                    return Ty::Bool;
                }
                let (Some(a), Some(b)) = (arith(&l), arith(&r)) else {
                    return mismatch(self);
                };
                if self.promote_operands(a, b, lhs, rhs).is_none() {
                    return mismatch(self);
                }
                Ty::Bool
            }
            _ if bitwise && matches!((&l, &r), (Ty::Bool, Ty::Bool)) => Ty::Bool,
            BinOp::Shl | BinOp::Shr => {
                let Some(NumTy::Int(k)) = arith(&l) else {
                    return mismatch(self);
                };
                if !matches!(arith(&r), Some(n) if n.widens_to(NumTy::Int(IntKind::I32))) {
                    self.type_error(format!("shift count must be an int, got {r}"), rhs.span);
                    return Ty::Any;
                }
                let result = Ty::Int(NumTy::int_promoted(k));
                if l != result {
                    insert_convert(lhs, &result);
                }
                if r != Ty::INT {
                    insert_convert(rhs, &Ty::INT);
                }
                result
            }
            _ => {
                let (Some(a), Some(b)) = (arith(&l), arith(&r)) else {
                    return mismatch(self);
                };
                let Some(common) = self.promote_operands(a, b, lhs, rhs) else {
                    return mismatch(self);
                };
                if bitwise && !matches!(common, NumTy::Int(_)) {
                    return mismatch(self);
                }
                if matches!(op, BinOp::Div | BinOp::Rem)
                    && matches!(common, NumTy::Int(_))
                    && const_of(rhs) == Some(Const::Int(0))
                {
                    self.type_error("division by zero", span);
                }
                common.ty()
            }
        }
    }

    /// Converts both operands to their common numeric type.
    fn promote_operands(
        &mut self,
        a: NumTy,
        b: NumTy,
        lhs: &mut Expr,
        rhs: &mut Expr,
    ) -> Option<NumTy> {
        let common = NumTy::promote(a, b)?;
        let ty = common.ty();
        if lhs.ty != ty {
            insert_convert(lhs, &ty);
        }
        if rhs.ty != ty {
            insert_convert(rhs, &ty);
        }
        Some(common)
    }

    fn call(
        &mut self,
        callee: &mut Expr,
        type_args: &[(Ty, Span)],
        args: &mut [Expr],
        inst: &mut Vec<Ty>,
        span: Span,
    ) -> Ty {
        for a in args.iter_mut() {
            self.expr(a);
        }

        // Built-ins take precedence over user definitions (as at run time).
        if let ExprKind::Ident(name) = &callee.kind {
            if builtins::is_builtin(name) {
                if let Some((_, tspan)) = type_args.first() {
                    let msg = format!("built-in '{name}' does not take type arguments");
                    self.type_error(msg, *tspan);
                }
                let name = name.clone();
                return match builtins::check(&name, args) {
                    Ok(ty) => {
                        self.builtin_coercions(&name, args);
                        ty
                    }
                    Err(e) => {
                        self.error(e.or_at(span));
                        Ty::Any
                    }
                };
            }
        }

        // `base(...)` at the start of `init` runs the base class's init.
        if matches!(callee.kind, ExprKind::Base) {
            let ctx = self.types.last().filter(|c| c.in_init);
            let Some(b) = ctx.and_then(|c| c.ty.base()) else {
                self.type_error("`: base(...)` needs a class with a base class", span);
                return Ty::Any;
            };
            let access = b.with_body(|x| x.ctor_access).unwrap_or_default();
            self.check_access(&b, access, &format!("the init of {}", b.name), span);
            let params = b.with_body(|x| x.ctor.clone()).unwrap_or_default();
            if params.len() != args.len() {
                self.type_error(
                    format!(
                        "the init of {} expects {} argument(s), got {}",
                        b.name,
                        params.len(),
                        args.len()
                    ),
                    span,
                );
            }
            for (i, ((_, ty), arg)) in params.iter().zip(args.iter_mut()).enumerate() {
                let name = b.name.clone();
                self.coerce(arg, ty, &|| {
                    format!("argument {} of the init of {name}", i + 1)
                });
            }
            return Ty::Null;
        }
        self.qualify(callee);
        let callee_ty = match &mut callee.kind {
            ExprKind::Member { object, name } => {
                let name = name.clone();
                let obj = self.expr(object);
                if matches!(object.kind, ExprKind::Base) {
                    if let Some((_, MemberInfo::Method(m))) =
                        obj.named().and_then(|t| t.member(&name))
                    {
                        if m.virt == Virt::Abstract {
                            self.type_error(
                                format!(
                                    "'{name}' is abstract in the base class and cannot be called"
                                ),
                                callee.span,
                            );
                        }
                    }
                }
                let ty = self.member_type(&obj, &name, callee.span, true);
                callee.ty = ty.clone();
                ty
            }
            _ => self.expr(callee),
        };
        // `Point(1, 2)` calls the constructor.
        let callee_ty = match callee_ty {
            Ty::Type(t) if t.kind == TypeKind::Enum => {
                self.type_error(
                    format!(
                        "an enum has no constructor; use one of its cases, like {}.{}",
                        t.name,
                        t.variants()
                            .first()
                            .map(|v| v.name.as_str())
                            .unwrap_or("Case")
                    ),
                    callee.span,
                );
                Ty::Any
            }
            Ty::Type(t) if t.kind == TypeKind::Interface || t.is_abstract() => {
                let what = if t.kind == TypeKind::Interface {
                    "an interface"
                } else {
                    "an abstract class"
                };
                self.type_error(
                    format!(
                        "{} is {what}; create a class that implements it instead",
                        t.name
                    ),
                    callee.span,
                );
                Ty::Any
            }
            Ty::Type(t) => {
                let access = t.with_body(|b| b.ctor_access).unwrap_or_default();
                self.check_access(&t, access, &format!("the init of {}", t.name), callee.span);
                let params = t.with_body(|b| b.ctor.clone()).unwrap_or_default();
                Ty::Fn(Some(Rc::new(Sig {
                    name: t.name.to_string(),
                    type_params: Vec::new(),
                    params: params.into_iter().map(|(_, ty)| ty).collect(),
                    ret: Ty::Named(t),
                })))
            }
            other => other,
        };
        match callee_ty {
            Ty::Fn(Some(sig)) => {
                let n = sig.params.len();
                if args.len() != n {
                    self.type_error(
                        format!(
                            "function '{}' expects {n} argument{}, got {}",
                            sig.name,
                            if n == 1 { "" } else { "s" },
                            args.len()
                        ),
                        span,
                    );
                }
                let mut bound: HashMap<Rc<str>, Ty> = HashMap::new();
                if type_args.is_empty() {
                    // Infer type arguments of a generic function from the
                    // arguments: `first([1, 2])` makes `T` = `int`.
                    for (param, arg) in sig.params.iter().zip(args.iter()) {
                        bind_type_params(&sig.type_params, param, &arg.ty, &mut bound);
                    }
                } else if type_args.len() != sig.type_params.len() {
                    let n = sig.type_params.len();
                    let msg = if n == 0 {
                        format!("function '{}' is not generic", sig.name)
                    } else {
                        format!(
                            "function '{}' takes {n} type argument{}, got {}",
                            sig.name,
                            if n == 1 { "" } else { "s" },
                            type_args.len()
                        )
                    };
                    self.type_error(msg, span);
                } else {
                    for (p, (t, _)) in sig.type_params.iter().zip(type_args) {
                        bound.insert(p.clone(), t.clone());
                    }
                }
                *inst = sig
                    .type_params
                    .iter()
                    .map(|p| bound.get(p).cloned().unwrap_or(Ty::Any))
                    .collect();
                let subst = |name: &str| {
                    sig.type_params
                        .iter()
                        .any(|p| &**p == name)
                        .then(|| bound.get(name).cloned().unwrap_or(Ty::Any))
                };
                for (i, (param, arg)) in sig.params.iter().zip(args.iter_mut()).enumerate() {
                    let name = sig.name.clone();
                    let param = param.substitute(&subst);
                    self.coerce(arg, &param, &|| format!("argument {} of '{name}'", i + 1));
                }
                sig.ret.substitute(&subst)
            }
            Ty::Fn(None) | Ty::Any => {
                if let Some((_, tspan)) = type_args.first() {
                    self.type_error(
                        "type arguments need a function whose declaration is known",
                        *tspan,
                    );
                }
                Ty::Any
            }
            other => {
                self.type_error(format!("{other} is not a function"), callee.span);
                Ty::Any
            }
        }
    }
}

impl Checker {
    /// Arguments of built-ins whose type depends on another argument:
    /// `push(int[], x)` converts `x` to `int`, `has(Dictionary<long, V>, k)`
    /// converts `k` to `long`.
    fn builtin_coercions(&mut self, name: &str, args: &mut [Expr]) {
        let Some((first, rest)) = args.split_first_mut() else {
            return;
        };
        let Some(arg) = rest.first_mut() else {
            return;
        };
        let target = match (name, &first.ty) {
            ("push", Ty::Array(elem)) => (**elem).clone(),
            ("has" | "remove", Ty::Map(key, _)) => (**key).clone(),
            _ => return,
        };
        self.coerce(arg, &target, &|| format!("the second argument of {name}()"));
    }
}

/// Records what the type parameters in `param` stand for, given an
/// argument of type `arg`.
fn bind_type_params(params: &[Rc<str>], param: &Ty, arg: &Ty, bound: &mut HashMap<Rc<str>, Ty>) {
    match (param, arg) {
        (Ty::Param(name), arg) if params.contains(name) => {
            if matches!(arg, Ty::Null) {
                return;
            }
            match bound.get(name) {
                None => {
                    bound.insert(name.clone(), arg.clone());
                }
                Some(prev) => {
                    // Widen to a type both arguments convert to.
                    if matches!(conversion(arg, prev, None), Conv::No)
                        && !matches!(conversion(prev, arg, None), Conv::No)
                    {
                        bound.insert(name.clone(), arg.clone());
                    }
                }
            }
        }
        (Ty::Array(p), Ty::Array(a)) => bind_type_params(params, p, a, bound),
        (Ty::Map(pk, pv), Ty::Map(ak, av)) => {
            bind_type_params(params, pk, ak, bound);
            bind_type_params(params, pv, av, bound);
        }
        _ => {}
    }
}

/// Every interface `t` implements: its own, its base classes', and the
/// interfaces those extend.
fn all_interfaces(t: &TypeRef) -> Vec<TypeRef> {
    let mut out: Vec<TypeRef> = Vec::new();
    let mut todo: Vec<TypeRef> = t
        .chain()
        .iter()
        .flat_map(|c| c.with_body(|b| b.interfaces.clone()).unwrap_or_default())
        .collect();
    while let Some(i) = todo.pop() {
        if out.contains(&i) || out.len() > 64 {
            continue;
        }
        todo.extend(i.with_body(|b| b.interfaces.clone()).unwrap_or_default());
        out.push(i);
    }
    out
}

/// A member of `t` or its base classes (not of interfaces), nearest
/// first, skipping abstract methods.
fn class_member(t: &TypeRef, name: &str) -> Option<MemberInfo> {
    t.chain().iter().find_map(|c| {
        c.with_body(|b| {
            if let Some(f) = b.fields.iter().find(|f| f.name == name) {
                return Some(MemberInfo::Field(f.clone()));
            }
            if let Some(p) = b.props.iter().find(|p| p.name == name) {
                return Some(MemberInfo::Prop(p.clone()));
            }
            b.methods
                .iter()
                .find(|m| m.sig.name == name && m.virt != Virt::Abstract)
                .map(|m| MemberInfo::Method(m.clone()))
        })
        .flatten()
    })
}

/// Whether struct `t` holds a `target` by value, directly or through
/// other structs.
fn contains_struct(t: &TypeRef, target: &TypeRef, depth: usize) -> bool {
    if depth > 32 {
        return false;
    }
    t.all_fields().iter().any(|f| match &f.ty {
        Ty::Named(n) if n.kind == TypeKind::Struct => {
            n == target || contains_struct(n, target, depth + 1)
        }
        _ => false,
    })
}

fn describe_place(target: &Expr) -> String {
    match &target.kind {
        ExprKind::Ident(name) => format!("'{name}'"),
        ExprKind::Member { name, .. } => format!("field '{name}'"),
        _ => "the element".to_string(),
    }
}
