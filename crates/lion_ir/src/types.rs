use std::collections::HashMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

/// The types supported so far (spec §7.2). Types that contain other types refer to
/// them through a [`TypeRef`], so that `Type` stays small and `Copy`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
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
    /// A failure that the program must handle (§18.1). For now, the errors made by
    /// `error(...)` and by the standard library.
    Error,
    /// `A or B` (§7.3): at least two members, none of them a union, in a fixed order.
    Union(UnionRef),
}

impl Type {
    pub fn is_numeric(self) -> bool {
        matches!(self, Type::Int | Type::Float)
    }

    pub fn list(element: Type) -> Type {
        Type::List(TypeRef::new(element))
    }

    /// `A or B or ...`, flattened, without repetitions; a single member is that member.
    pub fn union(members: impl IntoIterator<Item = Type>) -> Type {
        let mut flat: Vec<Type> = Vec::new();
        for member in members {
            match member {
                Type::Union(union) => flat.extend(union.members()),
                other => flat.push(other),
            }
        }
        flat.sort();
        flat.dedup();
        match flat.as_slice() {
            [single] => *single,
            _ => Type::Union(UnionRef::new(flat)),
        }
    }

    /// `maybe T`: `T or None` (§7.3, D13).
    pub fn maybe(ty: Type) -> Type {
        Type::union([ty, Type::None])
    }

    /// The members of a union, or the type itself.
    pub fn members(self) -> Vec<Type> {
        match self {
            Type::Union(union) => union.members(),
            other => vec![other],
        }
    }

    /// Whether every value of `self` is a value of `other`.
    pub fn is_subset_of(self, other: Type) -> bool {
        let others = other.members();
        self.members().iter().all(|member| others.contains(member))
    }

    /// The members of `self` that are not in `other`; `None` when nothing remains.
    pub fn without(self, other: Type) -> Option<Type> {
        let removed = other.members();
        let rest: Vec<Type> = self.members().into_iter().filter(|member| !removed.contains(member)).collect();
        (!rest.is_empty()).then(|| Type::union(rest))
    }

    /// The members of `self` that are in `other`; `None` when there are none.
    pub fn intersection(self, other: Type) -> Option<Type> {
        let kept = other.members();
        let common: Vec<Type> = self.members().into_iter().filter(|member| kept.contains(member)).collect();
        (!common.is_empty()).then(|| Type::union(common))
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
            // `of` nests to the right and binds more tightly than `or` (§15.1, D34).
            Type::List(element) => match element.get() {
                union @ Type::Union(_) => write!(f, "List of ({union})"),
                element => write!(f, "List of {element}"),
            },
            Type::Error => f.write_str("Error"),
            Type::Union(union) => {
                let members = union.members();
                // `maybe T` reads better than `T or None` (§7.3).
                if members.len() == 2 && members.contains(&Type::None) {
                    let other = members.iter().find(|member| **member != Type::None).expect("two members");
                    return write!(f, "maybe {other}");
                }
                let names: Vec<String> = members.iter().map(Type::to_string).collect();
                f.write_str(&names.join(" or "))
            }
        }
    }
}

/// The members of a union type, stored once for the whole process.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UnionRef(u32);

impl UnionRef {
    fn new(members: Vec<Type>) -> UnionRef {
        let mut interner = interner().lock().expect("the type interner is never poisoned");
        if let Some(&id) = interner.union_ids.get(&members) {
            return UnionRef(id);
        }
        let id = interner.unions.len() as u32;
        interner.unions.push(members.clone());
        interner.union_ids.insert(members, id);
        UnionRef(id)
    }

    pub fn members(self) -> Vec<Type> {
        interner().lock().expect("the type interner is never poisoned").unions[self.0 as usize].clone()
    }
}

impl fmt::Debug for UnionRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.members())
    }
}

/// A type contained in another type, stored once for the whole process: two equal
/// types have the same reference.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TypeRef(u32);

#[derive(Default)]
struct Interner {
    types: Vec<Type>,
    ids: HashMap<Type, u32>,
    unions: Vec<Vec<Type>>,
    union_ids: HashMap<Vec<Type>, u32>,
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

    #[test]
    fn unions_are_flat_and_ordered() {
        let a = Type::union([Type::Text, Type::Int]);
        let b = Type::union([Type::Int, Type::Text, Type::Int]);
        assert_eq!(a, b);
        assert_eq!(Type::union([a, Type::None]), Type::union([Type::None, Type::Int, Type::Text]));
        assert_eq!(Type::maybe(Type::maybe(Type::Int)), Type::maybe(Type::Int));
        assert_eq!(Type::union([Type::Int]), Type::Int);
        assert_eq!(Type::maybe(Type::Int).to_string(), "maybe Int");
        assert_eq!(Type::union([Type::Int, Type::Error]).to_string(), "Int or Error");
        assert_eq!(Type::list(Type::maybe(Type::Int)).to_string(), "List of (maybe Int)");
        assert_eq!(Type::maybe(Type::Int).without(Type::None), Some(Type::Int));
        assert!(Type::Int.is_subset_of(Type::maybe(Type::Int)));
        assert_eq!(Type::union([Type::Int, Type::Text]).intersection(Type::Text), Some(Type::Text));
    }
}
