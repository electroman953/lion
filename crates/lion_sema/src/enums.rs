//! Enumerations and named unions (spec §13): `Color = {red, green}`, `Days = [mon,
//! tue]`, `Shape = Circle or Rect`.
//!
//! A value of an enumeration is written `Color.red`, or `red` alone where a value of
//! the enumeration is expected (D32, C54).

use std::collections::HashMap;

use lion_diagnostics::{Diagnostic, Span};
use lion_ir::{self as ir, EnumRef, Type};
use lion_syntax::ast;

use crate::{Checker, typed};

/// A type defined with `=` (§13).
pub(crate) enum NamedType<'a> {
    Enum(EnumRef),
    Union { decl: &'a ast::TypeDef, state: AliasState },
}

#[derive(Clone, Copy)]
pub(crate) enum AliasState {
    Unresolved,
    Resolving,
    /// `None` when the definition has an error.
    Resolved(Option<Type>),
}

impl<'a> Checker<'a> {
    /// Registers `Name = ...`: an enumeration gets its type at once, a named union when
    /// a type first names it.
    pub(crate) fn register_type_definition(&mut self, decl: &'a ast::TypeDef) {
        let name = &decl.name.name;
        if !self.check_type_name(&decl.name) {
            return;
        }
        let named = match &decl.kind {
            ast::TypeDefKind::Enum { ordered, values } => {
                let mut seen: HashMap<&str, Span> = HashMap::new();
                let mut names = Vec::new();
                for value in values {
                    if let Some(&first) = seen.get(value.name.as_str()) {
                        self.diagnostics.push(
                            Diagnostic::error(format!("`{}` is twice a value of `{name}`", value.name))
                                .with_primary(value.span, "")
                                .with_secondary(first, "first here"),
                        );
                        continue;
                    }
                    seen.insert(&value.name, value.span);
                    names.push(value.name.clone());
                }
                let enumeration = EnumRef::new(&self.qualified(name), names, *ordered);
                self.enums.push(enumeration);
                NamedType::Enum(enumeration)
            }
            ast::TypeDefKind::Union(_) => NamedType::Union { decl, state: AliasState::Unresolved },
        };
        self.tables.type_spans.insert(name.clone(), decl.name.span);
        self.tables.named_types.insert(name.clone(), named);
    }

    /// A new type is named differently from the types of Lion and from the other types
    /// of the file.
    pub(crate) fn check_type_name(&mut self, name: &ast::Ident) -> bool {
        if crate::types::is_standard_type(&name.name) {
            self.diagnostics.push(
                Diagnostic::error(format!("`{}` is already a type of Lion", name.name))
                    .with_primary(name.span, "")
                    .with_help("give the new type another name"),
            );
            return false;
        }
        if let Some(&first) = self.tables.type_spans.get(&name.name) {
            self.diagnostics.push(
                Diagnostic::error(format!("the type `{}` is already declared", name.name))
                    .with_primary(name.span, "declared again here")
                    .with_secondary(first, "first declared here"),
            );
            return false;
        }
        true
    }

    /// The type named by a definition, resolving a named union the first time.
    pub(crate) fn defined_type(&mut self, name: &str, span: Span) -> Option<Option<Type>> {
        let named = self.tables.named_types.get(name)?;
        let (decl, state) = match named {
            NamedType::Enum(enumeration) => return Some(Some(Type::Enum(*enumeration))),
            NamedType::Union { decl, state } => (*decl, *state),
        };
        let resolved = match state {
            AliasState::Resolved(ty) => ty,
            AliasState::Resolving => {
                self.diagnostics.push(
                    Diagnostic::error(format!("the type `{name}` is defined by itself"))
                        .with_primary(span, "")
                        .with_secondary(decl.name.span, "declared here")
                        .with_note("a named union lists other types (§13.2)"),
                );
                None
            }
            AliasState::Unresolved => {
                self.set_alias_state(name, AliasState::Resolving);
                let ast::TypeDefKind::Union(ty) = &decl.kind else { unreachable!("a named union") };
                let resolved = self.resolve_type(ty);
                self.set_alias_state(name, AliasState::Resolved(resolved));
                resolved
            }
        };
        Some(resolved)
    }

    fn set_alias_state(&mut self, name: &str, new: AliasState) {
        if let Some(NamedType::Union { state, .. }) = self.tables.named_types.get_mut(name) {
            *state = new;
        }
    }

    /// Resolves every named union, to report the errors of those that are never used.
    pub(crate) fn resolve_type_definitions(&mut self) {
        let mut names: Vec<(String, Span)> = self
            .tables
            .named_types
            .iter()
            .filter_map(|(name, named)| match named {
                NamedType::Union { decl, .. } => Some((name.clone(), decl.name.span)),
                NamedType::Enum(_) => None,
            })
            .collect();
        // In the order of the file, for the same messages at each run.
        names.sort_by_key(|(_, span)| span.start);
        for (name, span) in names {
            self.defined_type(&name, span);
        }
    }

    /// `Color.red` (§13.1).
    pub(crate) fn enum_member(
        &mut self,
        type_name: &str,
        type_span: Span,
        name: &ast::Ident,
    ) -> Option<ir::Expr> {
        let span = type_span.to(name.span);
        let Some(Some(Type::Enum(enumeration))) = self.defined_type(type_name, type_span) else {
            self.diagnostics.push(
                Diagnostic::error(format!("`{type_name}` is not an enumeration"))
                    .with_primary(type_span, "")
                    .with_note("`Type.value` names a value of an enumeration (§13.1)"),
            );
            return None;
        };
        match enumeration.position(&name.name) {
            Some(value) => Some(enum_value(enumeration, value, span)),
            None => {
                let values = enumeration.values();
                let mut error = Diagnostic::error(format!("`{}` is not a value of `{type_name}`", name.name))
                    .with_primary(name.span, "")
                    .with_note(format!("the values of `{type_name}`: {}", values.join(", ")));
                if let Some(close) = crate::names::closest(&name.name, values.iter().map(String::as_str)) {
                    error = error.with_help(format!("a similar value exists: `{close}`"));
                }
                self.diagnostics.push(error);
                None
            }
        }
    }

    /// `red` alone, where a value of an enumeration of `expected` is expected (D32).
    pub(crate) fn expected_enum_value(&self, expr: &ast::Expr, expected: Type) -> Option<ir::Expr> {
        let ast::ExprKind::Name(name) = &expr.kind else { return None };
        // A variable of that name wins; a function is not a value here (§11.3).
        if self.is_variable(name) {
            return None;
        }
        expected.members().into_iter().find_map(|member| match member {
            Type::Enum(enumeration) => {
                enumeration.position(name).map(|value| enum_value(enumeration, value, expr.span))
            }
            _ => None,
        })
    }

    /// The enumerations that have a value named `name`, for messages.
    pub(crate) fn enums_with_value(&self, name: &str) -> Vec<EnumRef> {
        self.enums.iter().copied().filter(|enumeration| enumeration.position(name).is_some()).collect()
    }

    /// `for d in Days`: the values of an enumeration, in order (§13.1).
    pub(crate) fn enum_values(&mut self, type_name: &str, span: Span) -> Option<ir::Expr> {
        match self.defined_type(type_name, span) {
            Some(Some(Type::Enum(enumeration))) => {
                let values = (0..enumeration.values().len() as u32)
                    .map(|value| enum_value(enumeration, value, span))
                    .collect();
                Some(typed(ir::ExprKind::List(values), Type::list(Type::Enum(enumeration)), span))
            }
            _ => None,
        }
    }
}

pub(crate) fn enum_value(enumeration: EnumRef, value: u32, span: Span) -> ir::Expr {
    typed(ir::ExprKind::Enum { enumeration, value }, Type::Enum(enumeration), span)
}
