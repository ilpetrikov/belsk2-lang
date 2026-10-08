/// A type that can be written in a declaration (`int x = 1`, `var x: int = 1`,
/// `fn f(a: string): bool`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BType {
    Any,
    Int,
    Float,
    String,
    Bool,
    Array,
    Fn,
    /// A number that may not exceed 1000.
    Bel,
    /// A string (kept for compatibility with older code).
    Ster,
}

impl BType {
    pub fn name(self) -> &'static str {
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

    pub fn from_name(name: &str) -> Option<BType> {
        Some(match name {
            "any" => BType::Any,
            "int" => BType::Int,
            "float" => BType::Float,
            "string" => BType::String,
            "bool" => BType::Bool,
            "array" => BType::Array,
            "fn" => BType::Fn,
            "bel" => BType::Bel,
            "ster" => BType::Ster,
            _ => return None,
        })
    }

    pub fn is_numeric(self) -> bool {
        matches!(self, BType::Int | BType::Float | BType::Bel)
    }
}

impl std::fmt::Display for BType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}
