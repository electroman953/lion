//! Type checking of statements and blocks (spec §5, §6, §10).

use lion_diagnostics::{Diagnostic, Span};
use lion_ir::{self as ir, Type};
use lion_syntax::ast;

use crate::flow::{Assigned, Flow};
use crate::names::Resolved;
use crate::{Checker, ContextKind, GlobalType, LoopExits, Scope, article, typed};

impl Checker<'_> {
    pub(crate) fn stmts(&mut self, stmts: &[ast::Stmt]) -> Vec<ir::Stmt> {
        stmts.iter().filter_map(|stmt| self.stmt(stmt)).collect()
    }

    /// A block has its own scope: its declarations end with it.
    fn block(&mut self, block: &ast::Block) -> Vec<ir::Stmt> {
        self.ctx.scopes.push(Scope::default());
        let stmts = self.stmts(&block.stmts);
        self.close_scope();
        stmts
    }

    pub(crate) fn close_scope(&mut self) {
        let scope = self.ctx.scopes.pop().expect("a scope is open");
        self.check_constants_assigned(&scope.declared);
    }

    fn stmt(&mut self, stmt: &ast::Stmt) -> Option<ir::Stmt> {
        match &stmt.kind {
            ast::StmtKind::Let(decl) => self.let_stmt(decl),
            ast::StmtKind::Assign { target, op, value, .. } => self.assign(target, *op, value, stmt.span),
            ast::StmtKind::Expr(expr) => match &expr.kind {
                // `l.add(value)` changes the list in place.
                ast::ExprKind::Call { callee, args } if matches!(&callee.kind, ast::ExprKind::Field { name, .. } if name.name == "add") =>
                {
                    let ast::ExprKind::Field { object, .. } = &callee.kind else { unreachable!() };
                    self.add_stmt(object, args, expr.span)
                }
                _ => {
                    let checked = self.expr(expr)?;
                    // Nothing runs after `exit` (§20.1).
                    if let ir::ExprKind::CallBuiltin { builtin: ir::Builtin::Exit, .. } = checked.kind {
                        self.ctx.flow = Flow::unreachable();
                    }
                    Some(ir::Stmt::Expr(checked))
                }
            },
            ast::StmtKind::If { branches, otherwise } => self.if_stmt(branches, otherwise.as_ref()),
            ast::StmtKind::While { cond, body } => self.while_stmt(cond, body),
            ast::StmtKind::For { parallel, var, iterable, body } => {
                self.for_stmt(var, iterable, body, parallel.map(|_| stmt.span))
            }
            ast::StmtKind::Match { scrutinee, cases } => self.match_stmt(scrutinee, cases, stmt.span),
            ast::StmtKind::Break => self.jump(stmt.span, true),
            ast::StmtKind::Continue => self.jump(stmt.span, false),
            ast::StmtKind::Return(value) => match self.ctx.kind {
                ContextKind::Function(index) => self.return_in_function(index, value.as_ref(), stmt.span),
                ContextKind::Script => self.return_stmt(value.as_ref()),
                ContextKind::Structure(_) | ContextKind::Init(_) => {
                    unreachable!("a structure and the globals of a module have no statements")
                }
            },
            // Top-level functions are registered beforehand and checked on their own.
            ast::StmtKind::Fun(_) if self.ctx.kind == ContextKind::Script && self.ctx.scopes.len() == 1 => {
                None
            }
            ast::StmtKind::Fun(decl) => self.local_function(decl, stmt.span),
            // `use` is resolved beforehand, from the top level only (§20.2).
            ast::StmtKind::Use(_) if self.ctx.kind == ContextKind::Script && self.ctx.scopes.len() == 1 => {
                None
            }
            ast::StmtKind::Use(path) => {
                self.diagnostics.push(
                    Diagnostic::error("`use` is written at the top level of the file")
                        .with_primary(path[0].span, "")
                        .with_help("move it to the start of the file (§20.2)"),
                );
                None
            }
            // Types are registered beforehand, from the top level only.
            ast::StmtKind::Struct(_) | ast::StmtKind::TypeDef(_)
                if self.ctx.kind == ContextKind::Script && self.ctx.scopes.len() == 1 =>
            {
                None
            }
            ast::StmtKind::Struct(ast::StructDecl { name, .. })
            | ast::StmtKind::TypeDef(ast::TypeDef { name, .. }) => {
                self.diagnostics.push(
                    Diagnostic::error("a type is declared at the top level of the file")
                        .with_primary(name.span, "")
                        .with_help("move this declaration out of the block"),
                );
                None
            }
        }
    }

    /// `let x = value in T` and its variants (§6.1, §6.2).
    fn let_stmt(&mut self, decl: &ast::LetStmt) -> Option<ir::Stmt> {
        let annotation = decl.annotation.as_ref().map(|ty| (self.resolve_type(ty), ty.span));
        // `let s = ("Léa", 12) in Student` builds a Student, as `Student("Léa", 12)` (§6.2, §12.2).
        if let (Some((Some(Type::Struct(structure)), _)), Some(value)) = (annotation, &decl.value)
            && let ast::ExprKind::Tuple(elements) = &value.kind
        {
            let index = self.struct_index(structure);
            let built = self.construct(index, &crate::structs::Given::elements(elements), value.span);
            let local = self.declare(&decl.name, built.as_ref().map(|value| value.ty), decl.mutable, true);
            return built.map(|value| ir::Stmt::Assign { place: ir::Place::Local(local), value });
        }
        let mut ty = annotation.and_then(|(ty, _)| ty);
        let mut value = None;
        // The value is checked before the name is declared: in `let x = x + 1`, the
        // `x` on the right is the previous binding.
        let expected = annotation.and_then(|(ty, _)| ty);
        let checked = decl.value.as_ref().and_then(|expr| match expected {
            Some(expected) => self.expr_expecting(expr, expected),
            // The annotation has an error: an empty list cannot get its type from it.
            None if annotation.is_some() && is_empty_list(expr) => None,
            None => self.expr(expr),
        });
        if let Some(checked) = checked {
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
        if let Some(value) = &value {
            self.narrow_to_value(local, value.ty);
        }
        let assign = value.map(|value| ir::Stmt::Assign { place: ir::Place::Local(local), value });
        // A `var` starts here: a nested function may share it later (§11.5).
        let global = self.ctx.kind == ContextKind::Script && self.global_names.contains_key(&local);
        if !decl.mutable || global {
            return assign;
        }
        let declare = ir::Stmt::Declare { local };
        Some(match assign {
            Some(assign) => ir::Stmt::Seq(vec![declare, assign]),
            None => declare,
        })
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
            ast::ExprKind::Index { .. } => {
                return match compound_operator(op) {
                    None => self.change_in_place(target, value, false),
                    Some(op) => self.compound_element(target, op, value, span),
                };
            }
            ast::ExprKind::Field { .. } => {
                return match compound_operator(op) {
                    None => self.change_in_place(target, value, false),
                    Some(op) => self.compound_element(target, op, value, span),
                };
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
        // An empty list takes the type of the variable, known once it is resolved.
        let value_ast = value;
        let deferred = is_empty_list(value) || self.is_bare_unknown_name(value);
        let value = if deferred { None } else { self.expr(value) };
        if !self.is_known(name) {
            let error = self
                .unknown_name_error(name, target.span)
                .with_help(format!("a variable is declared before it is used: `var {name} = ...` (§6)"));
            self.diagnostics.push(error);
            return None;
        }
        let (place, ty, decl_span) = match self.resolve(name, target.span) {
            Resolved::Local(local) => {
                if op != ast::AssignOp::Set && !self.check_has_value(local, target.span) {
                    return None;
                }
                let assignable = self.check_assignable(local, target.span)
                    && !self.changes_outside_parallel(Some(local), name, target.span);
                // Even a refused assignment gives the variable a value, to report it only once.
                self.ctx.locals[local.index()].first_assignment.get_or_insert(target.span);
                self.ctx.flow.set(local, Assigned::Yes);
                if !assignable {
                    return None;
                }
                let info = &self.ctx.locals[local.index()];
                (ir::Place::Local(local), info.ty?, info.decl_span)
            }
            Resolved::Global(local) => {
                if !self.check_global_assignment(local, target.span)
                    || self.changes_outside_parallel(None, name, target.span)
                {
                    return None;
                }
                if op != ast::AssignOp::Set {
                    self.ctx.reads.push(local);
                }
                let global = &self.global_info(local);
                let GlobalType::Known(ty) = global.ty else { return None };
                (ir::Place::Global(local), ty?, global.decl.name.span)
            }
            Resolved::Function(_) | Resolved::Standard(_) => {
                self.diagnostics.push(
                    Diagnostic::error(format!("cannot assign to the function `{name}`"))
                        .with_primary(target.span, "")
                        .with_note("the left side of an assignment is a variable"),
                );
                return None;
            }
            Resolved::Nothing => return None,
        };
        let value = if deferred { self.expr_expecting(value_ast, ty) } else { value };
        let value = value?;
        let value = match compound_operator(op) {
            None => value,
            Some(op) => {
                let current = match place {
                    ir::Place::Local(local) => ir::ExprKind::Local(local),
                    ir::Place::Global(local) => ir::ExprKind::Global(local),
                };
                let current = typed(current, ty, target.span);
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
        let value_type = value.ty;
        let coerced = self.coerce(value, ty, Some(context));
        if let (ir::Place::Local(local), Some(value)) = (place, &coerced) {
            self.narrow_to_value(local, value.ty);
        }
        let Some(value) = coerced else {
            // A union that was not tested is the problem, not the type of the variable.
            if !ty.is_subset_of(value_type)
                && let Some(error) = self.diagnostics.last_mut()
            {
                error.help.push(format!(
                    "to give `{name}` a value of another type, declare it again: `let {name} = ...` (§6.3)"
                ));
            }
            return None;
        };
        Some(ir::Stmt::Assign { place, value })
    }

    /// `l.add(value)`.
    fn add_stmt(&mut self, list: &ast::Expr, args: &[ast::Arg], span: Span) -> Option<ir::Stmt> {
        let [arg] = args else {
            self.diagnostics.push(
                Diagnostic::error(format!("`add` takes one value, not {}", args.len()))
                    .with_primary(span, ""),
            );
            return None;
        };
        if arg.name.is_some() || arg.var_marker.is_some() {
            self.diagnostics.push(
                Diagnostic::error("the value given to `add` is written alone")
                    .with_primary(arg.value.span, ""),
            );
            return None;
        }
        self.change_in_place(list, &arg.value, true)
    }

    /// `if c: ... elif d: ... else: ... ;` (§5.2). Each branch starts from the flow
    /// where its condition is tested; the paths meet after the `;`.
    fn if_stmt(&mut self, branches: &[ast::Branch], otherwise: Option<&ast::Block>) -> Option<ir::Stmt> {
        let mut ends = Vec::new();
        let mut checked = Vec::new();
        for branch in branches {
            let cond = self.condition(&branch.cond, "if");
            let facts = cond.as_ref().map(|cond| self.facts(cond)).unwrap_or_default();
            let before = self.ctx.flow.clone();
            // The branch runs where the condition holds, the next one where it failed (§7.4).
            self.apply(&facts.when_true);
            let body = self.block(&branch.body);
            ends.push(std::mem::replace(&mut self.ctx.flow, before));
            self.apply(&facts.when_false);
            checked.push((cond, body));
        }
        // Without `else`, the flow where every condition is false goes on.
        let mut result = otherwise.map(|block| self.block(block)).unwrap_or_default();
        self.ctx.flow = ends.into_iter().fold(self.ctx.flow.clone(), Flow::join);
        for (cond, then) in checked.into_iter().rev() {
            result = vec![ir::Stmt::If { cond: cond?, then, otherwise: result }];
        }
        result.pop()
    }

    /// `while c: ... ;` (§10.2).
    fn while_stmt(&mut self, cond: &ast::Expr, body: &ast::Block) -> Option<ir::Stmt> {
        // At the start of a turn, a local assigned in the body may hold a value from an
        // earlier turn.
        self.enter_loop(&body.stmts);
        let cond_ir = self.condition(cond, "while");
        let facts = cond_ir.as_ref().map(|cond| self.facts(cond)).unwrap_or_default();
        let head = self.ctx.flow.clone();
        self.ctx.loops.push(LoopExits::default());
        self.apply(&facts.when_true);
        let body_ir = self.block(body);
        let exits = self.ctx.loops.pop().expect("the loop is open");
        // `while true` only ends through `break`: no path leaves it when the condition is false.
        let after = if matches!(cond.kind, ast::ExprKind::Bool(true)) {
            Flow::unreachable()
        } else {
            let mut after = head;
            std::mem::swap(&mut self.ctx.flow, &mut after);
            self.apply(&facts.when_false);
            std::mem::replace(&mut self.ctx.flow, after)
        };
        self.ctx.flow = exits.breaks.into_iter().fold(after, Flow::join);
        Some(ir::Stmt::While { cond: cond_ir?, body: body_ir })
    }

    /// `for x in values: ... ;` (§10.2). The loop variable is a constant of the body,
    /// which may run zero times.
    /// `for x in values: ... ;` (§10.2); with `parallel`, the turns run in parallel and
    /// change nothing outside the body (§19.2).
    fn for_stmt(
        &mut self,
        var: &ast::Ident,
        iterable: &ast::Expr,
        body: &ast::Block,
        parallel: Option<Span>,
    ) -> Option<ir::Stmt> {
        // `for d in Days` goes through the values of an enumeration (§13.1).
        let iterable = match &iterable.kind {
            ast::ExprKind::TypeName(name) if self.tables.named_types.contains_key(name) => {
                self.enum_values(name, iterable.span).or_else(|| {
                    self.diagnostics.push(
                        Diagnostic::error(format!("`for` cannot go through the type `{name}`"))
                            .with_primary(iterable.span, "")
                            .with_note(
                                "`for` goes through an interval, a list or an enumeration (§10.2, §13.1)",
                            ),
                    );
                    None
                })
            }
            _ => self.expr(iterable),
        };
        let element = match &iterable {
            Some(iterable) => match iterable.ty.element() {
                Some(element) => Some(element),
                None => {
                    self.diagnostics.push(
                        Diagnostic::error(format!("`for` cannot go through {}", article(iterable.ty)))
                            .with_primary(iterable.span, "")
                            .with_note("`for` goes through an interval or a list (§10.2)"),
                    );
                    None
                }
            },
            None => None,
        };
        self.enter_loop(&body.stmts);
        let head = self.ctx.flow.clone();
        self.ctx.loops.push(LoopExits::default());
        self.ctx.scopes.push(Scope::default());
        let turn = |checker: &mut Self| {
            let var = checker.declare(var, element, false, true);
            checker.ctx.locals[var.index()].loop_variable = true;
            (var, checker.stmts(&body.stmts))
        };
        let (var, body) = match parallel {
            Some(span) => self.in_parallel(span, turn),
            None => turn(self),
        };
        self.close_scope();
        let exits = self.ctx.loops.pop().expect("the loop is open");
        self.ctx.flow = exits.breaks.into_iter().fold(head, Flow::join);
        Some(ir::Stmt::For { var, iterable: iterable?, body })
    }

    /// At the start of a turn, a local assigned in the body may hold a value from an
    /// earlier turn, of any of its types.
    fn enter_loop(&mut self, body: &[ast::Stmt]) {
        for local in self.assigned_in(body) {
            if self.ctx.flow.is_reachable() && self.ctx.flow.get(local) == Assigned::No {
                self.ctx.flow.set(local, Assigned::Maybe);
            }
            self.ctx.flow.narrow(local, None);
        }
    }

    /// `break` or `continue`, in the innermost loop (§10.2).
    fn jump(&mut self, span: Span, is_break: bool) -> Option<ir::Stmt> {
        let keyword = if is_break { "break" } else { "continue" };
        let Some(exits) = self.ctx.loops.last_mut() else {
            self.diagnostics.push(
                Diagnostic::error(format!("`{keyword}` outside a loop"))
                    .with_primary(span, "")
                    .with_note(format!("`{keyword}` is used inside a `while` or `for` loop (§10.2)")),
            );
            return None;
        };
        // After `continue`, the flow goes back to the start of the loop, whose state
        // already accounts for every assignment of the body.
        let flow = std::mem::replace(&mut self.ctx.flow, Flow::unreachable());
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
        self.ctx.flow = Flow::unreachable();
        Some(ir::Stmt::Return(None))
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
                ast::StmtKind::While { body, .. } | ast::StmtKind::For { body, .. } => {
                    found.extend(self.assigned_in(&body.stmts))
                }
                ast::StmtKind::Match { cases, .. } => {
                    for (_, body) in cases {
                        found.extend(self.assigned_in(&body.stmts));
                    }
                }
                _ => {}
            }
        }
        found
    }
}

fn is_empty_list(expr: &ast::Expr) -> bool {
    match &expr.kind {
        ast::ExprKind::List(elements) => elements.is_empty(),
        ast::ExprKind::Paren(inner) => is_empty_list(inner),
        _ => false,
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
