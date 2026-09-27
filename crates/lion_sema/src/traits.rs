//! Traits (spec §14): a trait names the methods and the fields that a type needs, and
//! every type that has them satisfies it, without saying so (§14.1).
//!
//! A trait is the set of the types that satisfy it (§7.1), computed once every method
//! of the program is known. A value of a trait type is a value of one of them: a call
//! of a method on it chooses the method of its type at run time, with type tests (§14.4).

use std::collections::HashMap;
use std::rc::Rc;

use lion_diagnostics::{Diagnostic, Span};
use lion_ir::{self as ir, TraitRef, Type};
use lion_syntax::ast;

use crate::{Checker, article, typed};

pub(crate) struct TraitInfo<'a> {
    pub(crate) decl: &'a ast::TraitDecl,
    pub(crate) module: usize,
    pub(crate) id: TraitRef,
    requirements: Vec<Requirement>,
    fields: Vec<(String, Type)>,
    valid: bool,
}

struct Requirement {
    name: String,
    /// The types of the parameters after `self`.
    params: Vec<Type>,
    ret: Type,
    var_self: bool,
    /// The body of a default method (§14.2).
    default: Option<Rc<ast::FunDecl>>,
    span: Span,
}

impl<'a> Checker<'a> {
    pub(crate) fn register_trait(&mut self, decl: &'a ast::TraitDecl) {
        if !self.check_type_name(&decl.name) {
            return;
        }
        let name = &decl.name.name;
        let id = TraitRef::new(&self.qualified(name));
        self.tables.type_spans.insert(name.clone(), decl.name.span);
        self.tables.named_types.insert(name.clone(), crate::enums::NamedType::Trait(self.traits.len()));
        self.traits.push(TraitInfo {
            decl,
            module: self.module,
            id,
            requirements: Vec::new(),
            fields: Vec::new(),
            valid: true,
        });
    }

    /// The types of the methods and of the fields that each trait requires.
    pub(crate) fn resolve_traits(&mut self) {
        for index in 0..self.traits.len() {
            let previous = self.enter_module(self.traits[index].module);
            let decl = self.traits[index].decl;
            let mut requirements = Vec::new();
            let mut valid = true;
            for method in &decl.methods {
                let var_self = method.params.first().is_some_and(|param| param.name.name == "self");
                let mut params = Vec::new();
                for param in method.params.iter().skip(usize::from(var_self)) {
                    let Some(ty) = &param.ty else {
                        self.diagnostics.push(
                            Diagnostic::error("the parameters of a method of a trait have a type")
                                .with_primary(param.name.span, "")
                                .with_help(format!("write it: `{} in Int`", param.name.name)),
                        );
                        valid = false;
                        continue;
                    };
                    match self.resolve_type(ty) {
                        Some(ty) => params.push(ty),
                        None => valid = false,
                    }
                }
                let ret = match &method.ret {
                    Some(ret) => self.resolve_type(ret),
                    None => Some(Type::None),
                };
                let Some(ret) = ret else {
                    valid = false;
                    continue;
                };
                if requirements.iter().any(|known: &Requirement| known.name == method.name.name) {
                    self.diagnostics.push(
                        Diagnostic::error(format!("the trait asks twice for `{}`", method.name.name))
                            .with_primary(method.name.span, ""),
                    );
                    valid = false;
                    continue;
                }
                let default = match method.body {
                    ast::FunBody::Required => None,
                    _ => Some(Rc::new(method.clone())),
                };
                requirements.push(Requirement {
                    name: method.name.name.clone(),
                    params,
                    ret,
                    var_self,
                    default,
                    span: method.name.span,
                });
            }
            let mut fields = Vec::new();
            for (name, ty) in &decl.fields {
                match self.resolve_type(ty) {
                    Some(ty) => fields.push((name.name.clone(), ty)),
                    None => valid = false,
                }
            }
            let info = &mut self.traits[index];
            info.requirements = requirements;
            info.fields = fields;
            info.valid = valid;
            self.enter_module(previous);
        }
    }

    /// Which types satisfy each trait, and the default methods they receive (§14.1, §14.2).
    /// A method must write its return type to count (C67).
    pub(crate) fn conform_traits(&mut self) {
        let mut candidates = vec![Type::Int, Type::Float, Type::Bool, Type::Text, Type::None, Type::Error];
        candidates.extend(self.structs.iter().map(|info| Type::Struct(info.id)));
        candidates.extend(self.enums.iter().map(|&enumeration| Type::Enum(enumeration)));
        // `Comparable`: Int, Float, the ordered enumerations and the types with `less` (C67).
        let comparable: Vec<Type> = candidates
            .iter()
            .copied()
            .filter(|&ty| match ty {
                Type::Int | Type::Float => true,
                Type::Enum(enumeration) => enumeration.is_ordered(),
                _ => self.methods.get(&(ty, "less".to_string())).is_some_and(|methods| {
                    methods.iter().any(|&method| self.functions[method].declared_ret == Some(Type::Bool))
                }),
            })
            .collect();
        self.comparable.set_members(comparable);
        // `Error`: the simple error, and the types with `message() in Text` (§18.2, D17).
        let mut errors = vec![Type::Error];
        errors.extend(candidates.iter().copied().filter(|&ty| {
            ty != Type::Error
                && self.methods.get(&(ty, "message".to_string())).is_some_and(|methods| {
                    methods.iter().any(|&method| {
                        let function = &self.functions[method];
                        function.declared_ret == Some(Type::Text)
                            && !function.var_self
                            && function.signature.as_ref().is_some_and(|params| params.len() == 1)
                    })
                })
        }));
        self.error_trait.set_members(errors);
        // The defaults given to each type, to find two traits that give the same one.
        let mut given: HashMap<(Type, String), usize> = HashMap::new();
        // Two traits in conflict are reported once, whatever the number of types.
        let mut reported = std::collections::HashSet::new();
        for index in 0..self.traits.len() {
            if !self.traits[index].valid {
                continue;
            }
            let members: Vec<Type> =
                candidates.iter().copied().filter(|&ty| self.satisfies(index, ty)).collect();
            self.traits[index].id.set_members(members.clone());
            for &member in &members {
                let defaults: Vec<(String, Rc<ast::FunDecl>, Span)> = self.traits[index]
                    .requirements
                    .iter()
                    .filter_map(|requirement| {
                        let default = requirement.default.clone()?;
                        Some((requirement.name.clone(), default, requirement.span))
                    })
                    .collect();
                for (name, decl, span) in defaults {
                    let own = self.methods.get(&(member, name.clone())).is_some_and(|methods| {
                        methods.iter().any(|&method| !given.values().any(|&function| function == method))
                    });
                    if own {
                        continue;
                    }
                    if let Some(&first) = given.get(&(member, name.clone())) {
                        let other = self.functions[first].decl.name.span;
                        if !reported.insert((other, span)) {
                            continue;
                        }
                        self.diagnostics.push(
                            Diagnostic::error(format!("two traits give `{member}` a method `{name}`"))
                                .with_primary(span, "")
                                .with_secondary(other, "the other one")
                                .with_note(
                                    "a type receives the default methods of the traits it satisfies (§14.2)",
                                )
                                .with_help(format!("declare `fun {member}.{name}(...)` to choose")),
                        );
                        continue;
                    }
                    let function = self.register_default_method(decl, member, self.traits[index].module);
                    given.insert((member, name), function);
                }
            }
        }
    }

    /// Whether `ty` has every method and field that the trait requires (§14.1).
    fn satisfies(&self, index: usize, ty: Type) -> bool {
        let info = &self.traits[index];
        let fields_ok = info.fields.iter().all(|(name, field_ty)| match ty {
            Type::Struct(structure) => self.structs[self.struct_index(structure)]
                .fields
                .iter()
                .any(|field| &field.name == name && field.ty == Some(*field_ty)),
            _ => false,
        });
        fields_ok
            && info
                .requirements
                .iter()
                .all(|requirement| requirement.default.is_some() || self.has_method(ty, requirement))
    }

    fn has_method(&self, ty: Type, requirement: &Requirement) -> bool {
        let Some(methods) = self.methods.get(&(ty, requirement.name.clone())) else { return false };
        methods.iter().any(|&method| {
            let function = &self.functions[method];
            let Some(params) = &function.signature else { return false };
            function.var_self == requirement.var_self
                && function.declared_ret == Some(requirement.ret)
                && params.len() == requirement.params.len() + 1
                && params[1..]
                    .iter()
                    .zip(&requirement.params)
                    .all(|(param, &ty)| param.ty == Some(ty) && !param.by_reference)
        })
    }

    /// `x.name(args)` where `x` may be of several types: the method of each type, chosen
    /// with a type test (§14.4). The methods take the same values and give the same type.
    pub(crate) fn dispatch_call(
        &mut self,
        value: ir::Expr,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> Option<ir::Expr> {
        let members = value.ty.members();
        let mut methods = Vec::new();
        for &member in &members {
            let method = self.visible_method(member, &name.name)?;
            methods.push((member, method));
        }
        let (_, first) = methods[0];
        let signature = |checker: &Self, method: usize| {
            let function = &checker.functions[method];
            let params: Option<Vec<Type>> = function
                .signature
                .as_ref()
                .map(|params| params.iter().skip(1).filter_map(|param| param.ty).collect());
            (params, function.declared_ret, function.var_self)
        };
        let expected = signature(self, first);
        let same = methods.iter().all(|&(_, method)| signature(self, method) == expected);
        let (Some(params), Some(_), false) = expected.clone() else {
            self.diagnostics.push(
                Diagnostic::error(format!("`{}` cannot be chosen at run time here", name.name))
                    .with_primary(name.span, "")
                    .with_note(format!(
                        "the value is {}: each type needs a method `{}` whose parameters and result types are written, without `var self` (C67)",
                        article(value.ty),
                        name.name
                    )),
            );
            return None;
        };
        if !same || params.len() != args.len() {
            let mut error = Diagnostic::error(format!(
                "the methods `{}` of the types of this value do not take the same values",
                name.name
            ))
            .with_primary(name.span, "")
            .with_note(format!("the value is {}", article(value.ty)));
            if params.len() != args.len() {
                error = Diagnostic::error(format!(
                    "`{}` takes {} arguments, not {}",
                    name.name,
                    params.len(),
                    args.len()
                ))
                .with_primary(span, "");
            }
            self.diagnostics.push(error);
            return None;
        }
        // The object and the arguments are evaluated once, in order (§9.2).
        let object = self.temporary(value.ty, value.span);
        let mut lets = vec![(object, value)];
        for (arg, &ty) in args.iter().zip(&params) {
            let checked =
                self.expr_expecting(&arg.value, ty).and_then(|checked| self.coerce(checked, ty, None))?;
            let temp = self.temporary(ty, checked.span);
            lets.push((temp, checked));
        }
        let mut rets = Vec::new();
        let mut calls = Vec::new();
        for &(member, method) in &methods {
            let instance = match self.functions[method].instance {
                Some(instance) => instance,
                None => self.instantiate(
                    method,
                    std::iter::once(member).chain(params.iter().copied()).collect(),
                    span,
                )?,
            };
            let ret = self.return_type_of(instance, span)?;
            self.ctx.calls.push(instance);
            self.record_early_call(instance, span);
            rets.push(ret);
            let receiver = typed(ir::ExprKind::Local(object), member, span);
            let args = std::iter::once(receiver)
                .chain(
                    lets[1..].iter().map(|(temp, value)| typed(ir::ExprKind::Local(*temp), value.ty, span)),
                )
                .map(ir::Arg::Value)
                .collect();
            let call = ir::ExprKind::Call { function: ir::FunctionId(instance as u32), args };
            calls.push((member, typed(call, ret, span)));
        }
        let ty = Type::union(rets);
        let mut result = calls.pop().expect("a value has a type").1;
        for (member, call) in calls.into_iter().rev() {
            let test = typed(
                ir::ExprKind::TypeTest {
                    value: Box::new(typed(ir::ExprKind::Local(object), lets[0].1.ty, span)),
                    ty: member,
                },
                Type::Bool,
                span,
            );
            let kind =
                ir::ExprKind::If { cond: Box::new(test), then: Box::new(call), otherwise: Box::new(result) };
            result = typed(kind, ty, span);
        }
        for (local, value) in lets.into_iter().rev() {
            result =
                typed(ir::ExprKind::Let { local, value: Box::new(value), body: Box::new(result) }, ty, span);
        }
        Some(result)
    }
}
