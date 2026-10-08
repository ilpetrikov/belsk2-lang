use std::cell::{Ref, RefCell, RefMut};
use std::fmt;
use std::rc::Rc;

use belsk2_syntax::ast::FnDecl;
use belsk2_syntax::{BType, Error, Result};

use crate::env::EnvRef;

/// Arrays are reference types: copies of a value share the same elements,
/// so `push(a, x)` or `a[0] = x` is visible through every alias.
#[derive(Clone, Default)]
pub struct Array(Rc<RefCell<Vec<Value>>>);

impl Array {
    pub fn new(items: Vec<Value>) -> Self {
        Array(Rc::new(RefCell::new(items)))
    }

    /// Reads the elements. Fails only if the array is being modified at the
    /// same moment, which the interpreter never does.
    pub fn borrow(&self) -> Result<Ref<'_, Vec<Value>>> {
        self.0
            .try_borrow()
            .map_err(|_| Error::runtime("array is being modified"))
    }

    pub fn borrow_mut(&self) -> Result<RefMut<'_, Vec<Value>>> {
        self.0
            .try_borrow_mut()
            .map_err(|_| Error::runtime("array is being modified"))
    }

    /// A copy of the elements (the elements themselves are shared).
    pub fn snapshot(&self) -> Result<Vec<Value>> {
        Ok(self.borrow()?.clone())
    }

    pub fn len(&self) -> usize {
        self.0.try_borrow().map(|v| v.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn ptr_eq(&self, other: &Array) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

/// Dropping deeply nested arrays (`a = [a]` in a loop) must not recurse,
/// otherwise it could overflow the stack. Children that would be freed
/// together with this array are unlinked onto a heap-allocated work list.
impl Drop for Array {
    fn drop(&mut self) {
        if Rc::strong_count(&self.0) != 1 {
            return;
        }
        let Ok(mut items) = self.0.try_borrow_mut() else {
            return;
        };
        let mut work = std::mem::take(&mut *items);
        drop(items);
        while let Some(v) = work.pop() {
            if let Value::Array(child) = v {
                if Rc::strong_count(&child.0) == 1 {
                    if let Ok(mut inner) = child.0.try_borrow_mut() {
                        work.append(&mut inner);
                    }
                }
            }
        }
    }
}

/// A runtime value.
#[derive(Clone)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Array),
    Function(Rc<Function>),
}

/// A user-defined function together with the environment it closes over.
pub struct Function {
    pub decl: Rc<FnDecl>,
    pub env: EnvRef,
}

/// Values nested deeper than this are not printed or compared further.
/// It also stops runaway recursion on arrays that contain themselves.
const MAX_NESTING: usize = 64;

impl Value {
    pub fn array(items: Vec<Value>) -> Value {
        Value::Array(Array::new(items))
    }

    pub fn string(s: impl Into<String>) -> Value {
        Value::String(s.into())
    }

    /// The runtime type, as reported by `type(x)`.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Number(n) if is_integral(*n) => "int",
            Value::Number(_) => "float",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Function(_) => "fn",
        }
    }

    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Number(n) => *n != 0.0,
            Value::String(s) => !s.is_empty(),
            Value::Array(a) => !a.is_empty(),
            Value::Function(_) => true,
        }
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    /// Whether a value of this kind may be stored in a variable of type `ty`.
    pub fn fits(&self, ty: BType) -> bool {
        match (ty, self) {
            (BType::Any, _) => true,
            (BType::Bel | BType::Ster, Value::Null) => false,
            (_, Value::Null) => true,
            (BType::Int | BType::Float | BType::Bel, Value::Number(_)) => true,
            (BType::String | BType::Ster, Value::String(_)) => true,
            (BType::Bool, Value::Bool(_)) => true,
            (BType::Array, Value::Array(_)) => true,
            (BType::Fn, Value::Function(_)) => true,
            _ => false,
        }
    }

    fn fmt_nested(&self, f: &mut fmt::Formatter<'_>, depth: usize) -> fmt::Result {
        match self {
            Value::Null => f.write_str("null"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Number(n) => f.write_str(&format_number(*n)),
            Value::String(s) => f.write_str(s),
            Value::Function(func) => write!(f, "<fn {}>", func.decl.name),
            Value::Array(items) => {
                if depth >= MAX_NESTING {
                    return f.write_str("[...]");
                }
                let Ok(items) = items.borrow() else {
                    return f.write_str("[...]");
                };
                f.write_str("[")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    item.fmt_nested(f, depth + 1)?;
                }
                f.write_str("]")
            }
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_nested(f, 0)
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::String(s) => write!(f, "{s:?}"),
            other => write!(f, "{other}"),
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        values_equal(self, other, 0)
    }
}

fn values_equal(a: &Value, b: &Value, depth: usize) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Number(x), Value::Number(y)) => x == y,
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Function(x), Value::Function(y)) => Rc::ptr_eq(x, y),
        (Value::Array(x), Value::Array(y)) => {
            if x.ptr_eq(y) {
                return true;
            }
            if depth >= MAX_NESTING {
                return false;
            }
            let (Ok(x), Ok(y)) = (x.borrow(), y.borrow()) else {
                return false;
            };
            x.len() == y.len()
                && x.iter()
                    .zip(y.iter())
                    .all(|(a, b)| values_equal(a, b, depth + 1))
        }
        _ => false,
    }
}

pub(crate) fn is_integral(n: f64) -> bool {
    n.is_finite() && n.fract() == 0.0
}

/// Whole numbers print without a decimal point (`3`, not `3.0`).
pub fn format_number(n: f64) -> String {
    if is_integral(n) && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

impl From<f64> for Value {
    fn from(n: f64) -> Self {
        Value::Number(n)
    }
}

impl From<i64> for Value {
    fn from(n: i64) -> Self {
        Value::Number(n as f64)
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::String(s.to_string())
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::String(s)
    }
}

impl From<Vec<Value>> for Value {
    fn from(items: Vec<Value>) -> Self {
        Value::array(items)
    }
}
