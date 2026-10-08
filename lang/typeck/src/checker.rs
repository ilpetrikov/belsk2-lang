use std::collections::HashMap;
use std::rc::Rc;

use belsk2_syntax::ast::*;
use belsk2_syntax::{Error, IntKind, NumTy, Sig, Span, Ty, TypeExpr, TypeExprKind};

use crate::builtins;

struct FnCtx {
    name: String,
    ret: Ty,
}

/// Walks a program, infers and records types, inserts implicit conversions
/// and collects every error it finds.
pub struct Checker {
    scopes: Vec<HashMap<String, Ty>>,
    /// Type parameters of the generic functions being checked.
    type_params: Vec<Vec<Rc<str>>>,
    fns: Vec<FnCtx>,
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
            type_params: Vec::new(),
            fns: Vec::new(),
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

    /// Checks a program, filling in types and implicit conversions.
    /// Returns all errors, in source order.
    pub fn check_program(mut self, program: &mut Program) -> Vec<Error> {
        // The program gets its own scope so it may shadow predeclared globals.
        self.scopes.push(HashMap::new());
        self.check_stmts(&mut program.stmts);
        self.errors.sort_by_key(|e| e.span);
        self.errors
    }

    fn error(&mut self, e: Error) {
        self.errors.push(e);
    }

    fn type_error(&mut self, msg: impl Into<String>, span: Span) {
        self.error(Error::type_error(msg).at(span));
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
        for stmt in stmts.iter_mut() {
            let span = stmt.span;
            if let StmtKind::FnDecl(decl) = &mut stmt.kind {
                let Some(decl) = Rc::get_mut(decl) else {
                    self.type_error("internal error: function is shared before checking", span);
                    continue;
                };
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

    fn check_block(&mut self, block: &mut Block) {
        self.with_scope(|c| c.check_stmts(&mut block.stmts));
    }

    fn check_stmt(&mut self, stmt: &mut Stmt) {
        let span = stmt.span;
        match &mut stmt.kind {
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
        let ty = match &mut target.kind {
            ExprKind::Ident(name) => match self.lookup(name).cloned() {
                Some(ty) => Some(ty),
                None => {
                    let msg = format!("undefined variable '{name}'");
                    self.type_error(msg, span);
                    None
                }
            },
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
                self.type_error(format!("{obj} has no member '{name}'"), span);
                None
            }
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
            ExprKind::Call { callee, args } => self.call(callee, args, span),
            ExprKind::Member { object, name } => {
                let name = name.clone();
                let obj = self.expr(object);
                self.type_error(format!("{obj} has no member '{name}'"), span);
                Ty::Any
            }
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

    fn call(&mut self, callee: &mut Expr, args: &mut [Expr], span: Span) -> Ty {
        for a in args.iter_mut() {
            self.expr(a);
        }

        // Built-ins take precedence over user definitions (as at run time).
        if let ExprKind::Ident(name) = &callee.kind {
            if builtins::is_builtin(name) {
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

        match self.expr(callee) {
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
                // Infer type arguments of a generic function from the
                // arguments: `first([1, 2])` makes `T` = `int`.
                let mut bound: HashMap<Rc<str>, Ty> = HashMap::new();
                for (param, arg) in sig.params.iter().zip(args.iter()) {
                    bind_type_params(&sig.type_params, param, &arg.ty, &mut bound);
                }
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
            Ty::Fn(None) | Ty::Any => Ty::Any,
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

fn describe_place(target: &Expr) -> String {
    match &target.kind {
        ExprKind::Ident(name) => format!("'{name}'"),
        _ => "the element".to_string(),
    }
}
