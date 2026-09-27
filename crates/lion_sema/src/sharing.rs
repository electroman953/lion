//! Explicit sharing (spec §17.2, §19.3): `var score = shared Counter()`.
//!
//! Sharing belongs to variables: a declaration `shared` makes an object, and the names
//! that designate it are known where they are declared. A `let` of a `let shared` names
//! the same object; any other assignment, and any argument of a parameter that is not
//! `var`, is a frozen copy. So two names of the same object are always visible from the
//! same place, and `a same b` is known when the program is checked (C73). What changes
//! an object is already expressed by `var` parameters and `modifies`, which share the
//! variable itself.
//!
//! Between tasks, a `shared` object is refused; a `shared synced` one is allowed, a lock
//! protecting each access (§19.3). This version runs the parallel parts one after the
//! other, so the lock has nothing to do yet (C60).

use std::rc::Rc;

use lion_diagnostics::{Diagnostic, Severity, Span};
use lion_ir::{self as ir, Type};
use lion_syntax::ast;

use crate::parallel::Variable;
use crate::{Checker, ContextKind, typed};

/// How a variable names a shared object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Sharing {
    /// The `shared` expression that made the object: the same for all its names.
    pub(crate) origin: Span,
    /// `shared synced`: tasks may use it (§19.3).
    pub(crate) synced: bool,
}

impl Checker<'_> {
    /// `let x = ...` or `var x = ...`: with `shared`, the variable names a new object;
    /// `let t = s`, with `s` a `let shared`, names the same one (§17.2).
    pub(crate) fn let_stmt(&mut self, decl: &ast::LetStmt) -> Option<ir::Stmt> {
        if let Some(ast::Expr { kind: ast::ExprKind::Shared { value, synced }, span }) = &decl.value {
            let plain = ast::LetStmt { value: Some((**value).clone()), ..decl.clone() };
            let stmt = self.plain_let(&plain);
            self.mark_shared(&decl.name, Sharing { origin: *span, synced: *synced });
            return stmt;
        }
        // The value names a `let shared`: a `let` names the same object, a `var` cannot.
        let source = decl.value.as_ref().and_then(|value| self.sharing_of_expr(value));
        let stmt = self.plain_let(decl);
        if let Some((sharing, source_mutable)) = source
            && !source_mutable
        {
            if decl.mutable {
                let value = decl.value.as_ref().expect("a value names the object");
                self.diagnostics.push(
                    Diagnostic::error(format!(
                        "`{}` would change an object that nothing may change",
                        decl.name.name
                    ))
                    .with_primary(value.span, "this names a `let shared` object")
                    .with_secondary(sharing.origin, "made here")
                    .with_note("a `let` never changes, even when it holds a shared object (§17.2)")
                    .with_help(format!("declare it with `let`: `let {} = ...`", decl.name.name)),
                );
                return None;
            }
            self.mark_shared(&decl.name, sharing);
        }
        stmt
    }

    fn mark_shared(&mut self, name: &ast::Ident, sharing: Sharing) {
        let Some(local) = self.lookup(&name.name) else { return };
        self.ctx.locals[local.index()].shared = Some(sharing);
        let global = matches!(self.ctx.kind, ContextKind::Script | ContextKind::Init(_))
            && self.global_names.contains_key(&local);
        if global {
            self.global_sharing.insert(local, sharing);
        }
        if sharing.synced && !self.synced.iter().any(|(known, _)| *known == sharing) {
            self.synced.push((sharing, name.name.clone()));
        }
    }

    /// The object that `expr` names, if it is a variable that names a shared object,
    /// with whether that variable is a `var`. Nothing is reported.
    pub(crate) fn sharing_of_expr(&self, expr: &ast::Expr) -> Option<(Sharing, bool)> {
        match &expr.kind {
            ast::ExprKind::Paren(inner) => self.sharing_of_expr(inner),
            ast::ExprKind::Name(name) => {
                if let Some(local) = self.lookup(name) {
                    let info = &self.ctx.locals[local.index()];
                    return info.shared.map(|sharing| (sharing, info.mutable));
                }
                if self.ctx.kind == ContextKind::Script {
                    return None;
                }
                let global = self.tables.globals.get(name)?;
                let sharing = self.global_sharing.get(&global.local?)?;
                Some((*sharing, global.decl.mutable))
            }
            _ => None,
        }
    }

    /// The sharing of a resolved variable.
    pub(crate) fn sharing_of(&self, variable: Variable) -> Option<Sharing> {
        match variable {
            Variable::Local(local) => self.ctx.locals[local.index()].shared,
            Variable::Global(global) => self.global_sharing.get(&global).copied(),
        }
    }

    /// `shared` anywhere but as the value of a declaration (C73).
    pub(crate) fn misplaced_shared(&mut self, value: &ast::Expr, span: Span) -> Option<ir::Expr> {
        self.expr(value);
        self.diagnostics.push(
            Diagnostic::error("`shared` makes the object of a variable being declared")
                .with_primary(span, "")
                .with_note(
                    "a shared object is held by variables: `var score = shared Counter()` (§17.2, C73)",
                )
                .with_help("declare a variable with it, and use the variable"),
        );
        None
    }

    /// `a same b`: whether two names designate the same shared object (§9.4, §17.2).
    pub(crate) fn same(&mut self, lhs: &ast::Expr, rhs: &ast::Expr, span: Span) -> Option<ir::Expr> {
        let checked = [self.expr(lhs), self.expr(rhs)];
        let mut origins = Vec::new();
        for operand in [lhs, rhs] {
            match self.sharing_of_expr(operand) {
                Some((sharing, _)) => origins.push(sharing.origin),
                None => self.diagnostics.push(
                    Diagnostic::error("`same` compares objects made with `shared`")
                        .with_primary(operand.span, "this is not a variable that names a shared object")
                        .with_note(
                            "`a same b` tells whether two names designate the same shared object (§9.4)",
                        )
                        .with_help("to compare the content of two values, use `==`"),
                ),
            }
        }
        let [lhs, rhs] = checked;
        lhs?;
        rhs?;
        let [first, second] = origins.as_slice() else { return None };
        Some(typed(ir::ExprKind::Bool(first == second), Type::Bool, span))
    }

    /// A shared object used in a task or a parallel part, from outside it: refused,
    /// unless it is `synced` (§19.3).
    pub(crate) fn check_shared_in_parallel(&mut self, variable: Variable, name: &str, span: Span) {
        let Some(region) = self.ctx.parallel else { return };
        let Some(sharing) = self.sharing_of(variable) else { return };
        if let Variable::Local(local) = variable
            && !region.is_outside(local)
        {
            return;
        }
        if sharing.synced {
            self.synced_used.insert(sharing.origin);
            return;
        }
        if !self.shared_reported.insert(span) {
            return;
        }
        self.diagnostics.push(
            Diagnostic::error(format!("`{name}` is shared: a task or a parallel part cannot use it"))
                .with_primary(span, "")
                .with_secondary(region.span, "runs in parallel")
                .with_secondary(sharing.origin, "shared here")
                .with_note("two tasks could use the object at the same time (§19.3)")
                .with_help("declare it `shared synced`: a lock then protects each access"),
        );
    }

    /// A `synced` that no task nor parallel part uses (§19.3, D53).
    pub(crate) fn check_synced(&mut self) {
        let synced = std::mem::take(&mut self.synced);
        for (sharing, name) in synced {
            if self.synced_used.contains(&sharing.origin) {
                continue;
            }
            self.diagnostics.push(
                Diagnostic::new(Severity::Warning, format!("`{name}` is `synced`, but no task nor parallel part uses it"))
                    .with_primary(sharing.origin, "")
                    .with_note("a lock protects an object that several tasks use; here, it only slows the accesses (§19.3, D53)")
                    .with_help("remove `synced`"),
            );
        }
    }

    /// `score.increment`, with `score` a `var shared`: a function that changes that
    /// object, as `fun(...) modifies score = score.increment(...)` (§12.6).
    pub(crate) fn detached_shared_method(
        &mut self,
        method: usize,
        object: &ast::Expr,
        name: &ast::Ident,
        span: Span,
    ) -> Option<ir::Expr> {
        let ast::ExprKind::Name(variable) = &unparen(object).kind else { return None };
        let Some(instance) = self.functions[method].instance else {
            self.diagnostics.push(
                Diagnostic::error(format!(
                    "the types of the parameters of `{}` are not known here",
                    name.name
                ))
                .with_primary(span, "")
                .with_note("a method with parameters without a type is generic (§15.3)"),
            );
            return None;
        };
        let params = self.functions[method].signature.clone()?;
        let ret = self.return_type_of(instance, span)?;
        let method_decl = Rc::clone(&self.functions[method].decl);
        let at = |name: &str| ast::Ident { name: name.to_string(), span };
        let expr = |kind: ast::ExprKind| ast::Expr { kind, span };
        let names: Vec<ast::Ident> = method_decl
            .params
            .iter()
            .filter(|param| param.name.name != "self")
            .map(|param| at(&param.name.name))
            .collect();
        let args = names
            .iter()
            .map(|param| ast::Arg {
                var_marker: None,
                name: None,
                value: expr(ast::ExprKind::Name(param.name.clone())),
            })
            .collect();
        let callee = expr(ast::ExprKind::Field {
            object: Box::new(expr(ast::ExprKind::Name(variable.clone()))),
            name: at(&name.name),
        });
        let body = expr(ast::ExprKind::Call { callee: Box::new(callee), args });
        let decl = ast::FunDecl {
            private: None,
            foreign: None,
            infix: false,
            receiver: None,
            name: at("fun"),
            params: names
                .into_iter()
                .map(|name| ast::Param { var: None, name, ty: None, default: None })
                .collect(),
            ret: None,
            type_params: Vec::new(),
            modifies: vec![at(variable)],
            body: ast::FunBody::Expr(body),
        };
        let types: Vec<Type> = params[1..].iter().filter_map(|param| param.ty).collect();
        if types.len() + 1 != params.len() {
            return None;
        }
        let required = types.len() as u32;
        self.closure(&decl, span, Some(ir::FunData { params: types, required, ret }))
    }
}

fn unparen(expr: &ast::Expr) -> &ast::Expr {
    match &expr.kind {
        ast::ExprKind::Paren(inner) => unparen(inner),
        _ => expr,
    }
}
