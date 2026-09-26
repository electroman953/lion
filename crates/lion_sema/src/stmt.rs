//! Type checking of statements and blocks (spec §5, §6, §10).

use lion_diagnostics::{Diagnostic, Span};
use lion_ir::{self as ir, Type};
use lion_syntax::ast;

use crate::flow::{Assigned, Flow};
use crate::{Checker, LoopExits, Scope, article, typed};

impl Checker {
    pub(crate) fn stmts(&mut self, stmts: &[ast::Stmt]) -> Vec<ir::Stmt> {
        stmts.iter().filter_map(|stmt| self.stmt(stmt)).collect()
    }

    /// A block has its own scope: its declarations end with it.
    fn block(&mut self, block: &ast::Block) -> Vec<ir::Stmt> {
        self.scopes.push(Scope::default());
        let stmts = self.stmts(&block.stmts);
        self.close_scope();
        stmts
    }

    pub(crate) fn close_scope(&mut self) {
        let scope = self.scopes.pop().expect("a scope is open");
        self.check_constants_assigned(&scope.declared);
    }

    fn stmt(&mut self, stmt: &ast::Stmt) -> Option<ir::Stmt> {
        match &stmt.kind {
            ast::StmtKind::Let(decl) => self.let_stmt(decl),
            ast::StmtKind::Assign { target, op, value, .. } => self.assign(target, *op, value, stmt.span),
            ast::StmtKind::Expr(expr) => self.expr(expr).map(ir::Stmt::Expr),
            ast::StmtKind::If { branches, otherwise } => self.if_stmt(branches, otherwise.as_ref()),
            ast::StmtKind::While { cond, body } => self.while_stmt(cond, body),
            ast::StmtKind::Break => self.jump(stmt.span, true),
            ast::StmtKind::Continue => self.jump(stmt.span, false),
            ast::StmtKind::Return(value) => self.return_stmt(value.as_ref()),
        }
    }

    /// `let x = value in T` and its variants (§6.1, §6.2).
    fn let_stmt(&mut self, decl: &ast::LetStmt) -> Option<ir::Stmt> {
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
        value.map(|value| ir::Stmt::Assign { local, value })
    }

    /// `x = value`, `x += value`, ... (§6.3, D44).
    fn assign(
        &mut self,
        target: &ast::Expr,
        op: ast::AssignOp,
        value: &ast::Expr,
        span: Span,
    ) -> Option<ir::Stmt> {
        let name = match &target.kind {
            ast::ExprKind::Name(name) => name,
            ast::ExprKind::Field { .. } | ast::ExprKind::Index { .. } => {
                self.not_implemented(target.span, "assigning to a field or an element", "§6.3, §12, §16");
                return None;
            }
            _ => {
                self.diagnostics.push(
                    Diagnostic::error("cannot assign to this expression")
                        .with_primary(target.span, "")
                        .with_note("the left side of an assignment is a variable"),
                );
                return None;
            }
        };
        let value = self.expr(value);
        let Some(local) = self.lookup(name) else {
            let error = self
                .unknown_name_error(name, target.span)
                .with_help(format!("a variable is declared before it is used: `var {name} = ...` (§6)"));
            self.diagnostics.push(error);
            return None;
        };
        if op != ast::AssignOp::Set && !self.check_has_value(local, target.span) {
            return None;
        }
        let assignable = self.check_assignable(local, target.span);
        // Even a refused assignment gives the variable a value, to report it only once.
        self.locals[local.index()].first_assignment.get_or_insert(target.span);
        self.flow.set(local, Assigned::Yes);
        if !assignable {
            return None;
        }
        let info = &self.locals[local.index()];
        let (ty, decl_span, name) = (info.ty?, info.decl_span, info.name.clone());
        let value = value?;
        let value = match compound_operator(op) {
            None => value,
            Some(op) => {
                let current = typed(ir::ExprKind::Local(local), ty, target.span);
                let result = self.arithmetic(op, span, current, value, span)?;
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
                    return None;
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
            return None;
        };
        Some(ir::Stmt::Assign { local, value })
    }

    /// `if c: ... elif d: ... else: ... ;` (§5.2). Each branch starts from the flow
    /// where its condition is tested; the paths meet after the `;`.
    fn if_stmt(&mut self, branches: &[ast::Branch], otherwise: Option<&ast::Block>) -> Option<ir::Stmt> {
        let mut ends = Vec::new();
        let mut checked = Vec::new();
        for branch in branches {
            let cond = self.condition(&branch.cond, "if");
            let before = self.flow.clone();
            let body = self.block(&branch.body);
            ends.push(std::mem::replace(&mut self.flow, before));
            checked.push((cond, body));
        }
        // Without `else`, the flow where every condition is false goes on.
        let mut result = otherwise.map(|block| self.block(block)).unwrap_or_default();
        self.flow = ends.into_iter().fold(self.flow.clone(), Flow::join);
        for (cond, then) in checked.into_iter().rev() {
            result = vec![ir::Stmt::If { cond: cond?, then, otherwise: result }];
        }
        result.pop()
    }

    /// `while c: ... ;` (§10.2).
    fn while_stmt(&mut self, cond: &ast::Expr, body: &ast::Block) -> Option<ir::Stmt> {
        // At the start of a turn, a local assigned in the body may hold a value from an
        // earlier turn.
        for local in self.assigned_in(&body.stmts) {
            if self.flow.is_reachable() && self.flow.get(local) == Assigned::No {
                self.flow.set(local, Assigned::Maybe);
            }
        }
        let cond_ir = self.condition(cond, "while");
        let head = self.flow.clone();
        self.loops.push(LoopExits::default());
        let body_ir = self.block(body);
        let exits = self.loops.pop().expect("the loop is open");
        // `while true` only ends through `break`: no path leaves it when the condition is false.
        let after = if matches!(cond.kind, ast::ExprKind::Bool(true)) { Flow::unreachable() } else { head };
        self.flow = exits.breaks.into_iter().fold(after, Flow::join);
        Some(ir::Stmt::While { cond: cond_ir?, body: body_ir })
    }

    /// `break` or `continue`, in the innermost loop (§10.2).
    fn jump(&mut self, span: Span, is_break: bool) -> Option<ir::Stmt> {
        let keyword = if is_break { "break" } else { "continue" };
        let Some(exits) = self.loops.last_mut() else {
            self.diagnostics.push(
                Diagnostic::error(format!("`{keyword}` outside a loop"))
                    .with_primary(span, "")
                    .with_note(format!("`{keyword}` is used inside a `while` loop (§10.2)")),
            );
            return None;
        };
        // After `continue`, the flow goes back to the start of the loop, whose state
        // already accounts for every assignment of the body.
        let flow = std::mem::replace(&mut self.flow, Flow::unreachable());
        if is_break {
            exits.breaks.push(flow);
        }
        Some(if is_break { ir::Stmt::Break } else { ir::Stmt::Continue })
    }

    /// `return` ends the script (§20.1).
    fn return_stmt(&mut self, value: Option<&ast::Expr>) -> Option<ir::Stmt> {
        if let Some(value) = value {
            self.diagnostics.push(
                Diagnostic::error("a script cannot return a value")
                    .with_primary(value.span, "")
                    .with_note("at the level of a script, `return` alone ends the script (§20.1)"),
            );
            return None;
        }
        self.flow = Flow::unreachable();
        Some(ir::Stmt::Return)
    }

    /// The outer locals that `stmts` assigns, at any depth.
    fn assigned_in(&self, stmts: &[ast::Stmt]) -> Vec<ir::LocalId> {
        let mut found = Vec::new();
        for stmt in stmts {
            match &stmt.kind {
                ast::StmtKind::Assign { target, .. } => {
                    if let ast::ExprKind::Name(name) = &target.kind
                        && let Some(local) = self.lookup(name)
                    {
                        found.push(local);
                    }
                }
                ast::StmtKind::If { branches, otherwise } => {
                    for branch in branches {
                        found.extend(self.assigned_in(&branch.body.stmts));
                    }
                    if let Some(otherwise) = otherwise {
                        found.extend(self.assigned_in(&otherwise.stmts));
                    }
                }
                ast::StmtKind::While { body, .. } => found.extend(self.assigned_in(&body.stmts)),
                _ => {}
            }
        }
        found
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
