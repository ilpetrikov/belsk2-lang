#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BType {
    Any,
    Int,
    Float,
    String,
    Bool,
    Array,
    Fn,
    Bel,
    Ster,
}

impl BType {
    pub fn name(&self) -> &'static str {
        match self {
            BType::Any => "any",
            BType::Int => "int",
            BType::Float => "float",
            BType::String => "string",
            BType::Bool => "bool",
            BType::Array => "array",
            BType::Fn => "fn",
            BType::Bel => "bel",
            BType::Ster => "ster",
        }
    }
}

pub fn is_numeric_type(t: BType) -> bool {
    t == BType::Int || t == BType::Float || t == BType::Bel
}

pub fn type_compatible(declared: BType, actual: BType) -> bool {
    if declared == BType::Any || actual == BType::Any {
        return true;
    }
    if declared == actual {
        return true;
    }
    if is_numeric_type(declared) && is_numeric_type(actual) {
        return true;
    }
    if declared == BType::Ster && actual == BType::String {
        return true;
    }
    if declared == BType::String && actual == BType::Ster {
        return true;
    }
    false
}
