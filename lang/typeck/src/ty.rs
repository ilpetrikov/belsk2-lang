use std::fmt;
use std::rc::Rc;

use belsk2_syntax::BType;

/// The static type of an expression.
#[derive(Debug, Clone, PartialEq)]
pub enum Ty {
    /// Unknown until run time; compatible with everything.
    Any,
    Null,
    Bool,
    Number,
    String,
    Array,
    /// A function; the signature is known for named declarations.
    Fn(Option<Rc<Sig>>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sig {
    pub name: String,
    pub params: Vec<Ty>,
    pub ret: Ty,
}

impl Ty {
    pub fn from_btype(b: BType) -> Ty {
        match b {
            BType::Any => Ty::Any,
            BType::Int | BType::Float | BType::Bel => Ty::Number,
            BType::String | BType::Ster => Ty::String,
            BType::Bool => Ty::Bool,
            BType::Array => Ty::Array,
            BType::Fn => Ty::Fn(None),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Ty::Any => "any",
            Ty::Null => "null",
            Ty::Bool => "bool",
            Ty::Number => "number",
            Ty::String => "string",
            Ty::Array => "array",
            Ty::Fn(_) => "fn",
        }
    }

    pub fn is(&self, other: &Ty) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }

    pub fn is_any(&self) -> bool {
        matches!(self, Ty::Any)
    }

    /// `self` is `Any` or one of `options`.
    pub fn one_of(&self, options: &[Ty]) -> bool {
        self.is_any() || options.iter().any(|o| self.is(o))
    }

    /// Whether a value of type `src` may be stored where `self` is expected.
    pub fn accepts(&self, src: &Ty) -> bool {
        match (self, src) {
            (Ty::Any, _) | (_, Ty::Any) | (_, Ty::Null) => true,
            (a, b) => a.is(b),
        }
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}
