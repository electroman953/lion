//! Resolution of written types (`Int`, `List of Int`, ...) to IR types.

use lion_diagnostics::Diagnostic;
use lion_ir::Type;
use lion_syntax::ast;

use crate::Checker;
use crate::names::closest;

const SUPPORTED: &[(&str, Type)] = &[
    ("Error", Type::Error),
    ("Int", Type::Int),
    ("Float", Type::Float),
    ("Rational", Type::Rational),
    ("Bool", Type::Bool),
    ("Text", Type::Text),
    ("None", Type::None),
    ("Range", Type::Range),
];

/// Types of the spec that this version does not support yet, with their section.
const PLANNED: &[(&str, &str, &str)] = &[("Type", "the type `Type`", "§15")];

/// Whether `name` is a type of Lion, which a structure cannot be named after.
pub(crate) fn is_standard_type(name: &str) -> bool {
    // `Bool = {true, false}` is an enumeration of the standard library (D45).
    name == "List"
        || name == "Set"
        || name == "Domain"
        || name == "Map"
        || name == "Task"
        || name == "Comparable"
        || name == "Type"
        || SUPPORTED.iter().any(|(known, _)| *known == name)
        || PLANNED.iter().any(|(p, ..)| *p == name)
}

impl Checker<'_> {
    /// The type written, or `None` after reporting why it cannot be used.
    pub(crate) fn resolve_type(&mut self, ty: &ast::TypeExpr) -> Option<Type> {
        match &ty.kind {
            ast::TypeExprKind::Named { module, name, args } if module.is_empty() => {
                self.named_type(name, args, ty)
            }
            ast::TypeExprKind::Named { module, name, args } => {
                let found = match module.as_slice() {
                    [single] => self.modules[self.module].imports.get(&single.name).copied(),
                    _ => None,
                };
                let Some(found) = found else {
                    let path: Vec<&str> = module.iter().map(|part| part.name.as_str()).collect();
                    self.diagnostics.push(
                        Diagnostic::error(format!("no module is reached as `{}`", path.join(".")))
                            .with_primary(ty.span, "")
                            .with_help(format!(
                                "add `use {}` at the start of the file (§20.2)",
                                path.join(".")
                            )),
                    );
                    return None;
                };
                if !args.is_empty() {
                    self.not_implemented(ty.span, "generic types of modules", "§15.1");
                    return None;
                }
                self.module_type(found, name)
            }
            // `maybe maybe T` is `maybe T` (§7.3).
            ast::TypeExprKind::Maybe(inner) => self.resolve_type(inner).map(Type::maybe),
            ast::TypeExprKind::Union(members) => {
                let members: Vec<Option<Type>> =
                    members.iter().map(|member| self.resolve_type(member)).collect();
                members.into_iter().collect::<Option<Vec<Type>>>().map(Type::union)
            }
            ast::TypeExprKind::Tuple(elements) => {
                let elements: Vec<Option<Type>> =
                    elements.iter().map(|element| self.resolve_type(element)).collect();
                elements.into_iter().collect::<Option<Vec<Type>>>().map(Type::tuple)
            }
            ast::TypeExprKind::Fun { params, ret } => {
                let params: Vec<Option<Type>> = params.iter().map(|param| self.resolve_type(param)).collect();
                let ret = match ret {
                    Some(ret) => self.resolve_type(ret),
                    None => Some(Type::None),
                };
                let params = params.into_iter().collect::<Option<Vec<Type>>>()?;
                let required = params.len();
                Some(Type::function(params, required, ret?))
            }
        }
    }

    fn named_type(&mut self, name: &ast::Ident, args: &[ast::TypeExpr], ty: &ast::TypeExpr) -> Option<Type> {
        if let Some(&(_, var)) = self.type_vars.iter().rev().find(|(known, _)| *known == name.name) {
            return Some(var);
        }
        if name.name == "Comparable" {
            return Some(Type::Trait(self.comparable));
        }
        // `Error` is a trait: its own errors, and the types with `message()` (§18.2).
        if name.name == "Error" && args.is_empty() {
            return Some(Type::Trait(self.error_trait));
        }
        // `Map of (Text, Int)`: the type of the keys, then the type of the values (C79).
        if name.name == "Map" {
            let [key, value] = args else {
                self.diagnostics.push(
                    Diagnostic::error(
                        "`Map` takes the types of its keys and of its values: `Map of (Text, Int)`",
                    )
                    .with_primary(ty.span, "")
                    .with_note("`of` gives the type parameters of a generic type (§15.1)"),
                );
                return None;
            };
            let (key, value) = (self.resolve_type(key), self.resolve_type(value));
            return Some(Type::map(key?, value?));
        }
        if matches!(name.name.as_str(), "List" | "Set" | "Domain" | "Task") {
            let [element] = args else {
                self.diagnostics.push(
                    Diagnostic::error(format!(
                        "`{0}` takes the type of its elements: `{0} of Int`",
                        name.name
                    ))
                    .with_primary(ty.span, "")
                    .with_note("`of` gives the type parameters of a generic type (§15.1)"),
                );
                return None;
            };
            let element = self.resolve_type(element)?;
            return Some(match name.name.as_str() {
                "List" => Type::list(element),
                "Set" => Type::set(element),
                "Domain" => Type::domain(element),
                _ => Type::Task(lion_ir::TypeRef::new(element)),
            });
        }
        if let Some(&template) = self.tables.generic_structs.get(&name.name) {
            return self.generic_struct_type(template, args, ty.span);
        }
        if let Some(&index) = self.tables.struct_names.get(&name.name) {
            if !args.is_empty() {
                self.diagnostics.push(
                    Diagnostic::error(format!("`{}` does not take type parameters", name.name))
                        .with_primary(ty.span, ""),
                );
                return None;
            }
            return Some(Type::Struct(self.structs[index].id));
        }
        if let Some(resolved) = self.defined_type(&name.name, name.span) {
            if !args.is_empty() {
                self.diagnostics.push(
                    Diagnostic::error(format!("`{}` does not take type parameters", name.name))
                        .with_primary(ty.span, ""),
                );
                return None;
            }
            return resolved;
        }
        if let Some(&(_, what, section)) = PLANNED.iter().find(|(planned, ..)| *planned == name.name) {
            self.not_implemented(ty.span, what, section);
            return None;
        }
        let Some(&(_, resolved)) = SUPPORTED.iter().find(|(known, _)| *known == name.name) else {
            let known = SUPPORTED
                .iter()
                .map(|(known, _)| *known)
                .chain(PLANNED.iter().map(|(p, ..)| *p))
                .chain(["List", "Set", "Domain", "Map"])
                .chain(self.tables.type_spans.keys().map(String::as_str));
            let mut error = Diagnostic::error(format!("cannot find the type `{}`", name.name))
                .with_primary(name.span, "unknown type");
            if let Some(close) = closest(&name.name, known) {
                error = error.with_help(format!("a similar type exists: `{close}`"));
            }
            self.diagnostics.push(error);
            return None;
        };
        if !args.is_empty() {
            self.diagnostics.push(
                Diagnostic::error(format!("`{}` does not take type parameters", name.name))
                    .with_primary(ty.span, ""),
            );
            return None;
        }
        Some(resolved)
    }
}
