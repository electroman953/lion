//! Resolution of written types (`Int`, `List of Int`, ...) to IR types.

use lion_diagnostics::Diagnostic;
use lion_ir::Type;
use lion_syntax::ast;

use crate::Checker;
use crate::names::closest;

const SUPPORTED: &[(&str, Type)] = &[
    ("Int", Type::Int),
    ("Float", Type::Float),
    ("Bool", Type::Bool),
    ("Text", Type::Text),
    ("None", Type::None),
];

/// Types of the spec that this version does not support yet, with their section.
const PLANNED: &[(&str, &str, &str)] = &[
    ("Rational", "Rational numbers", "§8.3"),
    ("List", "collections", "§16"),
    ("Set", "collections", "§16"),
    ("Domain", "collections", "§16"),
    ("Range", "collections", "§16"),
    ("Map", "collections", "§16"),
    ("Error", "errors", "§18"),
    ("Task", "tasks", "§19.1"),
    ("Type", "the type `Type`", "§15"),
];

impl Checker<'_> {
    /// The type written, or `None` after reporting why it cannot be used.
    pub(crate) fn resolve_type(&mut self, ty: &ast::TypeExpr) -> Option<Type> {
        let (what, section) = match &ty.kind {
            ast::TypeExprKind::Named { module, name, args } if module.is_empty() => {
                return self.named_type(name, args, ty);
            }
            ast::TypeExprKind::Named { .. } => ("modules", "§20"),
            ast::TypeExprKind::Maybe(_) | ast::TypeExprKind::Union(_) => {
                ("union types (`or`, `maybe`)", "§7.3")
            }
            ast::TypeExprKind::Tuple(_) => ("tuples", "§16"),
            ast::TypeExprKind::Fun { .. } => ("function types", "§7.2, §11"),
        };
        self.not_implemented(ty.span, what, section);
        None
    }

    fn named_type(&mut self, name: &ast::Ident, args: &[ast::TypeExpr], ty: &ast::TypeExpr) -> Option<Type> {
        if let Some(&(_, what, section)) = PLANNED.iter().find(|(planned, ..)| *planned == name.name) {
            self.not_implemented(ty.span, what, section);
            return None;
        }
        let Some(&(_, resolved)) = SUPPORTED.iter().find(|(known, _)| *known == name.name) else {
            let known = SUPPORTED.iter().map(|(known, _)| *known).chain(PLANNED.iter().map(|(p, ..)| *p));
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
