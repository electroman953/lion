//! Name resolution and type checking: from the syntax tree to the typed IR.
//!
//! Every compile error that is not a syntax error comes from here, whatever the
//! execution mode (spec §22.1). The checker reports all the problems it finds and
//! builds an [`ir::Program`] only when there is none.

mod expr;
mod names;
mod types;

use std::collections::HashMap;

use lion_diagnostics::{Diagnostic, Span};
use lion_ir::{self as ir, Type};
use lion_syntax::ast;

pub struct Checked {
    /// Present only when there are no errors.
    pub program: Option<ir::Program>,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn check(module: &ast::Module) -> Checked {
    let mut checker = Checker::default();
    for stmt in &module.stmts {
        checker.stmt(stmt);
    }
    checker.finish()
}

/// What the checker knows about a local.
struct LocalInfo {
    name: String,
    /// `None` when its declaration had an error, already reported.
    ty: Option<Type>,
    mutable: bool,
    temporary: bool,
    decl_span: Span,
    /// Declared with a value (`let x = 1`) rather than without (`let x in Int`).
    initialized: bool,
    /// Where it received its first value, when declared without one.
    assigned_at: Option<Span>,
}

impl LocalInfo {
    fn has_value(&self) -> bool {
        self.initialized || self.assigned_at.is_some()
    }
}

#[derive(Default)]
struct Checker {
    diagnostics: Vec<Diagnostic>,
    locals: Vec<LocalInfo>,
    /// The names visible in the file's top-level block. Declaring a name again in the
    /// same block replaces the entry: the new binding hides the old one (§6.3).
    scope: HashMap<String, ir::LocalId>,
    body: Vec<ir::Stmt>,
}

impl Checker {
    fn stmt(&mut self, stmt: &ast::Stmt) {
        match &stmt.kind {
            ast::StmtKind::Let(decl) => self.let_stmt(decl),
            ast::StmtKind::Assign { target, op, value, .. } => self.assign(target, *op, value, stmt.span),
            ast::StmtKind::Expr(expr) => {
                if let Some(expr) = self.expr(expr) {
                    self.body.push(ir::Stmt::Expr(expr));
                }
            }
        }
    }

    /// `let x = value in T` and its variants (§6.1, §6.2).
    fn let_stmt(&mut self, decl: &ast::LetStmt) {
        self.check_not_standard_name(&decl.name);
        let annotation = decl.annotation.as_ref().map(|ty| (self.resolve_type(ty), ty.span));
        let mut ty = annotation.and_then(|(ty, _)| ty);
        let mut value = None;
        // The value is checked before the name is declared: in `let x = x + 1`, the
        // `x` on the right is the previous binding.
        if let Some(expr) = &decl.value
            && let Some(checked) = self.expr(expr)
        {
            value = match annotation {
                Some((Some(expected), span)) => {
                    self.coerce(checked, expected, Some((span, "expected because of this type".to_string())))
                }
                Some((None, _)) => None,
                None => {
                    ty = Some(checked.ty);
                    Some(checked)
                }
            };
        }
        let local = self.declare(&decl.name, ty, decl.mutable, decl.value.is_some());
        if let Some(value) = value {
            self.body.push(ir::Stmt::Assign { local, value });
        }
    }

    /// `x = value`, `x += value`, ... (§6.3, D44).
    fn assign(&mut self, target: &ast::Expr, op: ast::AssignOp, value: &ast::Expr, span: Span) {
        let name = match &target.kind {
            ast::ExprKind::Name(name) => name,
            ast::ExprKind::Field { .. } | ast::ExprKind::Index { .. } => {
                self.not_implemented(target.span, "assigning to a field or an element", "§6.3, §12, §16");
                return;
            }
            _ => {
                self.diagnostics.push(
                    Diagnostic::error("cannot assign to this expression")
                        .with_primary(target.span, "")
                        .with_note("the left side of an assignment is a variable"),
                );
                return;
            }
        };
        let value = self.expr(value);
        let Some(local) = self.lookup(name) else {
            let error = self
                .unknown_name_error(name, target.span)
                .with_help(format!("a variable is declared before it is used: `var {name} = ...` (§6)"));
            self.diagnostics.push(error);
            return;
        };
        if op != ast::AssignOp::Set && !self.check_has_value(local, target.span) {
            return;
        }
        if !self.check_assignable(local, target.span) {
            return;
        }
        let info = &mut self.locals[local.index()];
        if !info.has_value() {
            info.assigned_at = Some(target.span);
        }
        let (Some(ty), Some(value)) = (info.ty, value) else { return };
        let (decl_span, name) = (info.decl_span, info.name.clone());
        let value = match compound_operator(op) {
            None => value,
            Some(op) => {
                let current = typed(ir::ExprKind::Local(local), ty, target.span);
                let Some(result) = self.arithmetic(op, span, current, value, span) else { return };
                if result.ty != ty && !(result.ty == Type::Int && ty == Type::Float) {
                    self.diagnostics.push(
                        Diagnostic::error("mismatched types")
                            .with_primary(span, format!("the result is {}", article(result.ty)))
                            .with_secondary(
                                decl_span,
                                format!("`{name}` is declared as {} here", article(ty)),
                            )
                            .with_note(format!("expected: {ty}"))
                            .with_note(format!("found: {}", result.ty)),
                    );
                    return;
                }
                result
            }
        };
        let context = (decl_span, format!("`{name}` is declared as {} here", article(ty)));
        let Some(value) = self.coerce(value, ty, Some(context)) else {
            if let Some(error) = self.diagnostics.last_mut() {
                error.help.push(format!(
                    "to give `{name}` a value of another type, declare it again: `let {name} = ...` (§6.3)"
                ));
            }
            return;
        };
        self.body.push(ir::Stmt::Assign { local, value });
    }

    /// A constant changes at most once: a `let` without a value receives exactly one
    /// assignment, and a `let` with a value none (§6.1).
    fn check_assignable(&mut self, local: ir::LocalId, span: Span) -> bool {
        let info = &self.locals[local.index()];
        if info.mutable {
            return true;
        }
        let name = &info.name;
        let error = if let Some(first) = info.assigned_at {
            Diagnostic::error(format!("the constant `{name}` already has a value"))
                .with_primary(span, "second assignment")
                .with_secondary(first, "first assignment")
                .with_note("a `let` declared without a value receives exactly one assignment (§6.1)")
                .with_help(format!("to change `{name}`, declare it with `var`"))
        } else if info.initialized {
            Diagnostic::error(format!("cannot assign to the constant `{name}`"))
                .with_primary(span, "")
                .with_secondary(info.decl_span, "declared with `let`")
                .with_help(format!("to change `{name}`, declare it with `var` (§6)"))
        } else {
            return true;
        };
        self.diagnostics.push(error);
        false
    }

    fn check_has_value(&mut self, local: ir::LocalId, span: Span) -> bool {
        let info = &self.locals[local.index()];
        if info.has_value() {
            return true;
        }
        let name = &info.name;
        self.diagnostics.push(
            Diagnostic::error(format!("`{name}` is used before it has a value"))
                .with_primary(span, "used here")
                .with_secondary(info.decl_span, "declared here without a value")
                .with_help(format!("give `{name}` a value before this line (§6.1)")),
        );
        false
    }

    fn declare(
        &mut self,
        name: &ast::Ident,
        ty: Option<Type>,
        mutable: bool,
        initialized: bool,
    ) -> ir::LocalId {
        let id = ir::LocalId(self.locals.len() as u32);
        self.locals.push(LocalInfo {
            name: name.name.clone(),
            ty,
            mutable,
            temporary: false,
            decl_span: name.span,
            initialized,
            assigned_at: None,
        });
        self.scope.insert(name.name.clone(), id);
        id
    }

    /// A local introduced by the checker, invisible to the program.
    fn temporary(&mut self, ty: Type, span: Span) -> ir::LocalId {
        let id = ir::LocalId(self.locals.len() as u32);
        self.locals.push(LocalInfo {
            name: "%t".to_string(),
            ty: Some(ty),
            mutable: false,
            temporary: true,
            decl_span: span,
            initialized: true,
            assigned_at: None,
        });
        id
    }

    fn lookup(&self, name: &str) -> Option<ir::LocalId> {
        self.scope.get(name).copied()
    }

    fn not_implemented(&mut self, span: Span, what: &str, section: &str) {
        self.diagnostics.push(Diagnostic::not_implemented(span, what, section));
    }

    fn finish(mut self) -> Checked {
        let unassigned: Vec<Diagnostic> = self
            .locals
            .iter()
            .filter(|info| !info.mutable && !info.temporary && !info.has_value() && info.ty.is_some())
            .map(|info| {
                Diagnostic::error(format!("the constant `{}` never receives a value", info.name))
                    .with_primary(info.decl_span, "declared here without a value")
                    .with_note("a `let` declared without a value receives exactly one assignment (§6.1)")
            })
            .collect();
        self.diagnostics.extend(unassigned);
        if self.diagnostics.iter().any(Diagnostic::is_fatal) {
            return Checked { program: None, diagnostics: self.diagnostics };
        }
        let locals = self
            .locals
            .into_iter()
            .map(|info| ir::Local {
                name: info.name,
                ty: info.ty.expect("every local of a valid program has a type"),
                mutable: info.mutable,
                temporary: info.temporary,
                span: info.decl_span,
            })
            .collect();
        Checked { program: Some(ir::Program { locals, body: self.body }), diagnostics: self.diagnostics }
    }
}

fn compound_operator(op: ast::AssignOp) -> Option<ast::BinaryOp> {
    match op {
        ast::AssignOp::Set => None,
        ast::AssignOp::Add => Some(ast::BinaryOp::Add),
        ast::AssignOp::Sub => Some(ast::BinaryOp::Sub),
        ast::AssignOp::Mul => Some(ast::BinaryOp::Mul),
    }
}

fn typed(kind: ir::ExprKind, ty: Type, span: Span) -> ir::Expr {
    ir::Expr { kind, ty, span }
}

/// "an Int", "a Text": how a value of the type is named in messages.
fn article(ty: Type) -> &'static str {
    match ty {
        Type::Int => "an Int",
        Type::Float => "a Float",
        Type::Bool => "a Bool",
        Type::Text => "a Text",
        Type::None => "`none`",
    }
}

#[cfg(test)]
mod tests;
