//! The type model shared by the checker, the interpreter and (later) the
//! compiler.

use std::fmt;
use std::rc::Rc;

use crate::span::Span;

/// A fixed-width integer type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IntKind {
    /// `sbyte`
    I8,
    /// `byte`
    U8,
    /// `short`
    I16,
    /// `ushort`
    U16,
    /// `int`
    I32,
    /// `uint`
    U32,
    /// `long`
    I64,
    /// `ulong`
    U64,
}

impl IntKind {
    pub const ALL: [IntKind; 8] = [
        IntKind::I8,
        IntKind::U8,
        IntKind::I16,
        IntKind::U16,
        IntKind::I32,
        IntKind::U32,
        IntKind::I64,
        IntKind::U64,
    ];

    pub fn name(self) -> &'static str {
        match self {
            IntKind::I8 => "sbyte",
            IntKind::U8 => "byte",
            IntKind::I16 => "short",
            IntKind::U16 => "ushort",
            IntKind::I32 => "int",
            IntKind::U32 => "uint",
            IntKind::I64 => "long",
            IntKind::U64 => "ulong",
        }
    }

    pub fn signed(self) -> bool {
        matches!(
            self,
            IntKind::I8 | IntKind::I16 | IntKind::I32 | IntKind::I64
        )
    }

    pub fn bits(self) -> u32 {
        match self {
            IntKind::I8 | IntKind::U8 => 8,
            IntKind::I16 | IntKind::U16 => 16,
            IntKind::I32 | IntKind::U32 => 32,
            IntKind::I64 | IntKind::U64 => 64,
        }
    }

    pub fn min(self) -> i128 {
        match self {
            IntKind::I8 => i8::MIN.into(),
            IntKind::I16 => i16::MIN.into(),
            IntKind::I32 => i32::MIN.into(),
            IntKind::I64 => i64::MIN.into(),
            _ => 0,
        }
    }

    pub fn max(self) -> i128 {
        match self {
            IntKind::I8 => i8::MAX.into(),
            IntKind::U8 => u8::MAX.into(),
            IntKind::I16 => i16::MAX.into(),
            IntKind::U16 => u16::MAX.into(),
            IntKind::I32 => i32::MAX.into(),
            IntKind::U32 => u32::MAX.into(),
            IntKind::I64 => i64::MAX.into(),
            IntKind::U64 => u64::MAX.into(),
        }
    }

    pub fn fits(self, v: i128) -> bool {
        v >= self.min() && v <= self.max()
    }

    /// Reduces `v` to this width, wrapping around like C# unchecked
    /// arithmetic.
    pub fn wrap(self, v: i128) -> i128 {
        match self {
            IntKind::I8 => (v as i8).into(),
            IntKind::U8 => (v as u8).into(),
            IntKind::I16 => (v as i16).into(),
            IntKind::U16 => (v as u16).into(),
            IntKind::I32 => (v as i32).into(),
            IntKind::U32 => (v as u32).into(),
            IntKind::I64 => (v as i64).into(),
            IntKind::U64 => (v as u64).into(),
        }
    }
}

/// A floating-point type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FloatKind {
    /// `float`
    F32,
    /// `double`
    F64,
}

impl FloatKind {
    pub fn name(self) -> &'static str {
        match self {
            FloatKind::F32 => "float",
            FloatKind::F64 => "double",
        }
    }

    /// Rounds `v` to this precision.
    pub fn round(self, v: f64) -> f64 {
        match self {
            FloatKind::F32 => (v as f32).into(),
            FloatKind::F64 => v,
        }
    }
}

/// A numeric type: the part of [`Ty`] that arithmetic works on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NumTy {
    Int(IntKind),
    Float(FloatKind),
}

impl NumTy {
    pub fn ty(self) -> Ty {
        match self {
            NumTy::Int(k) => Ty::Int(k),
            NumTy::Float(k) => Ty::Float(k),
        }
    }

    /// The C# implicit (widening) numeric conversions.
    pub fn widens_to(self, to: NumTy) -> bool {
        use FloatKind::*;
        use IntKind::*;
        use NumTy::{Float as F, Int as I};
        if self == to {
            return true;
        }
        match (self, to) {
            (_, F(F64)) => true,
            (F(_), _) => false,
            (_, F(F32)) => true,
            (I(I8), I(I16 | I32 | I64)) => true,
            (I(U8), I(I16 | U16 | I32 | U32 | I64 | U64)) => true,
            (I(I16), I(I32 | I64)) => true,
            (I(U16), I(I32 | U32 | I64 | U64)) => true,
            (I(I32), I(I64)) => true,
            (I(U32), I(I64 | U64)) => true,
            _ => false,
        }
    }

    /// The type both operands of a binary operator are converted to
    /// (C# binary numeric promotion). `None` if there is none, as for
    /// `ulong` combined with a signed type.
    pub fn promote(a: NumTy, b: NumTy) -> Option<NumTy> {
        use IntKind::*;
        let (a, b) = match (a, b) {
            (NumTy::Float(x), NumTy::Float(y)) => return Some(NumTy::Float(x.max(y))),
            (NumTy::Float(x), _) | (_, NumTy::Float(x)) => return Some(NumTy::Float(x)),
            (NumTy::Int(a), NumTy::Int(b)) => (a, b),
        };
        let has = |k: IntKind| a == k || b == k;
        let other_signed = |k: IntKind| (a == k && b.signed()) || (b == k && a.signed());
        Some(NumTy::Int(if has(U64) {
            if other_signed(U64) {
                return None;
            }
            U64
        } else if has(I64) {
            I64
        } else if has(U32) {
            if other_signed(U32) {
                I64
            } else {
                U32
            }
        } else {
            I32
        }))
    }

    /// The result type of unary `-`.
    pub fn negated(self) -> Option<NumTy> {
        match self {
            NumTy::Float(_) => Some(self),
            NumTy::Int(IntKind::U64) => None,
            NumTy::Int(IntKind::U32 | IntKind::I64) => Some(NumTy::Int(IntKind::I64)),
            NumTy::Int(_) => Some(NumTy::Int(IntKind::I32)),
        }
    }

    /// Small integer types are widened to `int` before arithmetic.
    pub fn int_promoted(k: IntKind) -> IntKind {
        match k {
            IntKind::I8 | IntKind::U8 | IntKind::I16 | IntKind::U16 => IntKind::I32,
            k => k,
        }
    }
}

/// The static type of a value.
#[derive(Debug, Clone, PartialEq)]
pub enum Ty {
    /// Unknown until run time; compatible with everything and checked when
    /// the program runs.
    Any,
    /// The type of the `null` literal.
    Null,
    Bool,
    Int(IntKind),
    Float(FloatKind),
    Char,
    String,
    /// A `double` that may not exceed 1000.
    Bel,
    /// A `string` that may not be `null`.
    Ster,
    /// An array with elements of the given type.
    Array(Rc<Ty>),
    /// `Dictionary<K, V>`: a hash map that keeps insertion order.
    Map(Rc<Ty>, Rc<Ty>),
    /// A type parameter of a generic function (`T` in `fn f<T>(x: T)`).
    Param(Rc<str>),
    /// A function; the signature is known for named declarations.
    Fn(Option<Rc<Sig>>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sig {
    pub name: String,
    /// Type parameters of a generic function, in declaration order.
    pub type_params: Vec<Rc<str>>,
    pub params: Vec<Ty>,
    pub ret: Ty,
}

impl Ty {
    pub const INT: Ty = Ty::Int(IntKind::I32);
    pub const DOUBLE: Ty = Ty::Float(FloatKind::F64);

    pub fn array(elem: Ty) -> Ty {
        Ty::Array(Rc::new(elem))
    }

    pub fn map(key: Ty, value: Ty) -> Ty {
        Ty::Map(Rc::new(key), Rc::new(value))
    }

    /// Whether values of this type can be dictionary keys.
    pub fn is_key(&self) -> bool {
        matches!(
            self.canonical(),
            Ty::Any | Ty::Bool | Ty::Int(_) | Ty::Char | Ty::String | Ty::Param(_)
        )
    }

    /// Whether this type mentions a type parameter.
    pub fn has_params(&self) -> bool {
        match self {
            Ty::Param(_) => true,
            Ty::Array(elem) => elem.has_params(),
            Ty::Map(k, v) => k.has_params() || v.has_params(),
            _ => false,
        }
    }

    /// Replaces type parameters using `subst`.
    pub fn substitute(&self, subst: &dyn Fn(&str) -> Option<Ty>) -> Ty {
        match self {
            Ty::Param(name) => subst(name).unwrap_or_else(|| self.clone()),
            Ty::Array(elem) => Ty::array(elem.substitute(subst)),
            Ty::Map(k, v) => Ty::map(k.substitute(subst), v.substitute(subst)),
            t => t.clone(),
        }
    }

    /// The type named by a built-in type keyword.
    pub fn from_name(name: &str) -> Option<Ty> {
        Some(match name {
            "any" | "object" => Ty::Any,
            "bool" => Ty::Bool,
            "sbyte" => Ty::Int(IntKind::I8),
            "byte" => Ty::Int(IntKind::U8),
            "short" => Ty::Int(IntKind::I16),
            "ushort" => Ty::Int(IntKind::U16),
            "int" => Ty::Int(IntKind::I32),
            "uint" => Ty::Int(IntKind::U32),
            "long" => Ty::Int(IntKind::I64),
            "ulong" => Ty::Int(IntKind::U64),
            "float" => Ty::Float(FloatKind::F32),
            "double" => Ty::Float(FloatKind::F64),
            "char" => Ty::Char,
            "string" => Ty::String,
            "bel" => Ty::Bel,
            "ster" => Ty::Ster,
            "array" => Ty::array(Ty::Any),
            "fn" => Ty::Fn(None),
            _ => return None,
        })
    }

    /// The numeric type used for arithmetic, if this is a number.
    pub fn num(&self) -> Option<NumTy> {
        match self {
            Ty::Int(k) => Some(NumTy::Int(*k)),
            Ty::Float(k) => Some(NumTy::Float(*k)),
            Ty::Bel => Some(NumTy::Float(FloatKind::F64)),
            _ => None,
        }
    }

    /// `bel` behaves as `double` and `ster` as `string`.
    pub fn canonical(&self) -> Ty {
        match self {
            Ty::Bel => Ty::DOUBLE,
            Ty::Ster => Ty::String,
            t => t.clone(),
        }
    }

    pub fn is_any(&self) -> bool {
        matches!(self, Ty::Any)
    }

    /// Whether `null` is a valid value of this type. Numbers, `bool` and
    /// `char` are value types and are never null, as in C#.
    pub fn nullable(&self) -> bool {
        matches!(
            self,
            Ty::Any | Ty::Null | Ty::String | Ty::Array(_) | Ty::Map(..) | Ty::Fn(_) | Ty::Param(_)
        )
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::Any => f.write_str("any"),
            Ty::Null => f.write_str("null"),
            Ty::Bool => f.write_str("bool"),
            Ty::Int(k) => f.write_str(k.name()),
            Ty::Float(k) => f.write_str(k.name()),
            Ty::Char => f.write_str("char"),
            Ty::String => f.write_str("string"),
            Ty::Bel => f.write_str("bel"),
            Ty::Ster => f.write_str("ster"),
            Ty::Array(elem) => write!(f, "{elem}[]"),
            Ty::Map(k, v) => write!(f, "Dictionary<{k}, {v}>"),
            Ty::Param(name) => f.write_str(name),
            Ty::Fn(_) => f.write_str("fn"),
        }
    }
}

/// A type as written in source code, e.g. `int`, `string[]`.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeExpr {
    pub kind: TypeExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeExprKind {
    /// `int`, `List<int>`
    Named { name: String, args: Vec<TypeExpr> },
    /// `T[]`
    Array(Box<TypeExpr>),
}

impl fmt::Display for TypeExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            TypeExprKind::Named { name, args } => {
                f.write_str(name)?;
                if !args.is_empty() {
                    f.write_str("<")?;
                    for (i, a) in args.iter().enumerate() {
                        if i > 0 {
                            f.write_str(", ")?;
                        }
                        write!(f, "{a}")?;
                    }
                    f.write_str(">")?;
                }
                Ok(())
            }
            TypeExprKind::Array(elem) => write!(f, "{elem}[]"),
        }
    }
}
