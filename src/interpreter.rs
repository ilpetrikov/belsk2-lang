use std::collections::HashMap;
use std::io::{self, BufRead, Write};

use crate::ast::*;
use crate::lexer::Lexer;
use crate::parser::Parser;
use crate::types::*;

#[derive(Debug, Clone)]
pub enum Value {
    Number(f64),
    String(String),
    Bool(bool),
    Array(Vec<Value>),
    Function(FnValue),
    Null,
}

#[derive(Debug, Clone)]
pub struct FnValue {
    pub params: Vec<TypedParam>,
    pub return_type: BType,
    pub has_ret_type: bool,
    pub body: ASTNode,
    pub env: Scope,
}

#[derive(Debug, Clone)]
struct TypedVar {
    value: Value,
    var_type: BType,
}

#[derive(Debug, Clone)]
pub struct Scope {
    vars: HashMap<String, TypedVar>,
    parent: Option<Box<Scope>>,
}

impl Scope {
    fn new(parent: Option<Scope>) -> Self {
        Scope {
            vars: HashMap::new(),
            parent: parent.map(Box::new),
        }
    }

    fn get(&self, name: &str) -> Option<Value> {
        if let Some(tv) = self.vars.get(name) {
            return Some(tv.value.clone());
        }
        if let Some(ref parent) = self.parent {
            return parent.get(name);
        }
        None
    }

    fn set(&mut self, name: &str, v: Value) {
        if let Some(tv) = self.vars.get_mut(name) {
            if tv.var_type == BType::Bel {
                if !matches!(&v, Value::Number(_)) {
                    panic!(
                        "bel error: value {} is not a number for variable '{}'",
                        v.val_str(),
                        name
                    );
                }
                if let Value::Number(n) = &v {
                    if *n > 1000.0 {
                        panic!(
                            "bel error: value {} exceeds maximum of 1000 for variable '{}'",
                            v.val_str(),
                            name
                        );
                    }
                }
            }
            if tv.var_type == BType::Ster && !matches!(&v, Value::String(_)) {
                panic!(
                    "ster error: expected string, got {} for variable '{}'",
                    v.val_type().name(),
                    name
                );
            }
            tv.value = v;
            return;
        }
        if let Some(ref mut parent) = self.parent {
            parent.set(name, v);
            return;
        }
        panic!("undefined variable: {}", name);
    }

    fn define(&mut self, name: &str, v: Value) {
        self.vars.insert(
            name.to_string(),
            TypedVar {
                value: v,
                var_type: BType::Any,
            },
        );
    }

    fn define_typed(&mut self, name: &str, v: Value, t: BType) {
        self.vars.insert(
            name.to_string(),
            TypedVar {
                value: v,
                var_type: t,
            },
        );
    }
}

#[derive(Debug, Clone)]
enum Signal {
    Return(Value),
    Break,
    Continue,
}

fn type_mismatch_error(expected: BType, actual: BType, context: &str) {
    panic!(
        "type error: expected {}, got {} in {}",
        expected.name(),
        actual.name(),
        context
    );
}

impl Value {
    fn val_type(&self) -> BType {
        match self {
            Value::Number(_) => BType::Int,
            Value::String(_) => BType::String,
            Value::Bool(_) => BType::Bool,
            Value::Array(_) => BType::Array,
            Value::Function(_) => BType::Fn,
            Value::Null => BType::Any,
        }
    }

    fn val_str(&self) -> String {
        match self {
            Value::Null => "null".to_string(),
            Value::String(s) => s.clone(),
            Value::Number(n) => {
                if *n == (*n as i64) as f64 && *n < 1e15 && *n > -1e15 {
                    format!("{}", *n as i64)
                } else {
                    format!("{}", n)
                }
            }
            Value::Bool(b) => {
                if *b {
                    "true".to_string()
                } else {
                    "false".to_string()
                }
            }
            Value::Function(_) => "<fn>".to_string(),
            Value::Array(arr) => {
                let parts: Vec<String> = arr.iter().map(|el| el.val_str()).collect();
                format!("[{}]", parts.join(", "))
            }
        }
    }

    fn is_truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Number(n) => *n != 0.0,
            Value::String(s) => !s.is_empty(),
            Value::Array(a) => !a.is_empty(),
            Value::Function(_) => true,
        }
    }
}

fn val_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Null, _) | (_, Value::Null) => false,
        (Value::Number(a), Value::Number(b)) => a == b,
        (Value::String(a), Value::String(b)) => a == b,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        _ => false,
    }
}

pub struct Interpreter {
    global: Scope,
    id_bank: HashMap<i64, Value>,
}

impl Interpreter {
    pub fn new() -> Self {
        Interpreter {
            global: Scope::new(None),
            id_bank: HashMap::new(),
        }
    }

    pub fn run_source(&mut self, source: &str) {
        let mut output = io::stdout();
        self.run_source_with_writer(source, &mut output);
    }

    pub fn run_source_with_writer(&mut self, source: &str, output: &mut dyn Write) {
        let mut lexer = Lexer::new(source);
        let tokens = lexer.tokenize();
        let mut parser = Parser::new(tokens);
        let prog = parser.parse();
        let mut scope = self.global.clone();
        self.exec_stmts(&prog.stmts, &mut scope, output);
        self.global = scope;
    }

    /// Defines (or overwrites) a global variable. Unlike `=` assignment this
    /// never fails on a missing variable, which makes it suitable for
    /// injecting host-side values such as `_dt` and `_entity_id`.
    pub fn define_global(&mut self, name: &str, value: Value) {
        self.global.define(name, value);
    }

    /// Returns `true` when a global function with `name` is defined.
    pub fn has_function(&self, name: &str) -> bool {
        matches!(self.global.get(name), Some(Value::Function(_)))
    }

    /// Calls a global function by name with the given arguments, returning its
    /// return value. Prints output through `output` (like `run_source`). Panics
    /// when the function is not defined or fails to run.
    pub fn call_function(&mut self, name: &str, args: &[Value], output: &mut dyn Write) -> Value {
        let fv = match self.global.get(name) {
            Some(Value::Function(f)) => f,
            _ => {
                panic!("undefined function: {name}");
            }
        };
        let mut fn_scope = Scope::new(Some(fv.env.clone()));
        fn_scope.define(name, Value::Function(fv.clone()));
        let mut args = args.to_vec();
        for (i, param) in fv.params.iter().enumerate() {
            if i < args.len() {
                if !type_compatible(param.param_type, args[i].val_type()) {
                    type_mismatch_error(
                        param.param_type,
                        args[i].val_type(),
                        &format!("argument '{}' of function '{}'", param.name, name),
                    );
                }
                if param.param_type == BType::Int {
                    if let Value::Number(n) = &args[i] {
                        args[i] = Value::Number(*n as i64 as f64);
                    }
                }
                fn_scope.define_typed(&param.name, args[i].clone(), param.param_type);
            }
        }

        let mut result = Value::Null;
        if let Some(Signal::Return(v)) = self.exec(&fv.body, &mut fn_scope, output) {
            result = v;
            if fv.has_ret_type && !type_compatible(fv.return_type, result.val_type()) {
                type_mismatch_error(
                    fv.return_type,
                    result.val_type(),
                    &format!("return value of function '{}'", name),
                );
            }
        }
        result
    }

    fn exec_stmts(&mut self, stmts: &[ASTNode], scope: &mut Scope, output: &mut dyn Write) {
        for s in stmts {
            if let Some(sig) = self.exec(s, scope, output) {
                match sig {
                    Signal::Return(_) | Signal::Break | Signal::Continue => {}
                }
            }
        }
    }

    fn exec(
        &mut self,
        node: &ASTNode,
        scope: &mut Scope,
        output: &mut dyn Write,
    ) -> Option<Signal> {
        match node {
            ASTNode::Program(p) => {
                self.exec_stmts(&p.stmts, scope, output);
                None
            }
            ASTNode::BlockStmt(b) => {
                let mut bs = Scope::new(Some(scope.clone()));
                for s in &b.body {
                    if let Some(sig) = self.exec(s, &mut bs, output) {
                        return Some(sig);
                    }
                }
                None
            }
            ASTNode::ExprStmt(e) => {
                self.eval(&e.expr, scope, output);
                None
            }
            ASTNode::VarDecl(v) => {
                let val = self.eval(&v.value, scope, output);
                if v.is_bel {
                    if !matches!(&val, Value::Number(_)) {
                        panic!(
                            "bel error: value {} is not a number for variable '{}'",
                            val.val_str(),
                            v.name
                        );
                    }
                    if let Value::Number(n) = &val {
                        if *n > 1000.0 {
                            panic!(
                                "bel error: value {} exceeds maximum of 1000 for variable '{}'",
                                val.val_str(),
                                v.name
                            );
                        }
                    }
                    scope.define_typed(&v.name, val, BType::Bel);
                } else if v.is_ster {
                    if !matches!(&val, Value::String(_)) {
                        panic!(
                            "ster error: expected string, got {} for variable '{}'",
                            val.val_type().name(),
                            v.name
                        );
                    }
                    scope.define_typed(&v.name, val, BType::Ster);
                } else if v.has_type {
                    scope.define_typed(&v.name, val, v.var_type);
                } else {
                    scope.define(&v.name, val);
                }
                None
            }
            ASTNode::IdbDecl(d) => {
                let val = self.eval(&d.value, scope, output);
                self.id_bank.insert(d.id, val);
                None
            }
            ASTNode::Assign(a) => {
                let val = self.eval(&a.value, scope, output);
                self.set_target(&a.target, val, scope, output);
                None
            }
            ASTNode::FnDecl(f) => {
                let fv = FnValue {
                    params: f.params.clone(),
                    return_type: f.return_type,
                    has_ret_type: f.has_ret_type,
                    body: (*f.body).clone(),
                    env: Scope::new(None),
                };
                scope.define(&f.name, Value::Function(fv));
                let full_scope = scope.clone();
                if let Some(TypedVar {
                    value: Value::Function(ref mut fv),
                    ..
                }) = scope.vars.get_mut(&f.name)
                {
                    fv.env = full_scope;
                }
                None
            }
            ASTNode::ReturnStmt(r) => {
                let val = if let Some(ref v) = r.value {
                    self.eval(v, scope, output)
                } else {
                    Value::Null
                };
                Some(Signal::Return(val))
            }
            ASTNode::BreakStmt => Some(Signal::Break),
            ASTNode::ContinueStmt => Some(Signal::Continue),
            ASTNode::IfStmt(i) => {
                let cond = self.eval(&i.cond, scope, output);
                if cond.is_truthy() {
                    self.exec(&i.body, scope, output)
                } else if let Some(ref e) = i.else_body {
                    self.exec(e, scope, output)
                } else {
                    None
                }
            }
            ASTNode::WhileStmt(w) => {
                loop {
                    let cond = self.eval(&w.cond, scope, output);
                    if !cond.is_truthy() {
                        break;
                    }
                    match self.exec(&w.body, scope, output) {
                        Some(Signal::Break) => break,
                        Some(Signal::Continue) => continue,
                        r @ Some(Signal::Return(_)) => return r,
                        None => {}
                    }
                }
                None
            }
            ASTNode::ForStmt(f) => {
                let iter = self.eval(&f.iter, scope, output);
                match iter {
                    Value::Array(arr) => {
                        for item in arr {
                            let mut ls = Scope::new(Some(scope.clone()));
                            ls.define(&f.var, item);
                            match self.exec(&f.body, &mut ls, output) {
                                Some(Signal::Break) => break,
                                Some(Signal::Continue) => continue,
                                r @ Some(Signal::Return(_)) => return r,
                                None => {}
                            }
                        }
                    }
                    Value::String(s) => {
                        for ch in s.chars() {
                            let mut ls = Scope::new(Some(scope.clone()));
                            ls.define(&f.var, Value::String(ch.to_string()));
                            match self.exec(&f.body, &mut ls, output) {
                                Some(Signal::Break) => break,
                                Some(Signal::Continue) => continue,
                                r @ Some(Signal::Return(_)) => return r,
                                None => {}
                            }
                        }
                    }
                    _ => {}
                }
                None
            }
            _ => None,
        }
    }

    fn set_target(
        &mut self,
        target: &ASTNode,
        val: Value,
        scope: &mut Scope,
        output: &mut dyn Write,
    ) {
        match target {
            ASTNode::Ident(i) => scope.set(&i.name, val),
            ASTNode::IndexExpr(ix) => {
                let obj = self.eval(&ix.obj, scope, output);
                let idx = self.eval(&ix.index, scope, output);
                if let (Value::Array(mut arr), Value::Number(n)) = (obj, idx) {
                    let i = n as usize;
                    if i < arr.len() {
                        arr[i] = val;
                    }
                }
            }
            _ => {
                panic!("invalid assignment target");
            }
        }
    }

    fn eval(&mut self, node: &ASTNode, scope: &mut Scope, output: &mut dyn Write) -> Value {
        match node {
            ASTNode::StringLit(s) => Value::String(s.value.clone()),
            ASTNode::NumLit(n) => {
                let num = n.value.parse::<f64>().unwrap_or(0.0);
                Value::Number(num)
            }
            ASTNode::BoolLit(b) => Value::Bool(b.value),
            ASTNode::Ident(i) => {
                if i.name == "null" {
                    return Value::Null;
                }
                match scope.get(&i.name) {
                    Some(v) => v,
                    None => {
                        panic!("undefined variable: {}", i.name);
                    }
                }
            }
            ASTNode::ArrayLit(a) => {
                let elems: Vec<Value> = a
                    .elems
                    .iter()
                    .map(|el| self.eval(el, scope, output))
                    .collect();
                Value::Array(elems)
            }
            ASTNode::BinExpr(b) => {
                let left = self.eval(&b.left, scope, output);
                if b.op == "&&" {
                    if !left.is_truthy() {
                        return Value::Bool(false);
                    }
                    return Value::Bool(self.eval(&b.right, scope, output).is_truthy());
                }
                if b.op == "||" {
                    if left.is_truthy() {
                        return Value::Bool(true);
                    }
                    return Value::Bool(self.eval(&b.right, scope, output).is_truthy());
                }
                let right = self.eval(&b.right, scope, output);
                eval_bin_op(&b.op, &left, &right)
            }
            ASTNode::UnaryExpr(u) => {
                let val = self.eval(&u.expr, scope, output);
                match u.op.as_str() {
                    "-" => {
                        if let Value::Number(n) = val {
                            Value::Number(-n)
                        } else {
                            Value::Number(0.0)
                        }
                    }
                    "!" => Value::Bool(!val.is_truthy()),
                    _ => Value::Null,
                }
            }
            ASTNode::CallExpr(c) => self.eval_call(c, scope, output),
            ASTNode::DotExpr(d) => {
                let _obj = self.eval(&d.left, scope, output);
                panic!("undefined property: {}", d.right);
            }
            ASTNode::IndexExpr(ix) => {
                let obj = self.eval(&ix.obj, scope, output);
                let idx = self.eval(&ix.index, scope, output);
                match (&obj, &idx) {
                    (Value::Array(arr), Value::Number(n)) => {
                        let i = *n as usize;
                        if i >= arr.len() {
                            panic!("index out of bounds: {}", i);
                        }
                        arr[i].clone()
                    }
                    (Value::String(s), Value::Number(n)) => {
                        let i = *n as usize;
                        if i >= s.len() {
                            panic!("index out of bounds: {}", i);
                        }
                        Value::String(s.chars().nth(i).unwrap_or('\0').to_string())
                    }
                    _ => Value::Null,
                }
            }
            _ => Value::Null,
        }
    }

    fn eval_call(&mut self, call: &CallExpr, scope: &mut Scope, output: &mut dyn Write) -> Value {
        let args: Vec<Value> = call
            .args
            .iter()
            .map(|a| self.eval(a, scope, output))
            .collect();

        match call.name.as_str() {
            "prinb" => {
                if let Some(first) = args.first() {
                    let v = match first {
                        Value::Number(n) => {
                            if let Some(bv) = self.id_bank.get(&(*n as i64)) {
                                bv
                            } else {
                                first
                            }
                        }
                        _ => first,
                    };
                    writeln!(output, "{}", v.val_str()).unwrap();
                }
                Value::Null
            }
            "reab" => {
                if let Some(Value::Number(n)) = args.first() {
                    let mut line = String::new();
                    io::stdin().lock().read_line(&mut line).unwrap();
                    line.truncate(line.trim_end_matches(&['\r', '\n'][..]).len());
                    let v = Value::String(line);
                    self.id_bank.insert(*n as i64, v.clone());
                    return v;
                }
                Value::Null
            }
            "input" => {
                if let Some(first) = args.first() {
                    write!(output, "{}", first.val_str()).unwrap();
                    output.flush().unwrap();
                }
                let mut line = String::new();
                io::stdin().lock().read_line(&mut line).unwrap();
                line.truncate(line.trim_end_matches(&['\r', '\n'][..]).len());
                Value::String(line)
            }
            "len" => {
                if let Some(first) = args.first() {
                    match first {
                        Value::String(s) => Value::Number(s.len() as f64),
                        Value::Array(a) => Value::Number(a.len() as f64),
                        _ => Value::Number(0.0),
                    }
                } else {
                    Value::Number(0.0)
                }
            }
            "str" => {
                if let Some(first) = args.first() {
                    Value::String(first.val_str())
                } else {
                    Value::String(String::new())
                }
            }
            "num" => {
                if let Some(Value::String(s)) = args.first() {
                    Value::Number(s.parse::<f64>().unwrap_or(0.0))
                } else {
                    Value::Number(0.0)
                }
            }
            "int" => {
                if let Some(first) = args.first() {
                    match first {
                        Value::Number(n) => Value::Number(*n as i64 as f64),
                        Value::String(s) => {
                            Value::Number(s.parse::<f64>().unwrap_or(0.0) as i64 as f64)
                        }
                        _ => Value::Number(0.0),
                    }
                } else {
                    Value::Number(0.0)
                }
            }
            "float" => {
                if let Some(first) = args.first() {
                    match first {
                        Value::Number(n) => Value::Number(*n),
                        Value::String(s) => Value::Number(s.parse::<f64>().unwrap_or(0.0)),
                        _ => Value::Number(0.0),
                    }
                } else {
                    Value::Number(0.0)
                }
            }
            "bool" => {
                if let Some(first) = args.first() {
                    Value::Bool(first.is_truthy())
                } else {
                    Value::Bool(false)
                }
            }
            "push" => {
                if args.len() == 2 {
                    if let Value::Array(mut arr) = args[0].clone() {
                        arr.push(args[1].clone());
                        return Value::Array(arr);
                    }
                }
                Value::Null
            }
            "pop" => {
                if args.len() == 1 {
                    if let Value::Array(mut arr) = args[0].clone() {
                        if !arr.is_empty() {
                            return arr.pop().unwrap();
                        }
                    }
                }
                Value::Null
            }
            "substr" => {
                if args.len() == 3 {
                    if let (Value::String(s), Value::Number(start), Value::Number(length)) =
                        (&args[0], &args[1], &args[2])
                    {
                        let st = *start as usize;
                        let len = *length as usize;
                        if st >= s.len() {
                            return Value::String(String::new());
                        }
                        let mut end = st + len;
                        if end > s.len() {
                            end = s.len();
                        }
                        return Value::String(s[st..end].to_string());
                    }
                }
                Value::Null
            }
            "type" => {
                if let Some(first) = args.first() {
                    Value::String(first.val_type().name().to_string())
                } else {
                    Value::String("null".to_string())
                }
            }
            _ => {
                let fn_val = match scope.get(&call.name) {
                    Some(v) => v,
                    None => {
                        panic!("undefined function: {}", call.name);
                    }
                };
                let fv = match fn_val {
                    Value::Function(f) => f,
                    _ => {
                        panic!("{} is not a function", call.name);
                    }
                };
                let mut fn_scope = Scope::new(Some(fv.env.clone()));
                fn_scope.define(&call.name, Value::Function(fv.clone()));
                let mut args = args;
                for (i, param) in fv.params.iter().enumerate() {
                    if i < args.len() {
                        if !type_compatible(param.param_type, args[i].val_type()) {
                            type_mismatch_error(
                                param.param_type,
                                args[i].val_type(),
                                &format!("argument '{}' of function '{}'", param.name, call.name),
                            );
                        }
                        if param.param_type == BType::Int {
                            if let Value::Number(n) = &args[i] {
                                args[i] = Value::Number(*n as i64 as f64);
                            }
                        }
                        fn_scope.define_typed(&param.name, args[i].clone(), param.param_type);
                    }
                }

                let mut result = Value::Null;
                if let Some(Signal::Return(v)) = self.exec(&fv.body, &mut fn_scope, output) {
                    result = v;
                    if fv.has_ret_type && !type_compatible(fv.return_type, result.val_type()) {
                        type_mismatch_error(
                            fv.return_type,
                            result.val_type(),
                            &format!("return value of function '{}'", call.name),
                        );
                    }
                }
                result
            }
        }
    }
}

fn eval_bin_op(op: &str, left: &Value, right: &Value) -> Value {
    match op {
        "+" => {
            if matches!(left, Value::String(_)) || matches!(right, Value::String(_)) {
                return Value::String(format!("{}{}", left.val_str(), right.val_str()));
            }
            if let (Value::Number(l), Value::Number(r)) = (left, right) {
                return Value::Number(l + r);
            }
            Value::Number(0.0)
        }
        "-" => {
            if let (Value::Number(l), Value::Number(r)) = (left, right) {
                Value::Number(l - r)
            } else {
                Value::Number(0.0)
            }
        }
        "*" => {
            if let (Value::Number(l), Value::Number(r)) = (left, right) {
                Value::Number(l * r)
            } else {
                Value::Number(0.0)
            }
        }
        "/" => {
            if let (Value::Number(l), Value::Number(r)) = (left, right) {
                if *r == 0.0 {
                    panic!("division by zero");
                }
                Value::Number(l / r)
            } else {
                Value::Number(0.0)
            }
        }
        "%" => {
            if let (Value::Number(l), Value::Number(r)) = (left, right) {
                Value::Number((*l as i64 % *r as i64) as f64)
            } else {
                Value::Number(0.0)
            }
        }
        "==" => Value::Bool(val_equal(left, right)),
        "!=" => Value::Bool(!val_equal(left, right)),
        "<" => {
            if let (Value::Number(l), Value::Number(r)) = (left, right) {
                Value::Bool(l < r)
            } else {
                Value::Bool(false)
            }
        }
        ">" => {
            if let (Value::Number(l), Value::Number(r)) = (left, right) {
                Value::Bool(l > r)
            } else {
                Value::Bool(false)
            }
        }
        "<=" => {
            if let (Value::Number(l), Value::Number(r)) = (left, right) {
                Value::Bool(l <= r)
            } else {
                Value::Bool(false)
            }
        }
        ">=" => {
            if let (Value::Number(l), Value::Number(r)) = (left, right) {
                Value::Bool(l >= r)
            } else {
                Value::Bool(false)
            }
        }
        _ => Value::Null,
    }
}

pub fn run_file(filename: &str) {
    let data = std::fs::read_to_string(filename).unwrap_or_else(|e| {
        eprintln!("error reading file: {}", e);
        std::process::exit(1);
    });
    let mut interp = Interpreter::new();
    interp.run_source(&data);
}

pub fn run_source(source: &str) {
    let mut interp = Interpreter::new();
    interp.run_source(source);
}
