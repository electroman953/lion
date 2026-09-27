//! Generic traits (spec §15.1, D78, C93): `trait Container of T, T in Type`. Like a
//! generic structure, a generic trait is a model: each list of types makes a trait of its
//! own, `Container of Int`, whose methods use these types. In the signature of a generic
//! function, `c in Container of T` finds `T` in the methods of the value given.

use std::collections::HashMap;

use lion_diagnostics::{Diagnostic, Span};
use lion_ir::{self as ir, TraitRef, Type, VarRef};
use lion_syntax::ast;

use crate::Checker;

pub(crate) struct GenericTrait<'a> {
    pub(crate) decl: &'a ast::TraitDecl,
    /// The file that declares it.
    pub(crate) module: usize,
    /// The instances made so far, by their types, as indices of traits.
    instances: HashMap<Vec<Type>, usize>,
    /// Its instance for its own type parameters as variables, which never has members:
    /// its methods tell which types a value gives to the parameters.
    pattern: Option<(Vec<VarRef>, usize)>,
}

impl<'a> Checker<'a> {
    pub(crate) fn register_generic_trait(&mut self, decl: &'a ast::TraitDecl) {
        self.tables.generic_traits.insert(decl.name.name.clone(), self.generic_traits.len());
        self.generic_traits.push(GenericTrait {
            decl,
            module: self.module,
            instances: HashMap::new(),
            pattern: None,
        });
    }

    /// `Container of Int`: the instance of a generic trait for these types; with type
    /// variables, the pattern that each call of a generic function fills.
    pub(crate) fn generic_trait_type(
        &mut self,
        template: usize,
        args: &[ast::TypeExpr],
        span: Span,
    ) -> Option<Type> {
        let decl = self.generic_traits[template].decl;
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
                .with_note("the types of a generic trait are given with `of` (§15.1)"),
            );
            return None;
        }
        let types: Vec<Option<Type>> = args.iter().map(|arg| self.resolve_type(arg)).collect();
        let types: Vec<Type> = types.into_iter().collect::<Option<_>>()?;
        if types.iter().any(|ty| ty.has_vars()) {
            let name = self.qualified_in(self.generic_traits[template].module, &decl.name.name);
            return Some(Type::Applied(ir::AppliedRef::new(ir::AppliedData {
                template: template as u32,
                is_trait: true,
                name,
                args: types,
            })));
        }
        let index = self.instantiate_trait(template, types, span)?;
        Some(Type::Trait(self.traits[index].id))
    }

    /// The trait made from a generic one for these types, made on first use.
    pub(crate) fn instantiate_trait(
        &mut self,
        template: usize,
        types: Vec<Type>,
        span: Span,
    ) -> Option<usize> {
        if let Some(&index) = self.generic_traits[template].instances.get(&types) {
            return Some(index);
        }
        let (decl, module) = (self.generic_traits[template].decl, self.generic_traits[template].module);
        let previous = self.enter_module(module);
        if !self.check_type_params(&decl.type_params, &types, span) {
            self.enter_module(previous);
            return None;
        }
        let name = format!("{} of {}", self.qualified(&decl.name.name), ir::type_list(&types));
        let id = TraitRef::new(&name);
        id.set_origin(template as u32, types.clone());
        let type_args =
            decl.type_params.iter().map(|(param, _)| param.name.clone()).zip(types.iter().copied()).collect();
        let index = self.push_trait(decl, id, type_args);
        self.generic_traits[template].instances.insert(types, index);
        self.resolve_trait(index);
        self.enter_module(previous);
        // Made once the traits are known: its members are found now.
        if self.trait_members_known {
            self.conform_trait_late(index);
        }
        Some(index)
    }

    /// `pattern` filled so that it becomes `actual`, as `Type::unify`, where a generic
    /// trait with variables takes the types that the methods of `actual` give (C93).
    pub(crate) fn unify_param(
        &mut self,
        pattern: Type,
        actual: Type,
        bindings: &mut HashMap<VarRef, Type>,
        span: Span,
    ) -> bool {
        if !pattern.has_trait_pattern() {
            return pattern.unify(actual, bindings);
        }
        match (pattern, actual) {
            (Type::Applied(applied), _) if applied.get().is_trait => {
                self.unify_trait(applied.get(), actual, bindings, span)
            }
            (Type::List(pattern), Type::List(actual))
            | (Type::Set(pattern), Type::Set(actual))
            | (Type::Task(pattern), Type::Task(actual))
            | (Type::Domain(pattern), Type::Domain(actual)) => {
                self.unify_param(pattern.get(), actual.get(), bindings, span)
            }
            (Type::Tuple(pattern), Type::Tuple(actual)) | (Type::Map(pattern), Type::Map(actual)) => {
                let (pattern, actual) = (pattern.elements(), actual.elements());
                pattern.len() == actual.len()
                    && pattern
                        .into_iter()
                        .zip(actual)
                        .all(|(pattern, actual)| self.unify_param(pattern, actual, bindings, span))
            }
            (Type::Fun(pattern), Type::Fun(actual)) => {
                let (pattern, actual) = (pattern.get(), actual.get());
                pattern.params.len() == actual.params.len()
                    && pattern
                        .params
                        .into_iter()
                        .zip(actual.params)
                        .all(|(pattern, actual)| self.unify_param(pattern, actual, bindings, span))
                    && self.unify_param(pattern.ret, actual.ret, bindings, span)
            }
            (Type::Applied(pattern), Type::Struct(actual)) => {
                let pattern = pattern.get();
                match actual.origin() {
                    Some((template, args))
                        if template == pattern.template && args.len() == pattern.args.len() =>
                    {
                        pattern
                            .args
                            .into_iter()
                            .zip(args)
                            .all(|(pattern, actual)| self.unify_param(pattern, actual, bindings, span))
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    /// `Container of T` against the type of a value: an instance of the same generic trait
    /// gives its types; any other type gives those of its methods, and must then satisfy
    /// the instance made for them.
    fn unify_trait(
        &mut self,
        applied: ir::AppliedData,
        actual: Type,
        bindings: &mut HashMap<VarRef, Type>,
        span: Span,
    ) -> bool {
        let template = applied.template as usize;
        if let Type::Trait(set) = actual
            && let Some((origin, args)) = set.origin()
            && origin == applied.template
        {
            return applied
                .args
                .into_iter()
                .zip(args)
                .all(|(pattern, actual)| self.unify_param(pattern, actual, bindings, span));
        }
        // Every type of the value gives the same types to the trait.
        let mut types: Option<Vec<Type>> = None;
        for member in actual.members() {
            let Some(found) = self.infer_trait_args(template, member) else { return false };
            if types.as_ref().is_some_and(|types| *types != found) {
                return false;
            }
            types = Some(found);
        }
        let Some(types) = types else { return false };
        let Some(index) = self.instantiate_trait(template, types.clone(), span) else { return false };
        actual.is_subset_of(Type::Trait(self.traits[index].id))
            && applied
                .args
                .into_iter()
                .zip(types)
                .all(|(pattern, actual)| self.unify_param(pattern, actual, bindings, span))
    }

    /// The types that the methods of `ty` give to the parameters of the generic trait: its
    /// required methods, found by their names, take the types of the methods of `ty`.
    fn infer_trait_args(&mut self, template: usize, ty: Type) -> Option<Vec<Type>> {
        let (vars, pattern) = self.trait_pattern(template)?;
        let mut found = HashMap::new();
        for requirement in &self.traits[pattern].requirements {
            // A default method is given to the type if it does not have its own.
            if requirement.default.is_some() {
                continue;
            }
            let methods = self.methods.get(&(ty, requirement.name.clone()))?;
            let method = methods.iter().map(|&method| &self.functions[method]).find(|function| {
                function.var_self == requirement.var_self
                    && function
                        .signature
                        .as_ref()
                        .is_some_and(|params| params.len() == requirement.params.len() + 1)
            })?;
            let params = method.signature.as_ref().expect("checked");
            let fits = requirement.params.iter().zip(&params[1..]).all(|(pattern, param)| {
                param.ty.is_some_and(|ty| !param.by_reference && pattern.unify(ty, &mut found))
            }) && method.declared_ret.is_some_and(|ret| requirement.ret.unify(ret, &mut found));
            if !fits {
                return None;
            }
        }
        vars.iter().map(|var| found.get(var).copied()).collect()
    }

    /// The instances of the generic traits that `ty` satisfies, made for the types that
    /// its methods give: it receives their default methods (§14.2, C93).
    pub(crate) fn instantiate_traits_for(&mut self, ty: Type) {
        for template in 0..self.generic_traits.len() {
            if let Some(types) = self.infer_trait_args(template, ty) {
                let span = self.generic_traits[template].decl.name.span;
                let errors = self.diagnostics.len();
                self.instantiate_trait(template, types, span);
                // Types outside the set of a parameter: the type does not satisfy it.
                self.diagnostics.truncate(errors);
            }
        }
    }

    /// The generic trait with its type parameters as variables.
    fn trait_pattern(&mut self, template: usize) -> Option<(Vec<VarRef>, usize)> {
        if let Some(pattern) = &self.generic_traits[template].pattern {
            return Some(pattern.clone());
        }
        let (decl, module) = (self.generic_traits[template].decl, self.generic_traits[template].module);
        let vars: Vec<VarRef> = decl.type_params.iter().map(|(param, _)| VarRef::new(&param.name)).collect();
        let type_args = decl
            .type_params
            .iter()
            .map(|(param, _)| param.name.clone())
            .zip(vars.iter().map(|&var| Type::Var(var)))
            .collect();
        let previous = self.enter_module(module);
        let id = TraitRef::new(&self.qualified(&decl.name.name));
        let index = self.push_trait(decl, id, type_args);
        let errors = self.diagnostics.len();
        self.resolve_trait(index);
        // Its errors are those of every instance, already reported there.
        self.diagnostics.truncate(errors);
        self.enter_module(previous);
        let valid = self.traits[index].valid;
        self.traits[index].valid = false;
        let pattern = (vars, index);
        self.generic_traits[template].pattern = Some(pattern.clone());
        valid.then_some(pattern)
    }
}
