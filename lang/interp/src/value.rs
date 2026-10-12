use std::fmt;
use std::rc::Rc;

use belsk2_syntax::ast::{FnDecl, MemberKind, PropDecl, TypeDecl};
use belsk2_syntax::{FloatKind, IntKind, Ty, TypeRef, Virt};

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
    /// A function implemented in Rust (for example a stub for a function
    /// the host of a component provides).
    Native(Rc<Native>),
    /// A user-defined type used as a value: `Point` in `Point(1, 2)`.
    Type(Rc<Class>),
    /// A struct value. Structs are values: assigning one copies it, so
    /// the fields are never changed in place (a field assignment builds a
    /// new value).
    Struct(Rc<StructValue>),
    /// An object of a class. Classes are reference types: copies share
    /// the object.
    Object(Rc<Object>),
    /// `KeyValuePair<K, V>`: an entry of a dictionary.
    Pair(Rc<(Value, Value)>),
    /// A case of an enum (its index in [`Class::cases`]) and its data.
    Enum(Rc<Class>, usize, Rc<Vec<Value>>),
}

/// The fields of a class object.
pub struct Object {
    pub class: Rc<Class>,
    pub fields: std::cell::RefCell<Vec<Value>>,
    /// Its destructor ran already (or is running): it does not run again.
    pub finalized: std::cell::Cell<bool>,
}

impl Object {
    pub fn new(class: Rc<Class>, fields: Vec<Value>) -> Self {
        Object {
            class,
            fields: std::cell::RefCell::new(fields),
            finalized: std::cell::Cell::new(false),
        }
    }
}

/// A freed object with a destructor hands its fields to the interpreter,
/// which runs the destructor at the next statement.
impl Drop for Object {
    fn drop(&mut self) {
        if !self.finalized.get() && self.class.has_destructor() {
            let fields = std::mem::take(self.fields.get_mut());
            crate::gc::enqueue(Rc::clone(&self.class), fields);
        }
    }
}

/// A user-defined type at run time.
pub struct Class {
    pub decl: Rc<TypeDecl>,
    pub ty: TypeRef,
    /// Where its methods and field values are evaluated.
    pub env: EnvRef,
    /// The instance fields, in order.
    pub fields: Vec<(String, Ty)>,
    /// The cases of an enum and the numbers they stand for.
    pub cases: Vec<(String, i128)>,
    /// The fields of each case's data.
    pub case_fields: Vec<Vec<(String, Ty)>>,
    /// The static fields and their values.
    pub static_fields: Vec<(String, Ty)>,
    pub statics: std::cell::RefCell<Vec<Value>>,
    /// The base class, linked once every type of the block is defined.
    pub base: std::cell::OnceCell<Rc<Class>>,
}

impl Class {
    pub fn field_index(&self, name: &str) -> Option<usize> {
        self.fields.iter().position(|(n, _)| n == name)
    }

    pub fn static_index(&self, name: &str) -> Option<usize> {
        self.static_fields.iter().position(|(n, _)| n == name)
    }

    /// This class and its base classes, nearest first.
    pub fn chain(self: &Rc<Self>) -> Vec<Rc<Class>> {
        let mut out = vec![Rc::clone(self)];
        while let Some(base) = out.last().and_then(|c| c.base.get()).cloned() {
            if out.len() > 64 {
                break;
            }
            out.push(base);
        }
        out
    }

    /// The method `name` this class runs (its own, or inherited) and the
    /// class that declares it. Abstract methods have no code and are
    /// skipped.
    pub fn find_method(self: &Rc<Self>, name: &str) -> Option<(Rc<Class>, Rc<FnDecl>)> {
        self.chain().into_iter().find_map(|c| {
            let decl = c.decl.members.iter().find_map(|m| match &m.kind {
                MemberKind::Method(d) if d.name == name && m.mods.virt != Virt::Abstract => {
                    Some(Rc::clone(d))
                }
                _ => None,
            })?;
            Some((c, decl))
        })
    }

    /// The property `name` (its own, or inherited) and the class that
    /// declares it.
    pub fn find_prop(self: &Rc<Self>, name: &str) -> Option<(Rc<Class>, PropDecl)> {
        self.chain().into_iter().find_map(|c| {
            let prop = c.decl.members.iter().find_map(|m| match &m.kind {
                MemberKind::Prop(p) if p.name == name => Some((**p).clone()),
                _ => None,
            })?;
            Some((c, prop))
        })
    }

    /// The destructors to run for an object of this class: its own, then
    /// its base classes'.
    pub fn destructors(self: &Rc<Self>) -> Vec<(Rc<Class>, Rc<FnDecl>)> {
        self.chain()
            .into_iter()
            .filter_map(|c| {
                let d = c.decl.members.iter().find_map(|m| match &m.kind {
                    MemberKind::Drop(d) => Some(Rc::clone(d)),
                    _ => None,
                })?;
                Some((c, d))
            })
            .collect()
    }

    pub fn has_destructor(&self) -> bool {
        let mut cur = Some(self);
        let mut depth = 0;
        while let Some(c) = cur {
            if c.decl
                .members
                .iter()
                .any(|m| matches!(m.kind, MemberKind::Drop(_)))
            {
                return true;
            }
            depth += 1;
            if depth > 64 {
                break;
            }
            cur = c.base.get().map(|b| &**b);
        }
        false
    }

    pub fn init(&self) -> Option<&Rc<FnDecl>> {
        self.decl.members.iter().find_map(|m| match &m.kind {
            MemberKind::Init(d) => Some(d),
            _ => None,
        })
    }
}

/// The fields of a struct value.
#[derive(Clone)]
pub struct StructValue {
    pub class: Rc<Class>,
    pub fields: Vec<Value>,
}

/// The Rust implementation of a [`Value::Native`].
pub type NativeFn = dyn Fn(Vec<Value>, &mut dyn std::io::Write) -> belsk2_syntax::Result<Value>;

/// A function implemented in Rust.
pub struct Native {
    pub name: String,
    /// Its static type (`Ty::Fn`).
    pub ty: Ty,
    pub func: Box<NativeFn>,
}

/// A user-defined function together with the environment it closes over.
pub struct Function {
    pub decl: Rc<FnDecl>,
    pub env: EnvRef,
}

/// Values nested deeper than this are not printed or compared further.
/// It also stops runaway recursion on arrays that contain themselves.
const MAX_NESTING: usize = 64;
/// Objects nested deeper than this are printed as `Name {...}`.
const MAX_OBJECT_NESTING: usize = 8;

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
            Value::Native(n) => n.ty.clone(),
            Value::Type(c) => Ty::Type(c.ty.clone()),
            Value::Struct(s) => Ty::Named(s.class.ty.clone()),
            Value::Object(o) => Ty::Named(o.class.ty.clone()),
            Value::Enum(c, ..) => Ty::Named(c.ty.clone()),
            Value::Pair(p) => Ty::key_value(p.0.ty(), p.1.ty()),
        }
    }

    /// The name of the runtime type, as reported by `type(x)`.
    pub fn type_name(&self) -> String {
        match self {
            Value::Function(_) | Value::Native(_) => "fn".to_string(),
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
            Value::Function(_)
            | Value::Native(_)
            | Value::Type(_)
            | Value::Struct(_)
            | Value::Object(_)
            | Value::Pair(_)
            | Value::Enum(..) => true,
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
            (Ty::Fn(_), Value::Function(_) | Value::Native(_)) => true,
            (Ty::Named(t), Value::Struct(s)) => s.class.ty.is_subtype_of(&t),
            (Ty::Named(t), Value::Object(o)) => o.class.ty.is_subtype_of(&t),
            (Ty::Named(t), Value::Enum(c, ..)) => c.ty.is_subtype_of(&t),
            (Ty::KeyValue(k, v), Value::Pair(p)) => p.0.fits(&k) && p.1.fits(&v),
            (Ty::Type(t), Value::Type(c)) => c.ty == t,
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
            Value::Native(n) => write!(f, "<fn {}>", n.name),
            Value::Type(c) => write!(f, "<type {}>", c.ty.name),
            Value::Struct(s) => fmt_fields(f, &s.class, &s.fields, depth),
            // Objects may refer to each other in cycles: show a few levels.
            Value::Object(o) if depth >= MAX_OBJECT_NESTING => {
                write!(f, "{} {{...}}", o.class.ty.name)
            }
            Value::Object(o) => match o.fields.try_borrow() {
                Ok(fields) => fmt_fields(f, &o.class, &fields, depth),
                Err(_) => write!(f, "{} {{...}}", o.class.ty.name),
            },
            // As C# prints a KeyValuePair.
            Value::Pair(p) => {
                if depth >= MAX_NESTING {
                    return f.write_str("[...]");
                }
                f.write_str("[")?;
                p.0.fmt_nested(f, depth + 1)?;
                f.write_str(", ")?;
                p.1.fmt_nested(f, depth + 1)?;
                f.write_str("]")
            }
            Value::Enum(c, i, data) => {
                f.write_str(c.cases.get(*i).map(|(n, _)| n.as_str()).unwrap_or("?"))?;
                if data.is_empty() {
                    return Ok(());
                }
                if depth >= MAX_NESTING {
                    return f.write_str("(...)");
                }
                f.write_str("(")?;
                for (k, v) in data.iter().enumerate() {
                    if k > 0 {
                        f.write_str(", ")?;
                    }
                    v.fmt_nested(f, depth + 1)?;
                }
                f.write_str(")")
            }
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

/// `Point { x: 1, y: 2 }`
fn fmt_fields(
    f: &mut fmt::Formatter<'_>,
    class: &Class,
    fields: &[Value],
    depth: usize,
) -> fmt::Result {
    if depth >= MAX_NESTING {
        return write!(f, "{} {{...}}", class.ty.name);
    }
    write!(f, "{} {{", class.ty.name)?;
    for (i, ((name, _), v)) in class.fields.iter().zip(fields).enumerate() {
        f.write_str(if i > 0 { ", " } else { " " })?;
        write!(f, "{name}: ")?;
        v.fmt_nested(f, depth + 1)?;
    }
    f.write_str(if fields.is_empty() { "}" } else { " }" })
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
        (Value::Native(x), Value::Native(y)) => Rc::ptr_eq(x, y),
        (Value::Type(x), Value::Type(y)) => x.ty == y.ty,
        (Value::Object(x), Value::Object(y)) => Rc::ptr_eq(x, y),
        (Value::Pair(x), Value::Pair(y)) => {
            depth < MAX_NESTING
                && values_equal(&x.0, &y.0, depth + 1)
                && values_equal(&x.1, &y.1, depth + 1)
        }
        (Value::Enum(x, i, a), Value::Enum(y, j, b)) => {
            x.ty == y.ty
                && i == j
                && depth < MAX_NESTING
                && a.iter()
                    .zip(b.iter())
                    .all(|(a, b)| values_equal(a, b, depth + 1))
        }
        (Value::Struct(x), Value::Struct(y)) => {
            if Rc::ptr_eq(x, y) {
                return true;
            }
            if depth >= MAX_NESTING || x.class.ty != y.class.ty {
                return false;
            }
            x.fields
                .iter()
                .zip(&y.fields)
                .all(|(a, b)| values_equal(a, b, depth + 1))
        }
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
