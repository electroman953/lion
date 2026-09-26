use std::collections::HashMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

/// The types supported so far (spec §7.2). Types that contain other types refer to
/// them through a [`TypeRef`], so that `Type` stays small and `Copy`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    Int,
    Float,
    Bool,
    Text,
    /// The type whose only value is `none`.
    None,
    /// `a..b`: increasing integers, never stored element by element (§16.3, D46).
    Range,
    /// `List of T` (§16.1).
    List(TypeRef),
}

impl Type {
    pub fn is_numeric(self) -> bool {
        matches!(self, Type::Int | Type::Float)
    }

    pub fn list(element: Type) -> Type {
        Type::List(TypeRef::new(element))
    }

    /// The type of the elements met when going through a value of this type with `for`.
    pub fn element(self) -> Option<Type> {
        match self {
            Type::Range => Some(Type::Int),
            Type::List(element) => Some(element.get()),
            _ => None,
        }
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Int => f.write_str("Int"),
            Type::Float => f.write_str("Float"),
            Type::Bool => f.write_str("Bool"),
            Type::Text => f.write_str("Text"),
            Type::None => f.write_str("None"),
            Type::Range => f.write_str("Range"),
            // `of` nests to the right: `List of List of Int` (§15.1).
            Type::List(element) => write!(f, "List of {}", element.get()),
        }
    }
}

/// A type contained in another type, stored once for the whole process: two equal
/// types have the same reference.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeRef(u32);

#[derive(Default)]
struct Interner {
    types: Vec<Type>,
    ids: HashMap<Type, u32>,
}

fn interner() -> &'static Mutex<Interner> {
    static INTERNER: OnceLock<Mutex<Interner>> = OnceLock::new();
    INTERNER.get_or_init(Mutex::default)
}

impl TypeRef {
    pub fn new(ty: Type) -> TypeRef {
        let mut interner = interner().lock().expect("the type interner is never poisoned");
        if let Some(&id) = interner.ids.get(&ty) {
            return TypeRef(id);
        }
        let id = interner.types.len() as u32;
        interner.types.push(ty);
        interner.ids.insert(ty, id);
        TypeRef(id)
    }

    pub fn get(self) -> Type {
        interner().lock().expect("the type interner is never poisoned").types[self.0 as usize]
    }
}

impl fmt::Debug for TypeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.get())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_types_share_their_reference() {
        let a = Type::list(Type::list(Type::Int));
        let b = Type::list(Type::list(Type::Int));
        assert_eq!(a, b);
        assert_ne!(a, Type::list(Type::Int));
        assert_eq!(a.to_string(), "List of List of Int");
        assert_eq!(a.element(), Some(Type::list(Type::Int)));
        assert_eq!(Type::Range.element(), Some(Type::Int));
    }
}
