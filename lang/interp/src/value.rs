use std::fmt;
use std::rc::Rc;

use belsk2_syntax::ast::FnDecl;
use belsk2_syntax::{FloatKind, IntKind, Ty};

pub use crate::collections::{Array, Map};
use crate::env::EnvRef;

/// A runtime value.
#[derive(Clone)]
pub enum Value {
    Null,
    Bool(bool),
    /// An integer of the given type; always within that type's range.
    Int(i128, IntKind),
    /// A floating-point number; `float` values are rounded to 32 bits.
    Float(f64, FloatKind),
    Char(char),
    String(String),
    Array(Array),
    Map(Map),
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

    pub fn int(v: i32) -> Value {
        Value::Int(v.into(), IntKind::I32)
    }

    pub fn double(v: f64) -> Value {
        Value::Float(v, FloatKind::F64)
    }

    /// The runtime type.
    pub fn ty(&self) -> Ty {
        match self {
            Value::Null => Ty::Null,
            Value::Bool(_) => Ty::Bool,
            Value::Int(_, k) => Ty::Int(*k),
            Value::Float(_, k) => Ty::Float(*k),
            Value::Char(_) => Ty::Char,
            Value::String(_) => Ty::String,
            Value::Array(a) => Ty::array(a.elem_ty().clone()),
            Value::Map(m) => Ty::map(m.key_ty().clone(), m.value_ty().clone()),
            Value::Function(f) => belsk2_typeck::fn_type(&f.decl),
        }
    }

    /// The name of the runtime type, as reported by `type(x)`.
    pub fn type_name(&self) -> String {
        match self {
            Value::Function(_) => "fn".to_string(),
            v => v.ty().to_string(),
        }
    }

    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Int(i, _) => *i != 0,
            Value::Float(f, _) => *f != 0.0,
            Value::Char(c) => *c != '\0',
            Value::String(s) => !s.is_empty(),
            Value::Array(a) => !a.is_empty(),
            Value::Map(m) => !m.is_empty(),
            Value::Function(_) => true,
        }
    }

    /// The value as an `i64`, if it is an integer that fits.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i, _) => i64::try_from(*i).ok(),
            _ => None,
        }
    }

    /// The value as an `f64`, if it is a number.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(i, _) => Some(*i as f64),
            Value::Float(f, _) => Some(*f),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    /// Whether this value already has type `ty` (no conversion needed).
    pub fn fits(&self, ty: &Ty) -> bool {
        match (ty.canonical(), self) {
            (Ty::Any, _) => true,
            (t, Value::Null) => t.nullable(),
            (Ty::Bool, Value::Bool(_)) => true,
            (Ty::Int(k), Value::Int(_, vk)) => k == *vk,
            (Ty::Float(k), Value::Float(_, vk)) => k == *vk,
            (Ty::Char, Value::Char(_)) => true,
            (Ty::String, Value::String(_)) => true,
            (Ty::Param(_), _) => true,
            (Ty::Array(elem), Value::Array(a)) => {
                // An `any[]` may be used as a `T[]` if every element fits.
                elem.is_any()
                    || *a.elem_ty() == *elem
                    || (a.elem_ty().is_any()
                        && a.borrow()
                            .is_ok_and(|items| items.iter().all(|v| v.fits(&elem))))
            }
            (Ty::Map(k, v), Value::Map(m)) => {
                (k.is_any() || *m.key_ty() == *k) && (v.is_any() || *m.value_ty() == *v)
            }
            (Ty::Fn(_), Value::Function(_)) => true,
            _ => false,
        }
    }

    fn fmt_nested(&self, f: &mut fmt::Formatter<'_>, depth: usize) -> fmt::Result {
        match self {
            Value::Null => f.write_str("null"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Int(i, _) => write!(f, "{i}"),
            Value::Float(v, k) => f.write_str(&format_float(*v, *k)),
            Value::Char(c) => write!(f, "{c}"),
            Value::String(s) => f.write_str(s),
            Value::Function(func) => write!(f, "<fn {}>", func.decl.name),
            Value::Map(m) => {
                if depth >= MAX_NESTING {
                    return f.write_str("{...}");
                }
                let Ok(pairs) = m.pairs() else {
                    return f.write_str("{...}");
                };
                f.write_str("{")?;
                for (i, (k, v)) in pairs.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    k.fmt_nested(f, depth + 1)?;
                    f.write_str(": ")?;
                    v.fmt_nested(f, depth + 1)?;
                }
                f.write_str("}")
            }
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
            Value::Char(c) => write!(f, "{c:?}"),
            Value::Int(i, k) => write!(f, "{i}({})", k.name()),
            Value::Float(v, k) => write!(f, "{}({})", format_float(*v, *k), k.name()),
            other => write!(f, "{other}"),
        }
    }
}

/// Numbers compare by value across types (`1 == 1.0`); a one-character
/// string equals the same `char`.
impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        values_equal(self, other, 0)
    }
}

fn values_equal(a: &Value, b: &Value, depth: usize) -> bool {
    match (a, b) {
        (Value::Char(x), Value::Char(y)) => return x == y,
        _ => {
            if let Some(eq) = crate::num::num_eq(a, b) {
                return eq;
            }
        }
    }
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Char(c), Value::String(s)) | (Value::String(s), Value::Char(c)) => {
            let mut chars = s.chars();
            chars.next() == Some(*c) && chars.next().is_none()
        }
        (Value::Function(x), Value::Function(y)) => Rc::ptr_eq(x, y),
        (Value::Map(x), Value::Map(y)) => {
            if x.ptr_eq(y) {
                return true;
            }
            if depth >= MAX_NESTING {
                return false;
            }
            let (Ok(x), Ok(y)) = (x.pairs(), y.pairs()) else {
                return false;
            };
            x.len() == y.len()
                && x.iter().zip(y.iter()).all(|((ka, va), (kb, vb))| {
                    values_equal(ka, kb, depth + 1) && values_equal(va, vb, depth + 1)
                })
        }
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

/// Formats a floating-point number the way C# does by default: whole
/// numbers without a decimal point (`3`), the shortest text that reads back
/// as the same value otherwise (`0.1`, not `0.1000000015` for a `float`).
pub fn format_float(v: f64, kind: FloatKind) -> String {
    if v.is_nan() {
        return "NaN".to_string();
    }
    if v.is_infinite() {
        return if v > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    match kind {
        FloatKind::F32 => format!("{}", v as f32),
        FloatKind::F64 => format!("{v}"),
    }
}

impl From<i32> for Value {
    fn from(n: i32) -> Self {
        Value::int(n)
    }
}

impl From<i64> for Value {
    fn from(n: i64) -> Self {
        Value::Int(n.into(), IntKind::I64)
    }
}

impl From<f64> for Value {
    fn from(n: f64) -> Self {
        Value::double(n)
    }
}

impl From<f32> for Value {
    fn from(n: f32) -> Self {
        Value::Float(n.into(), FloatKind::F32)
    }
}

impl From<char> for Value {
    fn from(c: char) -> Self {
        Value::Char(c)
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
