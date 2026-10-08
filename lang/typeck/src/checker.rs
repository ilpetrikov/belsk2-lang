use std::collections::HashMap;
use std::rc::Rc;

use belsk2_syntax::ast::*;
use belsk2_syntax::{BType, Error, Span};

use crate::builtins;
use crate::ty::{Sig, Ty};

#[derive(Debug, Clone)]
struct Var {
    ty: Ty,
    /// The type written in the declaration, if any. Needed for `bel`/`ster`
    /// rules that [`Ty`] does not distinguish.
    declared: Option<BType>,
}

struct FnCtx {
    name: String,
    ret: Option<BType>,
}

/// Walks a program, infers types and collects every error it finds.
pub struct Checker {
    scopes: Vec<HashMap<String, Var>>,
    fns: Vec<FnCtx>,
    errors: Vec<Error>,
}

/// Grows the stack on demand so deep (but bounded) recursion never overflows.
fn with_stack<T>(f: impl FnOnce() -> T) -> T {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, f)
}

/// A number known at compile time (`5`, `-3.5`).
fn const_number(e: &Expr) -> Option<f64> {
    match &e.kind {
        ExprKind::Number(n) => Some(*n),
        ExprKind::Unary {
            op: UnaryOp::Neg,
            expr,
        } => match &expr.kind {
            ExprKind::Number(n) => Some(-n),
            _ => None,
        },
        _ => None,
    }
}

pub(crate) fn sig_of(decl: &FnDecl) -> Sig {
    Sig {
        name: decl.name.clone(),
        params: decl
            .params
            .iter()
            .map(|p| p.ty.map(Ty::from_btype).unwrap_or(Ty::Any))
            .collect(),
        ret: decl.ret.map(Ty::from_btype).unwrap_or(Ty::Any),
    }
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
            fns: Vec::new(),
            errors: Vec::new(),
        }
    }

    /// Makes a name known before checking (host-defined globals, or
    /// declarations from earlier REPL input).
    pub fn declare_global(&mut self, name: &str, ty: Ty) {
        if let Some(outer) = self.scopes.first_mut() {
            outer.insert(name.to_string(), Var { ty, declared: None });
        }
    }

    /// Checks a program. Returns all errors, in source order.
    pub fn check_program(mut self, program: &Program) -> Vec<Error> {
        // The program gets its own scope so it may shadow predeclared globals.
        self.scopes.push(HashMap::new());
        self.check_stmts(&program.stmts);
        self.errors.sort_by_key(|e| e.span);
        self.errors
    }

    fn error(&mut self, e: Error) {
        self.errors.push(e);
    }

    fn lookup(&self, name: &str) -> Option<&Var> {
        self.scopes.iter().rev().find_map(|s| s.get(name))
    }

    fn declare(&mut self, name: &str, var: Var, span: Span) {
        let Some(scope) = self.scopes.last_mut() else {
            return;
        };
        if scope.contains_key(name) {
            self.error(
                Error::type_error(format!("'{name}' is already declared in this scope")).at(span),
            );
            return;
        }
        scope.insert(name.to_string(), var);
    }

    fn with_scope(&mut self, f: impl FnOnce(&mut Self)) {
        self.scopes.push(HashMap::new());
        f(self);
        self.scopes.pop();
    }

    // ---- statements ----------------------------------------------------

    /// Functions are visible in their whole block, so they are declared
    /// before any statement is checked.
    fn check_stmts(&mut self, stmts: &[Stmt]) {
        for stmt in stmts {
            if let StmtKind::FnDecl(decl) = &stmt.kind {
                let var = Var {
                    ty: Ty::Fn(Some(Rc::new(sig_of(decl)))),
                    declared: Some(BType::Fn),
                };
                self.declare(&decl.name, var, stmt.span);
            }
        }
        for stmt in stmts {
            with_stack(|| self.check_stmt(stmt));
        }
    }

    fn check_block(&mut self, block: &Block) {
        self.with_scope(|c| c.check_stmts(&block.stmts));
    }

    fn check_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Expr(e) => {
                self.expr(e);
            }
            StmtKind::VarDecl(d) => {
                let value_ty = self.expr(&d.value);
                let ty = match d.ty {
                    Some(b) => {
                        self.check_store(Some(b), &Ty::from_btype(b), &d.value, &value_ty, &d.name);
                        Ty::from_btype(b)
                    }
                    // `var x = null` has no useful type.
                    None if matches!(value_ty, Ty::Null) => Ty::Any,
                    None => value_ty,
                };
                self.declare(&d.name, Var { ty, declared: d.ty }, stmt.span);
            }
            StmtKind::Idb { value, .. } => {
                self.expr(value);
            }
            StmtKind::Assign { target, value } => {
                let value_ty = self.expr(value);
                self.check_assign(target, value, &value_ty);
            }
            StmtKind::CompoundAssign { op, target, value } => {
                let target_ty = self.expr(target);
                let value_ty = self.expr(value);
                let result = self.binary(*op, &target_ty, &value_ty, value, stmt.span);
                self.check_assign(target, value, &result);
            }
            StmtKind::FnDecl(decl) => self.check_fn(decl),
            StmtKind::Return(value) => {
                let ty = match value {
                    Some(e) => self.expr(e),
                    None => Ty::Null,
                };
                let Some(ctx) = self.fns.last() else {
                    return;
                };
                if let (Some(ret), Some(e)) = (ctx.ret, value) {
                    let name = format!("return value of '{}'", ctx.name);
                    self.check_store(Some(ret), &Ty::from_btype(ret), e, &ty, &name);
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
            StmtKind::For { var, iter, body } => {
                let iter_ty = self.expr(iter);
                let item = match iter_ty {
                    Ty::String => Ty::String,
                    Ty::Array | Ty::Any => Ty::Any,
                    other => {
                        self.error(
                            Error::type_error(format!("cannot iterate over {other}")).at(iter.span),
                        );
                        Ty::Any
                    }
                };
                self.with_scope(|c| {
                    c.declare(
                        var,
                        Var {
                            ty: item,
                            declared: None,
                        },
                        stmt.span,
                    );
                    c.check_stmts(&body.stmts);
                });
            }
        }
    }

    fn check_fn(&mut self, decl: &FnDecl) {
        self.fns.push(FnCtx {
            name: decl.name.clone(),
            ret: decl.ret,
        });
        self.with_scope(|c| {
            for p in &decl.params {
                let var = Var {
                    ty: p.ty.map(Ty::from_btype).unwrap_or(Ty::Any),
                    declared: p.ty,
                };
                c.declare(&p.name, var, p.span);
            }
            c.check_stmts(&decl.body.stmts);
        });
        self.fns.pop();
    }

    /// Reports an error if `value` (of type `value_ty`) cannot be stored in
    /// a place of type `ty` / declared type `declared`.
    fn check_store(
        &mut self,
        declared: Option<BType>,
        ty: &Ty,
        value: &Expr,
        value_ty: &Ty,
        name: &str,
    ) {
        let shown = declared.map(BType::name).unwrap_or(ty.name());
        if matches!(declared, Some(BType::Bel | BType::Ster)) && matches!(value_ty, Ty::Null) {
            self.error(
                Error::type_error(format!("'{name}' of type {shown} cannot be null"))
                    .at(value.span),
            );
            return;
        }
        if !ty.accepts(value_ty) {
            self.error(
                Error::type_error(format!(
                    "cannot store {value_ty} value in '{name}' of type {shown}"
                ))
                .at(value.span),
            );
            return;
        }
        if declared == Some(BType::Bel) {
            if let Some(n) = const_number(value).filter(|n| *n > 1000.0) {
                self.error(
                    Error::type_error(format!(
                        "bel value {n} exceeds the maximum of 1000 for '{name}'"
                    ))
                    .at(value.span),
                );
            }
        }
    }

    fn check_assign(&mut self, target: &Expr, value: &Expr, value_ty: &Ty) {
        match &target.kind {
            ExprKind::Ident(name) => match self.lookup(name).cloned() {
                Some(var) => self.check_store(var.declared, &var.ty, value, value_ty, name),
                None => self.error(
                    Error::type_error(format!("undefined variable '{name}'")).at(target.span),
                ),
            },
            ExprKind::Index { object, index } => {
                let obj = self.expr(object);
                let idx = self.expr(index);
                self.check_index_ty(&idx, index.span);
                match obj {
                    Ty::Array | Ty::Any => {}
                    Ty::String => self.error(
                        Error::type_error("strings are immutable; build a new string instead")
                            .at(target.span),
                    ),
                    other => self.error(
                        Error::type_error(format!("cannot index into {other}")).at(target.span),
                    ),
                }
            }
            ExprKind::Member { object, name } => {
                let obj = self.expr(object);
                self.error(
                    Error::type_error(format!("{obj} has no member '{name}'")).at(target.span),
                );
            }
            _ => self.error(Error::type_error("invalid assignment target").at(target.span)),
        }
    }

    // ---- expressions ---------------------------------------------------

    fn expr(&mut self, e: &Expr) -> Ty {
        with_stack(|| self.expr_inner(e))
    }

    fn expr_inner(&mut self, e: &Expr) -> Ty {
        match &e.kind {
            ExprKind::Number(_) => Ty::Number,
            ExprKind::String(_) => Ty::String,
            ExprKind::Bool(_) => Ty::Bool,
            ExprKind::Null => Ty::Null,
            ExprKind::Ident(name) => match self.lookup(name) {
                Some(v) => v.ty.clone(),
                None if builtins::is_builtin(name) => {
                    self.error(
                        Error::type_error(format!(
                            "built-in '{name}' can only be called, not used as a value"
                        ))
                        .at(e.span),
                    );
                    Ty::Any
                }
                None => {
                    self.error(
                        Error::type_error(format!("undefined variable '{name}'")).at(e.span),
                    );
                    Ty::Any
                }
            },
            ExprKind::Array(items) => {
                for item in items {
                    self.expr(item);
                }
                Ty::Array
            }
            ExprKind::Unary { op, expr } => {
                let ty = self.expr(expr);
                match op {
                    UnaryOp::Not => Ty::Bool,
                    UnaryOp::Neg => {
                        if !ty.one_of(&[Ty::Number]) {
                            self.error(Error::type_error(format!("cannot negate {ty}")).at(e.span));
                        }
                        Ty::Number
                    }
                }
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let l = self.expr(lhs);
                let r = self.expr(rhs);
                self.binary(*op, &l, &r, rhs, e.span)
            }
            ExprKind::Call { callee, args } => self.call(callee, args, e.span),
            ExprKind::Member { object, name } => {
                let obj = self.expr(object);
                self.error(Error::type_error(format!("{obj} has no member '{name}'")).at(e.span));
                Ty::Any
            }
            ExprKind::Index { object, index } => {
                let obj = self.expr(object);
                let idx = self.expr(index);
                self.check_index_ty(&idx, index.span);
                match obj {
                    Ty::Array | Ty::Any => Ty::Any,
                    Ty::String => Ty::String,
                    other => {
                        self.error(
                            Error::type_error(format!("cannot index into {other}")).at(e.span),
                        );
                        Ty::Any
                    }
                }
            }
        }
    }

    fn check_index_ty(&mut self, idx: &Ty, span: Span) {
        if !idx.one_of(&[Ty::Number]) {
            self.error(Error::type_error(format!("index must be a number, got {idx}")).at(span));
        }
    }

    fn binary(&mut self, op: BinOp, l: &Ty, r: &Ty, rhs: &Expr, span: Span) -> Ty {
        let mismatch = |c: &mut Self| {
            c.error(
                Error::type_error(format!("cannot apply '{}' to {l} and {r}", op.symbol()))
                    .at(span),
            );
        };
        match op {
            BinOp::Eq | BinOp::Ne | BinOp::And | BinOp::Or => Ty::Bool,
            BinOp::Add => {
                if matches!(l, Ty::String) || matches!(r, Ty::String) {
                    Ty::String
                } else if l.is_any() || r.is_any() {
                    // Could be a number or a string at run time.
                    if l.one_of(&[Ty::Number, Ty::String]) && r.one_of(&[Ty::Number, Ty::String]) {
                        Ty::Any
                    } else {
                        mismatch(self);
                        Ty::Any
                    }
                } else if matches!((l, r), (Ty::Number, Ty::Number)) {
                    Ty::Number
                } else {
                    mismatch(self);
                    Ty::Any
                }
            }
            BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
                if !l.one_of(&[Ty::Number]) || !r.one_of(&[Ty::Number]) {
                    mismatch(self);
                } else if matches!(op, BinOp::Div | BinOp::Rem) && const_number(rhs) == Some(0.0) {
                    self.error(Error::type_error("division by zero").at(span));
                }
                Ty::Number
            }
            BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => {
                let ok = match (l, r) {
                    (Ty::Any, x) | (x, Ty::Any) => x.one_of(&[Ty::Number, Ty::String]),
                    (Ty::Number, Ty::Number) | (Ty::String, Ty::String) => true,
                    _ => false,
                };
                if !ok {
                    mismatch(self);
                }
                Ty::Bool
            }
        }
    }

    fn call(&mut self, callee: &Expr, args: &[Expr], span: Span) -> Ty {
        let arg_tys: Vec<Ty> = args.iter().map(|a| self.expr(a)).collect();

        // Built-ins take precedence over user definitions (as at run time).
        if let ExprKind::Ident(name) = &callee.kind {
            if builtins::is_builtin(name) {
                return match builtins::check(name, args, &arg_tys) {
                    Ok(ty) => ty,
                    Err(e) => {
                        self.error(e.or_at(span));
                        Ty::Any
                    }
                };
            }
        }

        match self.expr(callee) {
            Ty::Fn(Some(sig)) => {
                if args.len() != sig.params.len() {
                    let n = sig.params.len();
                    self.error(
                        Error::type_error(format!(
                            "function '{}' expects {n} argument{}, got {}",
                            sig.name,
                            if n == 1 { "" } else { "s" },
                            args.len()
                        ))
                        .at(span),
                    );
                }
                for (i, (param, (arg, arg_ty))) in
                    sig.params.iter().zip(args.iter().zip(&arg_tys)).enumerate()
                {
                    if !param.accepts(arg_ty) {
                        self.error(
                            Error::type_error(format!(
                                "argument {} of '{}' must be {param}, got {arg_ty}",
                                i + 1,
                                sig.name
                            ))
                            .at(arg.span),
                        );
                    }
                }
                sig.ret.clone()
            }
            Ty::Fn(None) | Ty::Any => Ty::Any,
            other => {
                self.error(Error::type_error(format!("{other} is not a function")).at(callee.span));
                Ty::Any
            }
        }
    }
}
