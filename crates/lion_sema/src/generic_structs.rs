//! Generic structures (spec §15.1, §15.3, D78): `struct Pair of (A, B), A in Type,
//! B in Type`. A generic structure is a model: each list of types it is used with, as
//! `Pair of (Int, Text)`, makes a structure of its own, with its fields, its conditions
//! and its layout (§15.3). `Pair(1, "one")` takes the types of the values given.

use std::collections::HashMap;

use lion_diagnostics::{Diagnostic, Span};
use lion_ir::{self as ir, StructRef, Type};
use lion_syntax::ast;

use crate::structs::Given;
use crate::{Checker, article, capitalize};

pub(crate) struct GenericStruct<'a> {
    pub(crate) decl: &'a ast::StructDecl,
    /// The file that declares it.
    pub(crate) module: usize,
    /// The instances made so far, by their types, as indices of structures.
    pub(crate) instances: HashMap<Vec<Type>, usize>,
    /// Its methods, which each instance receives (C90).
    pub(crate) methods: Vec<std::rc::Rc<ast::FunDecl>>,
}

/// A type that a type parameter must belong to, checked once the members of the traits
/// are known.
pub(crate) struct PendingConstraint {
    ty: Type,
    constraint: Type,
    param: ast::Ident,
    span: Span,
}

impl<'a> Checker<'a> {
    pub(crate) fn register_generic_structure(&mut self, decl: &'a ast::StructDecl) {
        self.tables.generic_structs.insert(decl.name.name.clone(), self.generic_structs.len());
        self.generic_structs.push(GenericStruct {
            decl,
            module: self.module,
            instances: HashMap::new(),
            methods: Vec::new(),
        });
    }

    /// `Pair of (Int, Text)`: the instance of a generic structure for these types.
    pub(crate) fn generic_struct_type(
        &mut self,
        template: usize,
        args: &[ast::TypeExpr],
        span: Span,
    ) -> Option<Type> {
        let decl = self.generic_structs[template].decl;
        let count = decl.type_params.len();
        if args.len() != count {
            let params: Vec<&str> = decl.type_params.iter().map(|(param, _)| param.name.as_str()).collect();
            let written = if count == 1 { params[0].to_string() } else { format!("({})", params.join(", ")) };
            let plural = if count == 1 { "" } else { "s" };
            self.diagnostics.push(
                Diagnostic::error(format!(
                    "`{0}` takes {count} type parameter{plural}: `{0} of {written}`",
                    decl.name.name
                ))
                .with_primary(span, "")
                .with_secondary(decl.name.span, "declared here")
                .with_note("the types of a generic structure are given with `of` (§15.1)"),
            );
            return None;
        }
        let types: Vec<Option<Type>> = args.iter().map(|arg| self.resolve_type(arg)).collect();
        let types: Vec<Type> = types.into_iter().collect::<Option<_>>()?;
        // `Pair of (T, U)` in a generic signature: each instance of the function makes the
        // structure for the types of its variables (§15.1, §15.3).
        if types.iter().any(|ty| ty.has_vars()) {
            let name = self.qualified_in(self.generic_structs[template].module, &decl.name.name);
            return Some(Type::Applied(ir::AppliedRef::new(ir::AppliedData {
                template: template as u32,
                name,
                args: types,
            })));
        }
        let index = self.instantiate_structure(template, types, span)?;
        Some(Type::Struct(self.structs[index].id))
    }

    /// The structure made from a generic one for these types, made on first use (§15.3).
    pub(crate) fn instantiate_structure(
        &mut self,
        template: usize,
        types: Vec<Type>,
        span: Span,
    ) -> Option<usize> {
        if let Some(&index) = self.generic_structs[template].instances.get(&types) {
            return Some(index);
        }
        let (decl, module) = (self.generic_structs[template].decl, self.generic_structs[template].module);
        let previous = self.enter_module(module);
        let mut valid = true;
        for ((param, set), &ty) in decl.type_params.iter().zip(&types) {
            if is_all_types(set) {
                continue;
            }
            let Some(constraint) = self.resolve_type(set) else {
                valid = false;
                continue;
            };
            let pending = PendingConstraint { ty, constraint, param: param.clone(), span };
            if self.trait_members_known {
                valid &= self.check_constraint(&pending);
            } else {
                self.pending_constraints.push(pending);
            }
        }
        if !valid {
            self.enter_module(previous);
            return None;
        }
        let name = format!("{} of {}", self.qualified(&decl.name.name), ir::type_list(&types));
        let type_args =
            decl.type_params.iter().map(|(param, _)| param.name.clone()).zip(types.iter().copied()).collect();
        let id = StructRef::new(&name);
        id.set_origin(template as u32, types.clone());
        let index = self.push_structure(decl, id, type_args);
        // Registered before its fields, which may name it: `next in maybe Node of T`.
        self.generic_structs[template].instances.insert(types, index);
        self.resolve_fields_here(index);
        self.enter_module(previous);
        for method in self.generic_structs[template].methods.clone() {
            self.register_method_instance(method, template, index);
        }
        // Made once the traits are known: it joins those it satisfies now (C67).
        if self.trait_members_known {
            self.conform_late(Type::Struct(self.structs[index].id));
        }
        Some(index)
    }

    /// `ty` with each generic structure written with types, as `Pair of (Int, Text)` once
    /// the variables of `Pair of (T, U)` are known, replaced by its structure (§15.3).
    pub(crate) fn make_applied(&mut self, ty: Type, span: Span) -> Option<Type> {
        ty.make_applied(&mut |applied| {
            let index = self.instantiate_structure(applied.template as usize, applied.args, span)?;
            Some(Type::Struct(self.structs[index].id))
        })
    }

    /// Makes the instances found by a previous check, before the traits (C90).
    pub(crate) fn make_seeds(&mut self) {
        for seed in std::mem::take(&mut self.seeds) {
            self.translate(seed);
        }
    }

    /// The type of this check that stands for `ty`, a type of a previous check: the
    /// structures, enumerations and traits are found by their names, which are unique,
    /// and the instances of generic structures are made again.
    fn translate(&mut self, ty: Type) -> Option<Type> {
        let each = |checker: &mut Self, types: Vec<Type>| {
            types.into_iter().map(|ty| checker.translate(ty)).collect::<Option<Vec<Type>>>()
        };
        Some(match ty {
            Type::List(inner) => Type::list(self.translate(inner.get())?),
            Type::Set(inner) => Type::set(self.translate(inner.get())?),
            Type::Domain(inner) => Type::domain(self.translate(inner.get())?),
            Type::Task(inner) => Type::Task(ir::TypeRef::new(self.translate(inner.get())?)),
            Type::Tuple(tuple) => Type::tuple(each(self, tuple.elements())?),
            Type::Map(parts) => {
                let parts = each(self, parts.elements())?;
                Type::map(parts[0], parts[1])
            }
            Type::Union(union) => Type::union(each(self, union.members())?),
            Type::Fun(function) => {
                let data = function.get();
                let params = each(self, data.params)?;
                Type::function(params, data.required as usize, self.translate(data.ret)?)
            }
            Type::Struct(structure) => match structure.origin() {
                Some((template, args)) => {
                    let args = each(self, args)?;
                    let span = self.generic_structs[template as usize].decl.name.span;
                    let index = self.instantiate_structure(template as usize, args, span)?;
                    Type::Struct(self.structs[index].id)
                }
                None => {
                    let name = structure.name();
                    let found = self
                        .structs
                        .iter()
                        .find(|info| info.type_args.is_empty() && info.id.name() == name)?;
                    Type::Struct(found.id)
                }
            },
            Type::Enum(enumeration) => {
                let name = enumeration.name();
                Type::Enum(self.enums.iter().copied().find(|found| found.name() == name)?)
            }
            Type::Trait(set) => {
                let name = set.name();
                if name == self.comparable.name() {
                    Type::Trait(self.comparable)
                } else if name == self.error_trait.name() {
                    Type::Trait(self.error_trait)
                } else {
                    Type::Trait(self.traits.iter().find(|info| info.id.name() == name)?.id)
                }
            }
            other => other,
        })
    }

    /// The constraints met before the members of the traits were known.
    pub(crate) fn check_pending_constraints(&mut self) {
        self.trait_members_known = true;
        for pending in std::mem::take(&mut self.pending_constraints) {
            self.check_constraint(&pending);
        }
    }

    fn check_constraint(&mut self, pending: &PendingConstraint) -> bool {
        if pending.ty.is_subset_of(pending.constraint) {
            return true;
        }
        self.diagnostics.push(
            Diagnostic::error(format!("{} is not {}", capitalize(&article(pending.ty)), pending.constraint))
                .with_primary(pending.span, "")
                .with_secondary(
                    pending.param.span,
                    format!("`{}` must be {}", pending.param.name, pending.constraint),
                )
                .with_note(
                    "each type parameter of a generic structure belongs to its set of types (§15.1, §15.2)",
                ),
        );
        false
    }

    /// `Pair(1, "one")`: the types of the parameters are those of the values (§15.1).
    pub(crate) fn construct_generic(
        &mut self,
        template: usize,
        args: &[ast::Arg],
        span: Span,
    ) -> Option<ir::Expr> {
        let decl = self.generic_structs[template].decl;
        let checked: Vec<Option<ir::Expr>> = args.iter().map(|arg| self.expr(&arg.value)).collect();
        let fields = decl.lines.iter().filter_map(|line| match line {
            ast::StructLine::Field(field) => Some(field),
            ast::StructLine::Invariant(_) => None,
        });
        let params: Vec<&str> = decl.type_params.iter().map(|(param, _)| param.name.as_str()).collect();
        let mut bindings = HashMap::new();
        for (value, field) in checked.iter().zip(fields) {
            if let Some(value) = value {
                self.bind_type_params(&field.ty, value.ty, &params, &mut bindings);
            }
        }
        if checked.iter().any(Option::is_none) {
            return None;
        }
        let mut types = Vec::new();
        for (param, _) in &decl.type_params {
            let Some(&ty) = bindings.get(&param.name) else {
                let written = if params.len() == 1 {
                    params[0].to_string()
                } else {
                    format!("({})", params.join(", "))
                };
                self.diagnostics.push(
                    Diagnostic::error(format!(
                        "the type `{}` of this `{}` is not known",
                        param.name, decl.name.name
                    ))
                    .with_primary(span, "")
                    .with_secondary(param.span, "type parameter declared here")
                    .with_note(
                        "the types of a generic structure come from the values given for its fields (§15.1)",
                    )
                    .with_help(format!(
                        "write its types: `let x = (...) in {} of {written}`",
                        decl.name.name
                    )),
                );
                return None;
            };
            types.push(ty);
        }
        let index = self.instantiate_structure(template, types, span)?;
        self.construct_from(index, &Given::args(args), span, Some(checked))
    }

    /// The types of the parameters that the written type `pattern` gets from `actual`.
    fn bind_type_params(
        &self,
        pattern: &ast::TypeExpr,
        actual: Type,
        params: &[&str],
        bindings: &mut HashMap<String, Type>,
    ) {
        match &pattern.kind {
            ast::TypeExprKind::Named { module, name, args } if module.is_empty() && args.is_empty() => {
                if params.contains(&name.name.as_str()) {
                    bindings.entry(name.name.clone()).or_insert(actual);
                }
            }
            ast::TypeExprKind::Named { module, name, args } => {
                match (if module.is_empty() { name.name.as_str() } else { "" }, args.as_slice(), actual) {
                    ("List", [element], Type::List(inner))
                    | ("Set", [element], Type::Set(inner))
                    | ("Domain", [element], Type::Domain(inner))
                    | ("Task", [element], Type::Task(inner)) => {
                        self.bind_type_params(element, inner.get(), params, bindings);
                    }
                    // An instance of a generic structure holds its types.
                    (_, _, Type::Struct(structure)) => {
                        let info = &self.structs[self.struct_index(structure)];
                        if info.decl.name.name == name.name && info.type_args.len() == args.len() {
                            for (arg, &(_, ty)) in args.iter().zip(&info.type_args) {
                                self.bind_type_params(arg, ty, params, bindings);
                            }
                        }
                    }
                    _ => {}
                }
            }
            ast::TypeExprKind::Maybe(inner) => {
                if let Some(rest) = actual.without(Type::None) {
                    self.bind_type_params(inner, rest, params, bindings);
                }
            }
            ast::TypeExprKind::Tuple(elements) => {
                if let Type::Tuple(tuple) = actual {
                    for (element, ty) in elements.iter().zip(tuple.elements()) {
                        self.bind_type_params(element, ty, params, bindings);
                    }
                }
            }
            ast::TypeExprKind::Fun { params: patterns, ret } => {
                if let Type::Fun(function) = actual {
                    let function = function.get();
                    for (pattern, ty) in patterns.iter().zip(function.params) {
                        self.bind_type_params(pattern, ty, params, bindings);
                    }
                    if let Some(ret) = ret {
                        self.bind_type_params(ret, function.ret, params, bindings);
                    }
                }
            }
            ast::TypeExprKind::Union(_) => {}
        }
    }
}

/// `T in Type`: every type (§15.2).
fn is_all_types(set: &ast::TypeExpr) -> bool {
    matches!(&set.kind, ast::TypeExprKind::Named { module, name, args }
        if module.is_empty() && args.is_empty() && name.name == "Type")
}
