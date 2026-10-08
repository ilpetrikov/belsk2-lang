use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::rc::Rc;

use belsk2_syntax::ast::*;
use belsk2_syntax::{Error, Result, Span, Ty};
use belsk2_typeck::Checker;

use crate::builtins;
use crate::env::{self, Env, EnvRef};
use crate::num::{self, coerce, convert};
use crate::value::{Function, Value};

/// Default limit for nested function calls.
pub const DEFAULT_MAX_CALL_DEPTH: usize = 10_000;

/// Where `reab` and `input` read lines from.
pub enum Input {
    Stdin,
    Reader(Box<dyn BufRead>),
}

impl Input {
    /// Reads one line without the trailing newline. Returns `None` at end of input.
    pub(crate) fn read_line(&mut self) -> Result<Option<String>> {
        let mut line = String::new();
        let n = match self {
            Input::Stdin => std::io::stdin().read_line(&mut line)?,
            Input::Reader(r) => r.read_line(&mut line)?,
        };
        if n == 0 {
            return Ok(None);
        }
        let trimmed = line.trim_end_matches(['\r', '\n']).len();
        line.truncate(trimmed);
        Ok(Some(line))
    }
}

/// How a statement finished.
enum Flow {
    Normal,
    Return(Value),
    Break,
    Continue,
}

pub struct Interpreter {
    globals: EnvRef,
    pub(crate) id_bank: HashMap<i64, Value>,
    pub(crate) input: Input,
    call_depth: usize,
    max_call_depth: usize,
}

impl Default for Interpreter {
    fn default() -> Self {
        Self::new()
    }
}

/// Grows the stack on demand so deep (but bounded) recursion never overflows.
fn with_stack<T>(f: impl FnOnce() -> T) -> T {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, f)
}

impl Interpreter {
    pub fn new() -> Self {
        Interpreter {
            globals: Env::root(),
            id_bank: HashMap::new(),
            input: Input::Stdin,
            call_depth: 0,
            max_call_depth: DEFAULT_MAX_CALL_DEPTH,
        }
    }

    /// Reads `reab`/`input` lines from `reader` instead of stdin.
    pub fn set_input(&mut self, reader: impl BufRead + 'static) {
        self.input = Input::Reader(Box::new(reader));
    }

    pub fn set_max_call_depth(&mut self, depth: usize) {
        self.max_call_depth = depth;
    }

    /// Runs source code, printing to stdout. Globals persist between calls,
    /// so this can be used to build a REPL.
    pub fn run_source(&mut self, source: &str) -> Result<()> {
        let mut out = std::io::stdout();
        self.run_source_with_writer(source, &mut out)
    }

    /// Parses, checks and runs source code. Returns the first error; use
    /// [`Interpreter::compile`] to get all of them.
    pub fn run_source_with_writer(&mut self, source: &str, out: &mut dyn Write) -> Result<()> {
        let program = self.compile(source).map_err(|errors| {
            errors
                .into_iter()
                .next()
                .unwrap_or_else(|| Error::type_error("invalid program"))
        })?;
        self.run_program(&program, out)
    }

    /// Parses and statically checks source code. Globals that already exist
    /// in this interpreter (from earlier runs or [`Interpreter::define_global`])
    /// are visible to the checker. The returned program carries the types
    /// and conversions the checker added.
    pub fn compile(&self, source: &str) -> std::result::Result<Program, Vec<Error>> {
        let mut program = belsk2_syntax::parse(source).map_err(|e| vec![e])?;
        let mut checker = Checker::new();
        for (name, ty) in self.globals.borrow().static_types() {
            checker.declare_global(&name, ty);
        }
        let errors = checker.check_program(&mut program);
        if errors.is_empty() {
            Ok(program)
        } else {
            Err(errors)
        }
    }

    /// Runs a program produced by [`Interpreter::compile`].
    pub fn run_program(&mut self, program: &Program, out: &mut dyn Write) -> Result<()> {
        let globals = Rc::clone(&self.globals);
        self.call_depth = 0;
        hoist_functions(&program.stmts, &globals);
        for stmt in &program.stmts {
            match self.exec(stmt, &globals, out)? {
                Flow::Normal => {}
                // The parser rejects these outside of functions and loops.
                Flow::Return(_) | Flow::Break | Flow::Continue => {
                    return Err(Error::runtime("unexpected control flow at top level").at(stmt.span))
                }
            }
        }
        out.flush()?;
        Ok(())
    }

    /// Defines (or overwrites) a global variable. Unlike `=` assignment this
    /// never fails on a missing variable, which makes it suitable for
    /// injecting host-side values. Its static type is the type of `value`.
    pub fn define_global(&mut self, name: &str, value: Value) {
        self.globals.borrow_mut().define(name, value, Ty::Any);
    }

    pub fn get_global(&self, name: &str) -> Option<Value> {
        env::lookup(&self.globals, name)
    }

    /// Returns `true` when a global function with `name` is defined.
    pub fn has_function(&self, name: &str) -> bool {
        matches!(self.get_global(name), Some(Value::Function(_)))
    }

    /// Calls a global function by name and returns its result. Arguments
    /// are converted to the parameter types (`Value::from(7.0)` is accepted
    /// for an `int` parameter only if it is a whole number).
    pub fn call_function(
        &mut self,
        name: &str,
        args: &[Value],
        out: &mut dyn Write,
    ) -> Result<Value> {
        let Some(Value::Function(f)) = self.get_global(name) else {
            return Err(Error::runtime(format!("undefined function '{name}'")));
        };
        self.call_depth = 0;
        let args = args.iter().map(|a| host_arg(a.clone())).collect();
        let result = self.call(&f, args, f.decl.span, out)?;
        out.flush()?;
        Ok(result)
    }

    // ---- statements ----------------------------------------------------

    fn exec_block(&mut self, block: &Block, env: &EnvRef, out: &mut dyn Write) -> Result<Flow> {
        let scope = Env::child(env);
        hoist_functions(&block.stmts, &scope);
        for stmt in &block.stmts {
            match self.exec(stmt, &scope, out)? {
                Flow::Normal => {}
                flow => return Ok(flow),
            }
        }
        Ok(Flow::Normal)
    }

    fn exec(&mut self, stmt: &Stmt, env: &EnvRef, out: &mut dyn Write) -> Result<Flow> {
        with_stack(|| self.exec_inner(stmt, env, out)).map_err(|e| e.or_at(stmt.span))
    }

    fn exec_inner(&mut self, stmt: &Stmt, env: &EnvRef, out: &mut dyn Write) -> Result<Flow> {
        match &stmt.kind {
            StmtKind::Expr(e) => {
                self.eval(e, env, out)?;
            }
            StmtKind::VarDecl(d) => {
                let value = self.eval(&d.value, env, out)?;
                let value = coerce(&d.resolved, value, &format!("'{}'", d.name))?;
                env.borrow_mut().define(&d.name, value, d.resolved.clone());
            }
            StmtKind::Idb { id, value } => {
                let value = self.eval(value, env, out)?;
                self.id_bank.insert(*id, value);
            }
            StmtKind::Assign { target, value } => {
                let value = self.eval(value, env, out)?;
                self.assign(target, value, env, out)?;
            }
            StmtKind::CompoundAssign {
                op,
                target,
                value,
                cast,
            } => {
                self.compound_assign(*op, target, value, cast, env, out)?;
            }
            // Defined by `hoist_functions` when the block was entered.
            StmtKind::FnDecl(_) => {}
            StmtKind::Return(value) => {
                let v = match value {
                    Some(e) => self.eval(e, env, out)?,
                    None => Value::Null,
                };
                return Ok(Flow::Return(v));
            }
            StmtKind::Break => return Ok(Flow::Break),
            StmtKind::Continue => return Ok(Flow::Continue),
            StmtKind::Block(b) => return self.exec_block(b, env, out),
            StmtKind::If {
                cond,
                then,
                otherwise,
            } => {
                if self.eval(cond, env, out)?.is_truthy() {
                    return self.exec_block(then, env, out);
                }
                if let Some(other) = otherwise {
                    return self.exec(other, env, out);
                }
            }
            StmtKind::While { cond, body } => {
                while self.eval(cond, env, out)?.is_truthy() {
                    match self.exec_block(body, env, out)? {
                        Flow::Break => break,
                        Flow::Normal | Flow::Continue => {}
                        ret @ Flow::Return(_) => return Ok(ret),
                    }
                }
            }
            StmtKind::For {
                var,
                var_ty,
                iter,
                body,
            } => {
                let items: Vec<Value> = match self.eval(iter, env, out)? {
                    Value::Array(a) => a.snapshot()?,
                    Value::String(s) => s.chars().map(Value::Char).collect(),
                    other => {
                        return Err(Error::type_error(format!(
                            "cannot iterate over {}",
                            other.type_name()
                        ))
                        .at(iter.span))
                    }
                };
                for item in items {
                    let scope = Env::child(env);
                    let item = coerce(var_ty, item, &format!("'{var}'"))?;
                    scope.borrow_mut().define(var, item, var_ty.clone());
                    match self.exec_block(body, &scope, out)? {
                        Flow::Break => break,
                        Flow::Normal | Flow::Continue => {}
                        ret @ Flow::Return(_) => return Ok(ret),
                    }
                }
            }
        }
        Ok(Flow::Normal)
    }

    fn assign(
        &mut self,
        target: &Expr,
        value: Value,
        env: &EnvRef,
        out: &mut dyn Write,
    ) -> Result<()> {
        match &target.kind {
            ExprKind::Ident(name) => {
                env::assign(env, name, value).map_err(|e| e.or_at(target.span))
            }
            ExprKind::Index { object, index } => {
                let obj = self.eval(object, env, out)?;
                let idx = self.eval(index, env, out)?;
                set_index(&obj, &idx, value).map_err(|e| e.or_at(target.span))
            }
            ExprKind::Member { name, .. } => {
                Err(Error::runtime(format!("cannot assign to member '{name}'")).at(target.span))
            }
            _ => Err(Error::runtime("invalid assignment target").at(target.span)),
        }
    }

    fn compound_assign(
        &mut self,
        op: BinOp,
        target: &Expr,
        value: &Expr,
        cast: &Ty,
        env: &EnvRef,
        out: &mut dyn Write,
    ) -> Result<()> {
        let at = |e: Error| e.or_at(target.span);
        // `x op= y` converts the result back to the type of `x`, which may
        // narrow (`byte b; b += 1;`).
        let finish = |v: Value| {
            if cast.is_any() {
                Ok(v)
            } else {
                convert(v, cast)
            }
        };
        match &target.kind {
            ExprKind::Index { object, index } => {
                // Evaluate the array and index only once.
                let obj = self.eval(object, env, out)?;
                let idx = self.eval(index, env, out)?;
                let current = get_index(&obj, &idx).map_err(at)?;
                let rhs = self.eval(value, env, out)?;
                let new = binary(op, &current, &rhs).and_then(finish).map_err(at)?;
                set_index(&obj, &idx, new).map_err(at)
            }
            _ => {
                let current = self.eval(target, env, out)?;
                let rhs = self.eval(value, env, out)?;
                let new = binary(op, &current, &rhs).and_then(finish).map_err(at)?;
                self.assign(target, new, env, out)
            }
        }
    }

    // ---- expressions ---------------------------------------------------

    fn eval(&mut self, expr: &Expr, env: &EnvRef, out: &mut dyn Write) -> Result<Value> {
        with_stack(|| self.eval_inner(expr, env, out)).map_err(|e| e.or_at(expr.span))
    }

    fn eval_inner(&mut self, expr: &Expr, env: &EnvRef, out: &mut dyn Write) -> Result<Value> {
        match &expr.kind {
            ExprKind::Int(v, k) => Ok(Value::Int(*v, *k)),
            ExprKind::Float(v, k) => Ok(Value::Float(*v, *k)),
            ExprKind::String(s) => Ok(Value::String(s.clone())),
            ExprKind::Bool(b) => Ok(Value::Bool(*b)),
            ExprKind::Null => Ok(Value::Null),
            ExprKind::Ident(name) => env::lookup(env, name)
                .ok_or_else(|| Error::runtime(format!("undefined variable '{name}'"))),
            ExprKind::Convert { expr: inner, to } => {
                let v = self.eval(inner, env, out)?;
                convert(v, to)
            }
            ExprKind::Array(items) => {
                let mut values = Vec::with_capacity(items.len());
                for item in items {
                    values.push(self.eval(item, env, out)?);
                }
                Ok(Value::array(values))
            }
            ExprKind::Unary { op, expr: inner } => {
                let v = self.eval(inner, env, out)?;
                match op {
                    UnaryOp::Not => Ok(Value::Bool(!v.is_truthy())),
                    UnaryOp::Neg => num::negate(&v),
                    UnaryOp::BitNot => num::bit_not(&v),
                }
            }
            ExprKind::Binary {
                op: BinOp::And,
                lhs,
                rhs,
            } => {
                if !self.eval(lhs, env, out)?.is_truthy() {
                    return Ok(Value::Bool(false));
                }
                Ok(Value::Bool(self.eval(rhs, env, out)?.is_truthy()))
            }
            ExprKind::Binary {
                op: BinOp::Or,
                lhs,
                rhs,
            } => {
                if self.eval(lhs, env, out)?.is_truthy() {
                    return Ok(Value::Bool(true));
                }
                Ok(Value::Bool(self.eval(rhs, env, out)?.is_truthy()))
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let l = self.eval(lhs, env, out)?;
                let r = self.eval(rhs, env, out)?;
                binary(*op, &l, &r)
            }
            ExprKind::Call { callee, args } => self.eval_call(callee, args, expr.span, env, out),
            ExprKind::Member { object, name } => {
                let obj = self.eval(object, env, out)?;
                Err(Error::runtime(format!(
                    "{} has no member '{name}'",
                    obj.type_name()
                )))
            }
            ExprKind::Index { object, index } => {
                let obj = self.eval(object, env, out)?;
                let idx = self.eval(index, env, out)?;
                get_index(&obj, &idx)
            }
        }
    }

    fn eval_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        span: Span,
        env: &EnvRef,
        out: &mut dyn Write,
    ) -> Result<Value> {
        // Built-in functions take precedence over user definitions.
        if let ExprKind::Ident(name) = &callee.kind {
            if belsk2_typeck::is_builtin(name) {
                let values = self.eval_args(args, env, out)?;
                return builtins::call(self, name, values, out);
            }
        }
        let f = match self.eval(callee, env, out)? {
            Value::Function(f) => f,
            other => {
                return Err(
                    Error::type_error(format!("{} is not a function", other.type_name()))
                        .at(callee.span),
                )
            }
        };
        let values = self.eval_args(args, env, out)?;
        self.call(&f, values, span, out)
    }

    fn eval_args(
        &mut self,
        args: &[Expr],
        env: &EnvRef,
        out: &mut dyn Write,
    ) -> Result<Vec<Value>> {
        let mut values = Vec::with_capacity(args.len());
        for a in args {
            values.push(self.eval(a, env, out)?);
        }
        Ok(values)
    }

    fn call(
        &mut self,
        f: &Function,
        args: Vec<Value>,
        span: Span,
        out: &mut dyn Write,
    ) -> Result<Value> {
        let decl = &f.decl;
        if args.len() != decl.params.len() {
            return Err(Error::runtime(format!(
                "function '{}' expects {} argument{}, got {}",
                decl.name,
                decl.params.len(),
                if decl.params.len() == 1 { "" } else { "s" },
                args.len()
            ))
            .at(span));
        }
        if self.call_depth >= self.max_call_depth {
            return Err(Error::runtime(format!(
                "stack overflow: more than {} nested calls (infinite recursion?)",
                self.max_call_depth
            ))
            .at(span));
        }

        let scope = Env::child(&f.env);
        for (param, arg) in decl.params.iter().zip(args) {
            let what = format!("argument '{}' of '{}'", param.name, decl.name);
            let value = coerce(&param.resolved, arg, &what).map_err(|e| e.or_at(span))?;
            scope
                .borrow_mut()
                .define(&param.name, value, param.resolved.clone());
        }

        self.call_depth += 1;
        let flow = self.exec_block(&decl.body, &scope, out);
        self.call_depth -= 1;

        let result = match flow? {
            Flow::Return(v) => v,
            _ => Value::Null,
        };
        let what = format!("the return value of '{}'", decl.name);
        coerce(&decl.ret_resolved, result, &what).map_err(|e| e.or_at(span))
    }
}

/// Values handed in by the host are converted to the most natural Belsk2
/// type: a whole `double` becomes an `int` (or `long`) so that
/// `call_function("f", &[Value::from(7.0)])` works for `fn f(x: int)`.
fn host_arg(v: Value) -> Value {
    match v {
        Value::Float(f, _) if f.fract() == 0.0 && f.abs() <= i64::MAX as f64 => {
            let i = f as i64;
            match i32::try_from(i) {
                Ok(small) => Value::int(small),
                Err(_) => Value::from(i),
            }
        }
        v => v,
    }
}

/// Functions are visible in their whole block, before their declaration.
fn hoist_functions(stmts: &[Stmt], env: &EnvRef) {
    for stmt in stmts {
        if let StmtKind::FnDecl(decl) = &stmt.kind {
            let f = Function {
                decl: Rc::clone(decl),
                env: Rc::clone(env),
            };
            let ty = belsk2_typeck::fn_type(decl);
            env.borrow_mut()
                .define(&decl.name, Value::Function(Rc::new(f)), ty);
        }
    }
}

/// Converts an index value to a position, rejecting non-integers and
/// out-of-range values.
fn to_index(idx: &Value, len: usize) -> Result<usize> {
    let Value::Int(i, _) = idx else {
        return Err(Error::type_error(format!(
            "index must be an integer, got {}",
            idx.type_name()
        )));
    };
    usize::try_from(*i)
        .ok()
        .filter(|&i| i < len)
        .ok_or_else(|| Error::runtime(format!("index {i} is out of bounds (length {len})")))
}

pub(crate) fn get_index(obj: &Value, idx: &Value) -> Result<Value> {
    match obj {
        Value::Array(a) => {
            let items = a.borrow()?;
            let i = to_index(idx, items.len())?;
            items
                .get(i)
                .cloned()
                .ok_or_else(|| Error::runtime("index out of bounds"))
        }
        Value::String(s) => {
            let len = s.chars().count();
            let i = to_index(idx, len)?;
            s.chars()
                .nth(i)
                .map(Value::Char)
                .ok_or_else(|| Error::runtime("index out of bounds"))
        }
        other => Err(Error::type_error(format!(
            "cannot index into {}",
            other.type_name()
        ))),
    }
}

fn set_index(obj: &Value, idx: &Value, value: Value) -> Result<()> {
    match obj {
        Value::Array(a) => {
            let mut items = a.borrow_mut()?;
            let i = to_index(idx, items.len())?;
            let slot = items
                .get_mut(i)
                .ok_or_else(|| Error::runtime("index out of bounds"))?;
            *slot = value;
            Ok(())
        }
        Value::String(_) => Err(Error::type_error(
            "strings are immutable; build a new string instead",
        )),
        other => Err(Error::type_error(format!(
            "cannot index into {}",
            other.type_name()
        ))),
    }
}

fn is_text(v: &Value) -> bool {
    matches!(v, Value::String(_))
}

pub(crate) fn binary(op: BinOp, l: &Value, r: &Value) -> Result<Value> {
    use Value::*;
    let mismatch = || {
        Error::type_error(format!(
            "cannot apply '{}' to {} and {}",
            op.symbol(),
            l.type_name(),
            r.type_name()
        ))
    };
    match op {
        BinOp::Eq => return Ok(Bool(l == r)),
        BinOp::Ne => return Ok(Bool(l != r)),
        BinOp::And => return Ok(Bool(l.is_truthy() && r.is_truthy())),
        BinOp::Or => return Ok(Bool(l.is_truthy() || r.is_truthy())),
        BinOp::Add if is_text(l) || is_text(r) || matches!((l, r), (Char(_), Char(_))) => {
            return Ok(String(format!("{l}{r}")))
        }
        BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => {
            let ord = match (l, r) {
                (String(a), String(b)) => Some(a.cmp(b)),
                (Char(a), Char(b)) => Some(a.cmp(b)),
                _ => match num::compare(l, r) {
                    Some(o) => Some(o),
                    // Comparisons with NaN are always false.
                    None if l.as_f64().is_some() && r.as_f64().is_some() => None,
                    None => return Err(mismatch()),
                },
            };
            let Some(ord) = ord else {
                return Ok(Bool(false));
            };
            return Ok(Bool(match op {
                BinOp::Lt => ord.is_lt(),
                BinOp::Gt => ord.is_gt(),
                BinOp::Le => ord.is_le(),
                _ => ord.is_ge(),
            }));
        }
        BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
            if let (Bool(a), Bool(b)) = (l, r) {
                return Ok(Bool(match op {
                    BinOp::BitAnd => a & b,
                    BinOp::BitOr => a | b,
                    _ => a ^ b,
                }));
            }
        }
        _ => {}
    }
    num::arith(op, l, r).unwrap_or_else(|| Err(mismatch()))
}
