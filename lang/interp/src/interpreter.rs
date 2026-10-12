use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::rc::Rc;

use belsk2_syntax::ast::*;
use belsk2_syntax::{Error, Result, Span, Ty, TypeKind};
use belsk2_typeck::Checker;

use crate::builtins;
use crate::collections::{Array, Map};
use crate::env::{self, Env, EnvRef};
use crate::num::{self, coerce, convert};
use crate::trace::{self, Trace};
use crate::value::{Class, Function, Object, StructValue, Value};

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
    /// What the type parameters of the running function stand for, one
    /// frame per call.
    type_env: Vec<HashMap<Rc<str>, Ty>>,
    /// Where to record line hits and values, when an editor asks for it.
    trace: Option<Rc<RefCell<Trace>>>,
    /// Module scopes, kept while tracing so their variables can be shown.
    module_envs: Vec<EnvRef>,
    /// Every user-defined type declared so far, by type id (for default
    /// values of fields).
    classes: HashMap<usize, Rc<Class>>,
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
            trace: None,
            module_envs: Vec::new(),
            input: Input::Stdin,
            call_depth: 0,
            max_call_depth: DEFAULT_MAX_CALL_DEPTH,
            type_env: Vec::new(),
            classes: HashMap::new(),
        }
    }

    /// Reads `reab`/`input` lines from `reader` instead of stdin.
    /// Records how often each line runs and the values written there
    /// into `trace` (for live values in editors).
    pub fn set_trace(&mut self, trace: Rc<RefCell<Trace>>) {
        self.trace = Some(trace);
    }

    /// Copies the current values of every module's top-level variables
    /// into the trace.
    pub fn snapshot_globals(&self) {
        let mut globals = Vec::new();
        for env in &self.module_envs {
            for (name, value, _) in env.borrow().entries() {
                if !matches!(value, Value::Function(_) | Value::Native(_)) {
                    globals.push((name, trace::short(&value)));
                }
            }
        }
        self.record(|t| t.globals = globals.into_iter().collect());
    }

    fn record(&self, f: impl FnOnce(&mut Trace)) {
        if let Some(t) = &self.trace {
            if let Ok(mut t) = t.try_borrow_mut() {
                f(&mut t);
            }
        }
    }

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

    /// Parses, checks and runs source code: its declarations, then `main`
    /// if it declares one. Returns the first error; use
    /// [`Interpreter::compile`] to get all of them.
    pub fn run_source_with_writer(&mut self, source: &str, out: &mut dyn Write) -> Result<()> {
        let program = self.compile(source).map_err(|errors| {
            errors
                .into_iter()
                .next()
                .unwrap_or_else(|| Error::type_error("invalid program"))
        })?;
        self.run_program(&program, out)?;
        let declares_main = program
            .stmts
            .iter()
            .any(|s| matches!(&s.kind, StmtKind::FnDecl(f) if f.name == "main"));
        if declares_main {
            if let Some(main) = self.get_global("main") {
                self.call_main(&main, &[], out)?;
            }
        }
        Ok(())
    }

    /// Calls a program's `main` function with command-line arguments.
    /// Returns its exit code if it returns an `int`.
    pub fn call_main(
        &mut self,
        main: &Value,
        args: &[String],
        out: &mut dyn Write,
    ) -> Result<Option<i64>> {
        let Value::Function(f) = main else {
            return Err(Error::runtime("`main` is not a function"));
        };
        let values = if f.decl.params.is_empty() {
            Vec::new()
        } else {
            let items = args.iter().map(|a| Value::String(a.clone())).collect();
            vec![Value::Array(Array::typed(Ty::String, items))]
        };
        self.call_depth = 0;
        let result = self.call(f, values, &[], f.decl.span, out)?;
        out.flush()?;
        Ok(result.as_i64())
    }

    /// Parses and statically checks source code. Globals that already exist
    /// in this interpreter (from earlier runs or [`Interpreter::define_global`])
    /// are visible to the checker. The returned program carries the types
    /// and conversions the checker added.
    pub fn compile(&self, source: &str) -> std::result::Result<Program, Vec<Error>> {
        self.compile_with(source, false)
    }

    /// Like [`Interpreter::compile`], but statements may also appear outside
    /// functions, as typed into a REPL.
    pub fn compile_repl(&self, source: &str) -> std::result::Result<Program, Vec<Error>> {
        self.compile_with(source, true)
    }

    fn compile_with(&self, source: &str, repl: bool) -> std::result::Result<Program, Vec<Error>> {
        let mut program = belsk2_syntax::parse(source).map_err(|e| vec![e])?;
        if let Some(stmt) = program
            .stmts
            .iter()
            .find(|s| matches!(s.kind, StmtKind::Using(_)))
        {
            return Err(vec![Error::syntax(
                "'using' needs a file or a project; run the program with `belsk2 run`",
                stmt.span,
            )]);
        }
        let mut checker = Checker::new();
        if repl {
            checker.allow_top_level_statements();
        }
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
        self.hoist(&program.stmts, &globals);
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

    /// Runs one module of a multi-file program in its own scope. `imports`
    /// are the declarations its `using` statements bring in. Returns the
    /// module's top-level declarations, for modules that import it.
    pub fn run_module(
        &mut self,
        program: &Program,
        imports: &[(String, Value, Ty)],
        out: &mut dyn Write,
    ) -> Result<Vec<(String, Value, Ty)>> {
        let import_env = Env::child(&self.globals);
        for (name, value, ty) in imports {
            import_env
                .borrow_mut()
                .define(name, value.clone(), ty.clone());
        }
        let module_env = Env::child(&import_env);
        if self.trace.is_some() {
            self.module_envs.push(module_env.clone());
        }
        self.call_depth = 0;
        self.hoist(&program.stmts, &module_env);
        for stmt in &program.stmts {
            match self.exec(stmt, &module_env, out)? {
                Flow::Normal => {}
                Flow::Return(_) | Flow::Break | Flow::Continue => {
                    return Err(Error::runtime("unexpected control flow at top level").at(stmt.span))
                }
            }
        }
        out.flush()?;
        let exports = module_env.borrow().entries();
        Ok(exports)
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
        let result = self.call(&f, args, &[], f.decl.span, out)?;
        out.flush()?;
        Ok(result)
    }

    /// Calls a function value with arguments that already have the
    /// parameter types.
    pub fn call_value(
        &mut self,
        f: &Value,
        args: Vec<Value>,
        out: &mut dyn Write,
    ) -> Result<Value> {
        self.call_depth = 0;
        let result = match f {
            Value::Function(f) => self.call(f, args, &[], f.decl.span, out)?,
            Value::Native(n) => (n.func)(args, out)?,
            other => {
                return Err(Error::runtime(format!(
                    "{} is not a function",
                    other.type_name()
                )))
            }
        };
        out.flush()?;
        Ok(result)
    }

    // ---- statements ----------------------------------------------------

    fn exec_block(&mut self, block: &Block, env: &EnvRef, out: &mut dyn Write) -> Result<Flow> {
        let scope = Env::child(env);
        self.hoist(&block.stmts, &scope);
        for stmt in &block.stmts {
            match self.exec(stmt, &scope, out)? {
                Flow::Normal => {}
                flow => return Ok(flow),
            }
        }
        Ok(Flow::Normal)
    }

    fn exec(&mut self, stmt: &Stmt, env: &EnvRef, out: &mut dyn Write) -> Result<Flow> {
        if self.trace.is_some() && !matches!(stmt.kind, StmtKind::FnDecl(_) | StmtKind::Using(_)) {
            self.record(|t| t.hit(stmt.span));
        }
        with_stack(|| self.exec_inner(stmt, env, out)).map_err(|e| e.or_at(stmt.span))
    }

    fn exec_inner(&mut self, stmt: &Stmt, env: &EnvRef, out: &mut dyn Write) -> Result<Flow> {
        match &stmt.kind {
            // Imports are bound by `run_module` before the module runs.
            StmtKind::Using(_) => {}
            StmtKind::Expr(e) => {
                self.eval(e, env, out)?;
            }
            StmtKind::VarDecl(d) => {
                let value = self.eval(&d.value, env, out)?;
                let ty = self.rt(&d.resolved);
                let value = coerce(&ty, value, &format!("'{}'", d.name))?;
                self.record(|t| t.value(stmt.span, d.name.clone(), &value));
                env.borrow_mut().define(&d.name, value, ty);
            }
            StmtKind::Idb { id, value } => {
                let value = self.eval(value, env, out)?;
                self.record(|t| {
                    t.value(stmt.span, format!("idb {id}"), &value);
                    t.idb(*id, &value);
                });
                self.id_bank.insert(*id, value);
            }
            StmtKind::Assign { target, value } => {
                let value = self.eval(value, env, out)?;
                if let Some(name) = self.trace.as_ref().and_then(|_| trace::describe(target)) {
                    self.record(|t| t.value(stmt.span, name, &value));
                }
                self.assign(target, value, env, out)?;
            }
            StmtKind::CompoundAssign {
                op,
                target,
                value,
                cast,
            } => {
                self.compound_assign(*op, target, value, cast, env, out)?;
                if let (Some(_), ExprKind::Ident(name)) = (&self.trace, &target.kind) {
                    if let Some(v) = env::lookup(env, name) {
                        self.record(|t| t.value(stmt.span, name.clone(), &v));
                    }
                }
            }
            // Defined by `hoist` when the block was entered.
            StmtKind::FnDecl(_) | StmtKind::Type(_) => {}
            StmtKind::Return(value) => {
                let v = match value {
                    Some(e) => self.eval(e, env, out)?,
                    None => Value::Null,
                };
                if value.is_some() {
                    self.record(|t| t.value(stmt.span, "return".to_string(), &v));
                }
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
                    Value::Map(m) => m.keys()?,
                    Value::String(s) => s.chars().map(Value::Char).collect(),
                    other => {
                        return Err(Error::type_error(format!(
                            "cannot iterate over {}",
                            other.type_name()
                        ))
                        .at(iter.span))
                    }
                };
                let var_ty = self.rt(var_ty);
                for item in items {
                    let scope = Env::child(env);
                    let item = coerce(&var_ty, item, &format!("'{var}'"))?;
                    self.record(|t| t.value(stmt.span, var.clone(), &item));
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
            ExprKind::Member { object, name } => {
                let obj = self.eval(object, env, out)?;
                if let Value::Object(o) = &obj {
                    let (i, ty) = field_of(&o.class, name).map_err(|e| e.or_at(target.span))?;
                    let value = coerce(&ty, value, &format!("field '{name}'"))?;
                    let mut fields = o
                        .fields
                        .try_borrow_mut()
                        .map_err(|_| Error::runtime("object is being modified"))?;
                    if let Some(slot) = fields.get_mut(i) {
                        *slot = value;
                    }
                    return Ok(());
                }
                let Value::Struct(s) = &obj else {
                    return Err(Error::runtime(format!(
                        "{} has no field '{name}'",
                        obj.type_name()
                    ))
                    .at(target.span));
                };
                let (i, ty) = field_of(&s.class, name).map_err(|e| e.or_at(target.span))?;
                let value = coerce(&ty, value, &format!("field '{name}'"))?;
                // A struct is a value: build the changed copy and store it
                // where the old one was.
                let mut new = (**s).clone();
                if let Some(slot) = new.fields.get_mut(i) {
                    *slot = value;
                }
                self.assign(object, Value::Struct(Rc::new(new)), env, out)
            }
            ExprKind::This => env::assign(env, "this", value).map_err(|e| e.or_at(target.span)),
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
        let cast = self.rt(cast);
        let finish = |v: Value| {
            if cast.is_any() {
                Ok(v)
            } else {
                convert(v, &cast)
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
                let elem = match &expr.ty {
                    Ty::Array(elem) => self.rt(elem),
                    _ => Ty::Any,
                };
                let mut values = Vec::with_capacity(items.len());
                for item in items {
                    values.push(self.eval(item, env, out)?);
                }
                Ok(Value::Array(Array::typed(elem, values)))
            }
            ExprKind::Map(entries) => {
                let map = match &expr.ty {
                    Ty::Map(k, v) => Map::new(self.rt(k), self.rt(v)),
                    _ => Map::new(Ty::Any, Ty::Any),
                };
                for (k, v) in entries {
                    let k = self.eval(k, env, out)?;
                    let v = self.eval(v, env, out)?;
                    map.insert(k, v)?;
                }
                Ok(Value::Map(map))
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
            ExprKind::Call {
                callee, args, inst, ..
            } => {
                let inst: Vec<Ty> = inst.iter().map(|t| self.rt(t)).collect();
                self.eval_call(callee, args, &inst, expr.span, env, out)
            }
            ExprKind::Member { object, name } => {
                let obj = self.eval(object, env, out)?;
                match &obj {
                    Value::Struct(s) => {
                        if let Some(i) = s.class.field_index(name) {
                            return s
                                .fields
                                .get(i)
                                .cloned()
                                .ok_or_else(|| Error::runtime("missing field"));
                        }
                    }
                    Value::Object(o) => {
                        if let Some(i) = o.class.field_index(name) {
                            let fields = o
                                .fields
                                .try_borrow()
                                .map_err(|_| Error::runtime("object is being modified"))?;
                            return fields
                                .get(i)
                                .cloned()
                                .ok_or_else(|| Error::runtime("missing field"));
                        }
                    }
                    Value::Type(c) => {
                        if let Some(i) = c.cases.iter().position(|(n, _)| n == name) {
                            return Ok(Value::Enum(Rc::clone(c), i));
                        }
                    }
                    _ => {}
                }
                Err(Error::runtime(format!(
                    "{} has no member '{name}'",
                    obj.type_name()
                )))
            }
            ExprKind::This => {
                env::lookup(env, "this").ok_or_else(|| Error::runtime("'this' outside of a method"))
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
        inst: &[Ty],
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
        if let ExprKind::Member { object, name } = &callee.kind {
            let obj = self.eval(object, env, out)?;
            let class = match &obj {
                Value::Struct(s) => Some(Rc::clone(&s.class)),
                Value::Object(o) => Some(Rc::clone(&o.class)),
                Value::Enum(c, _) => Some(Rc::clone(c)),
                _ => None,
            };
            if let Some(class) = class {
                if let Some(decl) = class.method(name).cloned() {
                    let values = self.eval_args(args, env, out)?;
                    let this = Some((obj.clone(), Ty::Named(class.ty.clone())));
                    let (result, this) =
                        self.call_decl(&decl, &class.env, values, inst, span, this, out)?;
                    // The method may have changed `this`: store the changed
                    // struct back where it came from.
                    if let Some(this) = this.filter(|t| *t != obj) {
                        if object.is_place() {
                            self.assign(object, this, env, out)?;
                        }
                    }
                    return Ok(result);
                }
            }
        }
        let f = match self.eval(callee, env, out)? {
            Value::Function(f) => f,
            Value::Type(class) => {
                let values = self.eval_args(args, env, out)?;
                return self.construct(&class, values, span, out);
            }
            Value::Native(n) => {
                let values = self.eval_args(args, env, out)?;
                return (n.func)(values, out).map_err(|e| e.or_at(span));
            }
            other => {
                return Err(
                    Error::type_error(format!("{} is not a function", other.type_name()))
                        .at(callee.span),
                )
            }
        };
        let values = self.eval_args(args, env, out)?;
        self.call(&f, values, inst, span, out)
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

    /// A type as it is at run time: type parameters replaced by what they
    /// stand for in the running call (`any` if unknown).
    fn rt(&self, t: &Ty) -> Ty {
        if !t.has_params() {
            return t.clone();
        }
        let frame = self.type_env.last();
        t.substitute(&|name| Some(frame.and_then(|f| f.get(name)).cloned().unwrap_or(Ty::Any)))
    }

    fn call(
        &mut self,
        f: &Function,
        args: Vec<Value>,
        inst: &[Ty],
        span: Span,
        out: &mut dyn Write,
    ) -> Result<Value> {
        self.call_decl(&f.decl, &f.env, args, inst, span, None, out)
            .map(|(v, _)| v)
    }

    /// Calls a function declared in `env`; for a method, `this` is the
    /// value it runs on, and its value at the end is returned too.
    #[allow(clippy::too_many_arguments)]
    fn call_decl(
        &mut self,
        decl: &FnDecl,
        env: &EnvRef,
        args: Vec<Value>,
        inst: &[Ty],
        span: Span,
        this: Option<(Value, Ty)>,
        out: &mut dyn Write,
    ) -> Result<(Value, Option<Value>)> {
        let frame = decl
            .type_params
            .iter()
            .enumerate()
            .map(|(i, name)| {
                (
                    Rc::from(name.as_str()),
                    inst.get(i).cloned().unwrap_or(Ty::Any),
                )
            })
            .collect();
        self.type_env.push(frame);
        let result = self.call_in_frame(decl, env, args, span, this, out);
        self.type_env.pop();
        result
    }

    fn call_in_frame(
        &mut self,
        decl: &FnDecl,
        env: &EnvRef,
        args: Vec<Value>,
        span: Span,
        this: Option<(Value, Ty)>,
        out: &mut dyn Write,
    ) -> Result<(Value, Option<Value>)> {
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

        let scope = Env::child(env);
        let has_this = this.is_some();
        if let Some((value, ty)) = this {
            scope.borrow_mut().define("this", value, ty);
        }
        for (param, arg) in decl.params.iter().zip(args) {
            let what = format!("argument '{}' of '{}'", param.name, decl.name);
            let ty = self.rt(&param.resolved);
            let value = coerce(&ty, arg, &what).map_err(|e| e.or_at(span))?;
            scope.borrow_mut().define(&param.name, value, ty);
        }

        self.call_depth += 1;
        let flow = self.exec_block(&decl.body, &scope, out);
        self.call_depth -= 1;

        let result = match flow? {
            Flow::Return(v) => v,
            _ => Value::Null,
        };
        let what = format!("the return value of '{}'", decl.name);
        let result =
            coerce(&self.rt(&decl.ret_resolved), result, &what).map_err(|e| e.or_at(span))?;
        let this = if has_this {
            env::lookup(&scope, "this")
        } else {
            None
        };
        Ok((result, this))
    }

    /// Defines the functions and types of a block: they are visible in the
    /// whole block, before their declaration.
    fn hoist(&mut self, stmts: &[Stmt], env: &EnvRef) {
        for stmt in stmts {
            match &stmt.kind {
                StmtKind::FnDecl(decl) => {
                    let f = Function {
                        decl: Rc::clone(decl),
                        env: Rc::clone(env),
                    };
                    let ty = belsk2_typeck::fn_type(decl);
                    env.borrow_mut()
                        .define(&decl.name, Value::Function(Rc::new(f)), ty);
                }
                StmtKind::Type(decl) => {
                    let Some(info) = &decl.info else {
                        continue;
                    };
                    let ty = info.reference();
                    let class = Rc::new(Class {
                        decl: Rc::clone(decl),
                        fields: ty
                            .all_fields()
                            .into_iter()
                            .map(|f| (f.name, f.ty))
                            .collect(),
                        cases: ty
                            .variants()
                            .into_iter()
                            .map(|v| (v.name, v.value))
                            .collect(),
                        ty: ty.clone(),
                        env: Rc::clone(env),
                    });
                    self.classes.insert(ty.id(), Rc::clone(&class));
                    env.borrow_mut()
                        .define(&decl.name, Value::Type(class), Ty::Type(ty));
                }
                _ => {}
            }
        }
    }

    /// `Point(1, 2)`: fields get their declared values (or defaults), then
    /// `init` runs, or, without one, the arguments go to the fields that
    /// have no value, in order.
    fn construct(
        &mut self,
        class: &Rc<Class>,
        args: Vec<Value>,
        span: Span,
        out: &mut dyn Write,
    ) -> Result<Value> {
        if class.ty.kind == TypeKind::Enum {
            return Err(
                Error::runtime(format!("enum {} has no constructor", class.ty.name)).at(span),
            );
        }
        let mut fields = Vec::with_capacity(class.fields.len());
        let mut takes_arg = Vec::new();
        for m in &class.decl.members {
            if let MemberKind::Field(f) = &m.kind {
                let ty = self.rt(&f.resolved);
                let value = match &f.value {
                    Some(e) => {
                        let v = self.eval(e, &class.env, out)?;
                        coerce(&ty, v, &format!("field '{}'", f.name))?
                    }
                    None => {
                        takes_arg.push(fields.len());
                        self.default_value(&ty, 0)
                    }
                };
                fields.push(value);
            }
        }
        let is_class = class.ty.kind == TypeKind::Class;
        let make = |fields: Vec<Value>| {
            if is_class {
                Value::Object(Rc::new(Object {
                    class: Rc::clone(class),
                    fields: std::cell::RefCell::new(fields),
                }))
            } else {
                Value::Struct(Rc::new(StructValue {
                    class: Rc::clone(class),
                    fields,
                }))
            }
        };
        if let Some(init) = class.init().cloned() {
            let this = Some((make(fields), Ty::Named(class.ty.clone())));
            let (_, this) = self.call_decl(&init, &class.env, args, &[], span, this, out)?;
            return this.ok_or_else(|| Error::runtime("init lost 'this'").at(span));
        }
        if args.len() != takes_arg.len() {
            return Err(Error::runtime(format!(
                "'{}' expects {} argument{}, got {}",
                class.ty.name,
                takes_arg.len(),
                if takes_arg.len() == 1 { "" } else { "s" },
                args.len()
            ))
            .at(span));
        }
        for (i, arg) in takes_arg.into_iter().zip(args) {
            let (name, ty) = class
                .fields
                .get(i)
                .cloned()
                .unwrap_or_else(|| (String::new(), Ty::Any));
            if let Some(slot) = fields.get_mut(i) {
                *slot = coerce(&ty, arg, &format!("field '{name}'")).map_err(|e| e.or_at(span))?;
            }
        }
        Ok(make(fields))
    }

    /// The value a field has before it is assigned: `0`, `false`, `null`,
    /// or a struct whose fields have their default values.
    fn default_value(&self, ty: &Ty, depth: usize) -> Value {
        match ty.canonical() {
            Ty::Bool => Value::Bool(false),
            Ty::Int(k) => Value::Int(0, k),
            Ty::Float(k) => Value::Float(0.0, k),
            Ty::Char => Value::Char('\0'),
            Ty::Named(t) if t.kind == TypeKind::Enum => match self.classes.get(&t.id()) {
                Some(class) => Value::Enum(Rc::clone(class), 0),
                None => Value::Null,
            },
            Ty::Named(t) if t.kind == TypeKind::Struct && depth < 32 => {
                match self.classes.get(&t.id()) {
                    Some(class) => Value::Struct(Rc::new(StructValue {
                        class: Rc::clone(class),
                        fields: class
                            .fields
                            .iter()
                            .map(|(_, ty)| self.default_value(ty, depth + 1))
                            .collect(),
                    })),
                    None => Value::Null,
                }
            }
            _ => Value::Null,
        }
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

/// The index and type of field `name`.
fn field_of(class: &Class, name: &str) -> Result<(usize, Ty)> {
    class
        .fields
        .iter()
        .position(|(n, _)| n == name)
        .and_then(|i| class.fields.get(i).map(|(_, t)| (i, t.clone())))
        .ok_or_else(|| Error::runtime(format!("{} has no field '{name}'", class.ty.name)))
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
        Value::Map(m) => m.get(idx),
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
        Value::Map(m) => m.insert(idx.clone(), value),
        Value::Array(a) => {
            let value = a.check(value)?;
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
