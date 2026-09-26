use std::fmt;

/// The types supported so far (spec §7.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    Int,
    Float,
    Bool,
    Text,
    /// The type whose only value is `none`.
    None,
}

impl Type {
    pub fn is_numeric(self) -> bool {
        matches!(self, Type::Int | Type::Float)
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Type::Int => "Int",
            Type::Float => "Float",
            Type::Bool => "Bool",
            Type::Text => "Text",
            Type::None => "None",
        })
    }
}
