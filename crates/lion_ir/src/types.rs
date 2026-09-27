use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Mutex, OnceLock};

/// The types supported so far (spec §7.2). Types that contain other types refer to
/// them through a [`TypeRef`], so that `Type` stays small and `Copy`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Type {
    Int,
    Float,
    /// An exact fraction, always simplified (§8.3).
    Rational,
    Bool,
    Text,
    /// The type whose only value is `none`.
    None,
    /// `a..b`: increasing integers, never stored element by element (§16.3, D46).
    Range,
    /// `List of T` (§16.1).
    List(TypeRef),
    /// `Set of T`: without order nor repetition (§16.1).
    Set(TypeRef),
    /// `Domain of T`: the values of `T` that have a property, as `{x in Int, x > 0}`;
    /// only tested for membership (§16.5).
    Domain(TypeRef),
    /// `Map of (K, V)`: values found by their keys, which are unique (§16.1, C79). The
    /// tuple holds the type of the keys, then the type of the values.
    Map(TupleRef),
    /// `Task of T`: a computation that gives a `T` (§19.1).
    Task(TypeRef),
    /// `(A, B)`: a tuple, whose elements have these types (§4.5).
    Tuple(TupleRef),
    /// A failure that the program must handle (§18.1). For now, the errors made by
    /// `error(...)` and by the standard library.
    Error,
    /// `A or B` (§7.3): at least two members, none of them a union, in a fixed order.
    Union(UnionRef),
    /// A structure declared by the program (§12).
    Struct(StructRef),
    /// An enumeration declared by the program (§13.1).
    Enum(EnumRef),
    /// `fun(A, B) in R`: a function used as a value (§7.2, D63).
    Fun(FunRef),
    /// A trait: the set of the types that satisfy it (§14). Its members are known once
    /// every method of the program is.
    Trait(TraitRef),
    /// A type variable of a generic function, as `T` in `fun biggest(a in T, b in T) in
    /// T, T in Comparable` (§15.2). It appears only in signatures: each instance
    /// replaces it.
    Var(VarRef),
    /// A generic structure written with type variables, as `Pair of (T, U)` in the
    /// signature of a generic function (§15.1). Like `Var`, it appears only in
    /// signatures: each instance replaces it with the structure made for its types.
    Applied(AppliedRef),
}

impl Type {
    pub fn is_numeric(self) -> bool {
        matches!(self, Type::Int | Type::Float | Type::Rational)
    }

    pub fn list(element: Type) -> Type {
        Type::List(TypeRef::new(element))
    }

    pub fn set(element: Type) -> Type {
        Type::Set(TypeRef::new(element))
    }

    pub fn domain(element: Type) -> Type {
        Type::Domain(TypeRef::new(element))
    }

    pub fn map(key: Type, value: Type) -> Type {
        Type::Map(TupleRef::new(vec![key, value]))
    }

    /// The type of the keys and the type of the values of a Map.
    pub fn map_parts(self) -> Option<(Type, Type)> {
        match self {
            Type::Map(parts) => match parts.elements().as_slice() {
                [key, value] => Some((*key, *value)),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn tuple(elements: Vec<Type>) -> Type {
        Type::Tuple(TupleRef::new(elements))
    }

    /// `fun(params) in ret`, of which the first `required` parameters must be given.
    pub fn function(params: Vec<Type>, required: usize, ret: Type) -> Type {
        Type::Fun(FunRef::new(FunData { params, required: required as u32, ret }))
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

    /// The members of a union, or the type itself; a trait stands for the types that
    /// satisfy it.
    pub fn members(self) -> Vec<Type> {
        match self {
            Type::Union(union) => {
                let mut members: Vec<Type> = union.members().into_iter().flat_map(Type::members).collect();
                members.sort();
                members.dedup();
                members
            }
            Type::Trait(set) => {
                let members = set.members();
                if members.is_empty() { vec![self] } else { members }
            }
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
            Type::List(element) | Type::Set(element) => Some(element.get()),
            // A Map is gone through by its keys (C79).
            Type::Map(_) => self.map_parts().map(|(key, _)| key),
            _ => None,
        }
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Int => f.write_str("Int"),
            Type::Float => f.write_str("Float"),
            Type::Rational => f.write_str("Rational"),
            Type::Bool => f.write_str("Bool"),
            Type::Text => f.write_str("Text"),
            Type::None => f.write_str("None"),
            Type::Range => f.write_str("Range"),
            // `of` nests to the right and binds more tightly than `or` (§15.1, D34).
            Type::List(element) => match element.get() {
                union @ Type::Union(_) => write!(f, "List of ({union})"),
                element => write!(f, "List of {element}"),
            },
            Type::Set(element) => match element.get() {
                union @ Type::Union(_) => write!(f, "Set of ({union})"),
                element => write!(f, "Set of {element}"),
            },
            Type::Domain(element) => match element.get() {
                union @ Type::Union(_) => write!(f, "Domain of ({union})"),
                element => write!(f, "Domain of {element}"),
            },
            Type::Map(parts) => {
                let parts: Vec<String> = parts.elements().iter().map(Type::to_string).collect();
                write!(f, "Map of ({})", parts.join(", "))
            }
            Type::Task(result) => match result.get() {
                union @ Type::Union(_) => write!(f, "Task of ({union})"),
                result => write!(f, "Task of {result}"),
            },
            Type::Tuple(tuple) => {
                let elements: Vec<String> = tuple.elements().iter().map(Type::to_string).collect();
                match elements.as_slice() {
                    [single] => write!(f, "({single},)"),
                    _ => write!(f, "({})", elements.join(", ")),
                }
            }
            Type::Error => f.write_str("Error"),
            Type::Struct(structure) => f.write_str(&structure.name()),
            Type::Enum(enumeration) => f.write_str(&enumeration.name()),
            Type::Trait(set) => f.write_str(&set.name()),
            Type::Var(var) => f.write_str(&var.name()),
            Type::Applied(applied) => {
                let data = applied.get();
                write!(f, "{} of {}", data.name, type_list(&data.args))
            }
            Type::Fun(function) => {
                let data = function.get();
                let params: Vec<String> = data
                    .params
                    .iter()
                    .enumerate()
                    .map(|(position, ty)| {
                        if position < data.required as usize { ty.to_string() } else { format!("{ty} = ...") }
                    })
                    .collect();
                write!(f, "fun({})", params.join(", "))?;
                if data.ret != Type::None {
                    // A union reads better between parentheses after `in`.
                    match data.ret {
                        union @ Type::Union(_) => write!(f, " in ({union})")?,
                        ret => write!(f, " in {ret}")?,
                    }
                }
                Ok(())
            }
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

/// A structure declared by a program: each declaration gets its own reference.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StructRef(u32);

impl StructRef {
    pub fn new(name: &str) -> StructRef {
        let mut interner = interner().lock().expect("the type interner is never poisoned");
        interner.structs.push(name.to_string());
        StructRef(interner.structs.len() as u32 - 1)
    }

    pub fn name(self) -> String {
        interner().lock().expect("the type interner is never poisoned").structs[self.0 as usize].clone()
    }

    /// Marks the structure as compared with its `equals` method (§12.5).
    pub fn set_custom_equality(self) {
        interner().lock().expect("the type interner is never poisoned").custom_equality.insert(self.0);
    }

    /// Whether the structure is compared with its `equals` method rather than field by
    /// field (§12.5).
    pub fn has_custom_equality(self) -> bool {
        interner().lock().expect("the type interner is never poisoned").custom_equality.contains(&self.0)
    }

    /// A number that identifies the structure while the program runs.
    pub fn id(self) -> u32 {
        self.0
    }

    /// Records that the structure is the instance of the generic structure `template`
    /// for the types `args` (§15.3).
    pub fn set_origin(self, template: u32, args: Vec<Type>) {
        interner()
            .lock()
            .expect("the type interner is never poisoned")
            .struct_origins
            .insert(self.0, (template, args));
    }

    /// For an instance of a generic structure: the generic structure and its types.
    pub fn origin(self) -> Option<(u32, Vec<Type>)> {
        interner().lock().expect("the type interner is never poisoned").struct_origins.get(&self.0).cloned()
    }
}

/// A generic structure or trait written with type variables (§15.1): the generic
/// structure or trait, as numbered by the checker, its name, and the types given to it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AppliedData {
    pub template: u32,
    /// A generic trait, as `Container of T`, rather than a generic structure.
    pub is_trait: bool,
    pub name: String,
    pub args: Vec<Type>,
}

/// A generic structure written with type variables, stored once for the whole process.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AppliedRef(u32);

impl AppliedRef {
    pub fn new(data: AppliedData) -> AppliedRef {
        let mut interner = interner().lock().expect("the type interner is never poisoned");
        if let Some(&id) = interner.applied_ids.get(&data) {
            return AppliedRef(id);
        }
        let id = interner.applied.len() as u32;
        interner.applied.push(data.clone());
        interner.applied_ids.insert(data, id);
        AppliedRef(id)
    }

    pub fn get(self) -> AppliedData {
        interner().lock().expect("the type interner is never poisoned").applied[self.0 as usize].clone()
    }
}

impl fmt::Debug for AppliedRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.get())
    }
}

/// The types after `of`, as they are written: `Int`, `(maybe Int)`, `(Int, Text)`.
pub fn type_list(types: &[Type]) -> String {
    match types {
        [single @ Type::Union(_)] => format!("({single})"),
        [single] => single.to_string(),
        _ => {
            let types: Vec<String> = types.iter().map(Type::to_string).collect();
            format!("({})", types.join(", "))
        }
    }
}

impl fmt::Debug for StructRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

/// An enumeration declared by a program: its name and its values, in order.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EnumRef(u32);

struct EnumData {
    name: String,
    values: Vec<String>,
    ordered: bool,
}

impl EnumRef {
    pub fn new(name: &str, values: Vec<String>, ordered: bool) -> EnumRef {
        let mut interner = interner().lock().expect("the type interner is never poisoned");
        interner.enums.push(EnumData { name: name.to_string(), values, ordered });
        EnumRef(interner.enums.len() as u32 - 1)
    }

    pub fn name(self) -> String {
        interner().lock().expect("the type interner is never poisoned").enums[self.0 as usize].name.clone()
    }

    pub fn values(self) -> Vec<String> {
        interner().lock().expect("the type interner is never poisoned").enums[self.0 as usize].values.clone()
    }

    /// Declared with `[...]`: its values compare with `<` (D33).
    pub fn is_ordered(self) -> bool {
        interner().lock().expect("the type interner is never poisoned").enums[self.0 as usize].ordered
    }

    /// The position of the value `name`, from 0.
    pub fn position(self, name: &str) -> Option<u32> {
        let interner = interner().lock().expect("the type interner is never poisoned");
        interner.enums[self.0 as usize].values.iter().position(|value| value == name).map(|at| at as u32)
    }
}

impl fmt::Debug for EnumRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

/// A type variable of a generic function (§15.2).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VarRef(u32);

impl VarRef {
    pub fn new(name: &str) -> VarRef {
        let mut interner = interner().lock().expect("the type interner is never poisoned");
        interner.vars.push(name.to_string());
        VarRef(interner.vars.len() as u32 - 1)
    }

    pub fn name(self) -> String {
        interner().lock().expect("the type interner is never poisoned").vars[self.0 as usize].clone()
    }
}

impl fmt::Debug for VarRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

impl Type {
    /// Whether the type mentions a type variable.
    pub fn has_vars(self) -> bool {
        match self {
            Type::Var(_) => true,
            Type::List(inner) | Type::Set(inner) | Type::Task(inner) | Type::Domain(inner) => {
                inner.get().has_vars()
            }
            Type::Tuple(tuple) | Type::Map(tuple) => tuple.elements().into_iter().any(Type::has_vars),
            Type::Union(union) => union.members().into_iter().any(Type::has_vars),
            Type::Fun(function) => {
                let data = function.get();
                data.ret.has_vars() || data.params.into_iter().any(Type::has_vars)
            }
            Type::Applied(applied) => applied.get().args.into_iter().any(Type::has_vars),
            _ => false,
        }
    }

    /// Whether the type mentions a generic trait written with type variables, as
    /// `Container of T` (§15.1).
    pub fn has_trait_pattern(self) -> bool {
        match self {
            Type::List(inner) | Type::Set(inner) | Type::Task(inner) | Type::Domain(inner) => {
                inner.get().has_trait_pattern()
            }
            Type::Tuple(tuple) | Type::Map(tuple) => {
                tuple.elements().into_iter().any(Type::has_trait_pattern)
            }
            Type::Union(union) => union.members().into_iter().any(Type::has_trait_pattern),
            Type::Fun(function) => {
                let data = function.get();
                data.ret.has_trait_pattern() || data.params.into_iter().any(Type::has_trait_pattern)
            }
            Type::Applied(applied) => {
                let data = applied.get();
                data.is_trait || data.args.into_iter().any(Type::has_trait_pattern)
            }
            _ => false,
        }
    }

    /// The type with its variables replaced.
    pub fn substitute(self, bindings: &HashMap<VarRef, Type>) -> Type {
        match self {
            Type::Var(var) => bindings.get(&var).copied().unwrap_or(self),
            Type::List(inner) => Type::list(inner.get().substitute(bindings)),
            Type::Set(inner) => Type::set(inner.get().substitute(bindings)),
            Type::Task(inner) => Type::Task(TypeRef::new(inner.get().substitute(bindings))),
            Type::Domain(inner) => Type::domain(inner.get().substitute(bindings)),
            Type::Tuple(tuple) => {
                Type::tuple(tuple.elements().into_iter().map(|ty| ty.substitute(bindings)).collect())
            }
            Type::Map(parts) => Type::Map(TupleRef::new(
                parts.elements().into_iter().map(|ty| ty.substitute(bindings)).collect(),
            )),
            Type::Union(union) => Type::union(union.members().into_iter().map(|ty| ty.substitute(bindings))),
            Type::Fun(function) => {
                let data = function.get();
                let params = data.params.into_iter().map(|ty| ty.substitute(bindings)).collect();
                Type::function(params, data.required as usize, data.ret.substitute(bindings))
            }
            // Still a pattern: the checker makes the structure for the types (§15.3).
            Type::Applied(applied) => {
                let data = applied.get();
                let args = data.args.into_iter().map(|ty| ty.substitute(bindings)).collect();
                Type::Applied(AppliedRef::new(AppliedData { args, ..data }))
            }
            other => other,
        }
    }

    /// The type with each generic structure written with types replaced by `make`,
    /// innermost first; `None` when `make` fails.
    pub fn make_applied(self, make: &mut dyn FnMut(AppliedData) -> Option<Type>) -> Option<Type> {
        let each = |types: Vec<Type>, make: &mut dyn FnMut(AppliedData) -> Option<Type>| {
            types.into_iter().map(|ty| ty.make_applied(make)).collect::<Option<Vec<Type>>>()
        };
        Some(match self {
            Type::List(inner) => Type::list(inner.get().make_applied(make)?),
            Type::Set(inner) => Type::set(inner.get().make_applied(make)?),
            Type::Task(inner) => Type::Task(TypeRef::new(inner.get().make_applied(make)?)),
            Type::Domain(inner) => Type::domain(inner.get().make_applied(make)?),
            Type::Tuple(tuple) => Type::tuple(each(tuple.elements(), make)?),
            Type::Map(parts) => Type::Map(TupleRef::new(each(parts.elements(), make)?)),
            Type::Union(union) => Type::union(each(union.members(), make)?),
            Type::Fun(function) => {
                let data = function.get();
                let params = each(data.params, make)?;
                Type::function(params, data.required as usize, data.ret.make_applied(make)?)
            }
            Type::Applied(applied) => {
                let data = applied.get();
                let args = each(data.args, make)?;
                make(AppliedData { args, ..data })?
            }
            other => other,
        })
    }

    /// Binds the variables of `self`, a pattern, so that it becomes `actual`; false when
    /// it cannot, or when a variable would get two types.
    pub fn unify(self, actual: Type, bindings: &mut HashMap<VarRef, Type>) -> bool {
        match (self, actual) {
            (Type::Var(var), _) => match bindings.get(&var) {
                Some(&bound) => bound == actual,
                None => {
                    bindings.insert(var, actual);
                    true
                }
            },
            (Type::List(pattern), Type::List(actual))
            | (Type::Set(pattern), Type::Set(actual))
            | (Type::Task(pattern), Type::Task(actual))
            | (Type::Domain(pattern), Type::Domain(actual)) => pattern.get().unify(actual.get(), bindings),
            (Type::Tuple(pattern), Type::Tuple(actual)) | (Type::Map(pattern), Type::Map(actual)) => {
                let (pattern, actual) = (pattern.elements(), actual.elements());
                pattern.len() == actual.len()
                    && pattern
                        .into_iter()
                        .zip(actual)
                        .all(|(pattern, actual)| pattern.unify(actual, bindings))
            }
            (Type::Fun(pattern), Type::Fun(actual)) => {
                let (pattern, actual) = (pattern.get(), actual.get());
                pattern.params.len() == actual.params.len()
                    && pattern
                        .params
                        .into_iter()
                        .zip(actual.params)
                        .all(|(pattern, actual)| pattern.unify(actual, bindings))
                    && pattern.ret.unify(actual.ret, bindings)
            }
            // `Pair of (T, U)` and the instance `Pair of (Int, Text)` (§15.1).
            (Type::Applied(pattern), Type::Struct(actual)) if !pattern.get().is_trait => {
                let pattern = pattern.get();
                match actual.origin() {
                    Some((template, args))
                        if template == pattern.template && args.len() == pattern.args.len() =>
                    {
                        pattern
                            .args
                            .into_iter()
                            .zip(args)
                            .all(|(pattern, actual)| pattern.unify(actual, bindings))
                    }
                    _ => false,
                }
            }
            (pattern, actual) if !pattern.has_vars() => {
                actual.is_subset_of(pattern) || actual == Type::Int && pattern == Type::Float
            }
            _ => false,
        }
    }
}

/// A trait declared by a program, and the types that satisfy it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TraitRef(u32);

impl TraitRef {
    pub fn new(name: &str) -> TraitRef {
        let mut interner = interner().lock().expect("the type interner is never poisoned");
        interner.traits.push((name.to_string(), Vec::new()));
        TraitRef(interner.traits.len() as u32 - 1)
    }

    pub fn name(self) -> String {
        interner().lock().expect("the type interner is never poisoned").traits[self.0 as usize].0.clone()
    }

    pub fn members(self) -> Vec<Type> {
        interner().lock().expect("the type interner is never poisoned").traits[self.0 as usize].1.clone()
    }

    /// Once every method is known: the types that satisfy the trait.
    pub fn set_members(self, members: Vec<Type>) {
        interner().lock().expect("the type interner is never poisoned").traits[self.0 as usize].1 = members;
    }

    /// Records that the trait is the instance of the generic trait `template` for the
    /// types `args` (§15.1).
    pub fn set_origin(self, template: u32, args: Vec<Type>) {
        interner()
            .lock()
            .expect("the type interner is never poisoned")
            .trait_origins
            .insert(self.0, (template, args));
    }

    /// For an instance of a generic trait: the generic trait and its types.
    pub fn origin(self) -> Option<(u32, Vec<Type>)> {
        interner().lock().expect("the type interner is never poisoned").trait_origins.get(&self.0).cloned()
    }
}

impl fmt::Debug for TraitRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

/// The type of a function value: its parameters and its result.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FunData {
    pub params: Vec<Type>,
    /// The first parameters, which a call must give; the others have default values.
    pub required: u32,
    pub ret: Type,
}

/// A function type, stored once for the whole process.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FunRef(u32);

impl FunRef {
    fn new(data: FunData) -> FunRef {
        let mut interner = interner().lock().expect("the type interner is never poisoned");
        if let Some(&id) = interner.function_ids.get(&data) {
            return FunRef(id);
        }
        let id = interner.functions.len() as u32;
        interner.functions.push(data.clone());
        interner.function_ids.insert(data, id);
        FunRef(id)
    }

    pub fn get(self) -> FunData {
        interner().lock().expect("the type interner is never poisoned").functions[self.0 as usize].clone()
    }
}

impl fmt::Debug for FunRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.get())
    }
}

/// The types of the elements of a tuple, stored once for the whole process.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TupleRef(u32);

impl TupleRef {
    fn new(elements: Vec<Type>) -> TupleRef {
        let mut interner = interner().lock().expect("the type interner is never poisoned");
        if let Some(&id) = interner.tuple_ids.get(&elements) {
            return TupleRef(id);
        }
        let id = interner.tuples.len() as u32;
        interner.tuples.push(elements.clone());
        interner.tuple_ids.insert(elements, id);
        TupleRef(id)
    }

    pub fn elements(self) -> Vec<Type> {
        interner().lock().expect("the type interner is never poisoned").tuples[self.0 as usize].clone()
    }
}

impl fmt::Debug for TupleRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.elements())
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
    structs: Vec<String>,
    /// The structures whose equality is their `equals` method (§12.5).
    custom_equality: HashSet<u32>,
    enums: Vec<EnumData>,
    tuples: Vec<Vec<Type>>,
    tuple_ids: HashMap<Vec<Type>, u32>,
    functions: Vec<FunData>,
    function_ids: HashMap<FunData, u32>,
    traits: Vec<(String, Vec<Type>)>,
    vars: Vec<String>,
    /// The instances of generic structures: their generic structure and types (§15.3).
    struct_origins: HashMap<u32, (u32, Vec<Type>)>,
    /// The instances of generic traits: their generic trait and types (§15.1).
    trait_origins: HashMap<u32, (u32, Vec<Type>)>,
    applied: Vec<AppliedData>,
    applied_ids: HashMap<AppliedData, u32>,
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
    fn a_generic_structure_with_variables_binds_them() {
        let (t, u) = (VarRef::new("T"), VarRef::new("U"));
        let pattern = Type::Applied(AppliedRef::new(AppliedData {
            template: 7,
            is_trait: false,
            name: "Pair".to_string(),
            args: vec![Type::Var(t), Type::Var(u)],
        }));
        assert_eq!(pattern.to_string(), "Pair of (T, U)");
        assert!(pattern.has_vars());
        let instance = StructRef::new("Pair of (Int, Text)");
        instance.set_origin(7, vec![Type::Int, Type::Text]);
        let mut bindings = HashMap::new();
        assert!(pattern.unify(Type::Struct(instance), &mut bindings));
        assert_eq!(bindings[&t], Type::Int);
        assert_eq!(bindings[&u], Type::Text);
        let other = StructRef::new("Other");
        assert!(!pattern.unify(Type::Struct(other), &mut HashMap::new()));
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
