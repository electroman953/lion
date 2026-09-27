//! `match` (spec §10.3): the cases are tried in order and the first that matches wins.
//! The value is evaluated once; a case `in T g` binds `g` to the value with the
//! narrower type `T`. Every possible value must have a case: the types left after the
//! cases without conditions must be none, or `otherwise` must end the list.

use std::collections::HashMap;

use lion_diagnostics::{Diagnostic, Severity, Span};
use lion_ir::{self as ir, Type};
use lion_syntax::ast;

use crate::flow::Flow;
use crate::{Checker, Scope, article, typed};

/// The value being matched.
struct Subject {
    /// A temporary that holds the value.
    temp: ir::LocalId,
    ty: Type,
    /// The variable matched, if the value is one: it is narrowed in each case too.
    variable: Option<ir::LocalId>,
    span: Span,
}

/// A checked case, before its body or its value.
struct CheckedCase {
    /// `None` when the case matches every value that reaches it.
    test: Option<ir::Expr>,
    /// The name bound to the value, and the narrowed value.
    binding: Option<(ir::LocalId, ir::Expr)>,
    /// The type of the value inside the case, for the matched variable.
    narrowed: Option<Type>,
}

/// What the cases seen so far cover.
struct Coverage {
    /// The types not covered yet; `None` once every value is covered.
    remaining: Option<Type>,
    /// For a Bool value: which of `true` and `false` are covered.
    booleans: [bool; 2],
    /// For each enumeration among the types: which of its values are covered.
    values: HashMap<ir::EnumRef, Vec<bool>>,
}

impl Coverage {
    fn new(ty: Type) -> Coverage {
        let values = ty
            .members()
            .into_iter()
            .filter_map(|member| match member {
                Type::Enum(enumeration) => Some((enumeration, vec![false; enumeration.values().len()])),
                _ => None,
            })
            .collect();
        Coverage { remaining: Some(ty), booleans: [false; 2], values }
    }
}

impl Checker<'_> {
    /// `match` as a statement: one block per case.
    pub(crate) fn match_stmt(
        &mut self,
        scrutinee: &ast::Expr,
        cases: &[(ast::Case, ast::Block)],
        span: Span,
    ) -> Option<ir::Stmt> {
        let (subject, value) = self.subject(scrutinee)?;
        let mut coverage = Coverage::new(subject.ty);
        let before = self.ctx.flow.clone();
        let mut ends = Vec::new();
        let mut checked = Vec::new();
        let mut valid = true;
        for (case, body) in cases {
            self.warn_if_unreachable(&coverage, case);
            self.ctx.scopes.push(Scope::default());
            let result = self.case(&subject, case, &mut coverage);
            if let Some(result) = &result
                && let (Some(variable), Some(narrowed)) = (subject.variable, result.narrowed)
            {
                self.ctx.flow.narrow(variable, Some(narrowed));
            }
            let stmts = self.stmts(&body.stmts);
            self.close_scope();
            ends.push(std::mem::replace(&mut self.ctx.flow, before.clone()));
            match result {
                Some(result) => checked.push((result, stmts)),
                None => valid = false,
            }
        }
        let exhaustive = self.check_exhaustive(&subject, &coverage, cases.iter().map(|(case, _)| case), span);
        // Without a case for some values, those go on after the `match`.
        let mut flow = if exhaustive { Flow::unreachable() } else { before };
        for end in ends {
            flow = flow.join(end);
        }
        self.ctx.flow = flow;
        if !valid || !exhaustive {
            return None;
        }
        let mut chain = Vec::new();
        for (case, body) in checked.into_iter().rev() {
            let mut stmts = Vec::new();
            if let Some((local, value)) = case.binding {
                stmts.push(ir::Stmt::Assign { place: ir::Place::Local(local), value });
            }
            stmts.extend(body);
            chain = match case.test {
                None => stmts,
                Some(cond) => vec![ir::Stmt::If { cond, then: stmts, otherwise: chain }],
            };
        }
        let mut stmts = vec![ir::Stmt::Assign { place: ir::Place::Local(subject.temp), value }];
        stmts.extend(chain);
        Some(ir::Stmt::Seq(stmts))
    }

    /// `match` as an expression: one value per case (D51).
    pub(crate) fn match_expr(
        &mut self,
        scrutinee: &ast::Expr,
        cases: &[(ast::Case, ast::Expr)],
        span: Span,
    ) -> Option<ir::Expr> {
        let (subject, value) = self.subject(scrutinee)?;
        let mut coverage = Coverage::new(subject.ty);
        let before = self.ctx.flow.clone();
        let mut checked = Vec::new();
        let mut valid = true;
        for (case, result) in cases {
            self.warn_if_unreachable(&coverage, case);
            self.ctx.scopes.push(Scope::default());
            let checked_case = self.case(&subject, case, &mut coverage);
            if let Some(checked_case) = &checked_case
                && let (Some(variable), Some(narrowed)) = (subject.variable, checked_case.narrowed)
            {
                self.ctx.flow.narrow(variable, Some(narrowed));
            }
            let result = self.expr(result);
            self.ctx.scopes.pop();
            self.ctx.flow = before.clone();
            match (checked_case, result) {
                (Some(case), Some(result)) => checked.push((case, result)),
                _ => valid = false,
            }
        }
        let exhaustive = self.check_exhaustive(&subject, &coverage, cases.iter().map(|(case, _)| case), span);
        if !valid || !exhaustive || checked.is_empty() {
            return None;
        }
        let values: Vec<ir::Expr> = checked.iter().map(|(_, value)| value.clone()).collect();
        let ty = self.common_type(&values, span, "the values of a `match`")?;
        // Each value, with its binding; the last case needs no test.
        let mut cases = checked.into_iter().rev();
        let (last, last_value) = cases.next().expect("one case");
        let mut result = bind(last.binding, crate::collections::widen(last_value, ty), ty);
        for (case, value) in cases {
            let then = bind(case.binding, crate::collections::widen(value, ty), ty);
            result = match case.test {
                None => then,
                Some(cond) => {
                    let kind = ir::ExprKind::If {
                        cond: Box::new(cond),
                        then: Box::new(then),
                        otherwise: Box::new(result),
                    };
                    typed(kind, ty, span)
                }
            };
        }
        let kind = ir::ExprKind::Let { local: subject.temp, value: Box::new(value), body: Box::new(result) };
        Some(typed(kind, ty, span))
    }

    fn subject(&mut self, scrutinee: &ast::Expr) -> Option<(Subject, ir::Expr)> {
        let value = self.expr(scrutinee)?;
        let variable = match value.kind {
            ir::ExprKind::Local(local) if !self.ctx.locals[local.index()].temporary => Some(local),
            _ => None,
        };
        let temp = self.temporary(value.ty, value.span);
        Some((Subject { temp, ty: value.ty, variable, span: value.span }, value))
    }

    /// The test and the binding of one case; updates what is covered.
    fn case(&mut self, subject: &Subject, case: &ast::Case, coverage: &mut Coverage) -> Option<CheckedCase> {
        let checked = self.case_parts(subject, case, coverage);
        // After an error, the name still exists, so that the body reports nothing more.
        if checked.is_none()
            && let ast::Pattern::Type { binding: Some(name), .. }
            | ast::Pattern::In { binding: Some(name), .. } = &case.pattern
            && self.lookup(&name.name).is_none()
        {
            self.declare(name, None, false, true);
        }
        checked
    }

    fn case_parts(
        &mut self,
        subject: &Subject,
        case: &ast::Case,
        coverage: &mut Coverage,
    ) -> Option<CheckedCase> {
        let current = coverage.remaining.unwrap_or(subject.ty);
        let read = |ty: Type| typed(ir::ExprKind::Local(subject.temp), ty, subject.span);
        let unconditional = case.conditions.is_empty();
        let (pattern_test, binding_name, narrowed) = match &case.pattern {
            ast::Pattern::Otherwise => {
                if unconditional {
                    coverage.remaining = None;
                    coverage.booleans = [true; 2];
                }
                (None, None, None)
            }
            ast::Pattern::Value(value) => {
                let value = self.expr_expecting(value, subject.ty)?;
                if unconditional {
                    self.cover_value(coverage, &value);
                }
                (Some(self.equality_test(read(subject.ty), value, case.span)?), None, None)
            }
            ast::Pattern::Type { ty, binding } => {
                let target = self.resolve_type(ty)?;
                let Some(tested) = subject.ty.intersection(target) else {
                    self.diagnostics.push(
                        Diagnostic::error(format!(
                            "this case never matches: the value is {}",
                            article(subject.ty)
                        ))
                        .with_primary(ty.span, "")
                        .with_note(format!(
                            "{} is never {}",
                            article(subject.ty),
                            article(target)
                        )),
                    );
                    return None;
                };
                if !self.distinguishable(subject.ty, tested, ty.span) {
                    return None;
                }
                let narrowed = current.intersection(target).unwrap_or(tested);
                if unconditional {
                    coverage.remaining = coverage.remaining.and_then(|remaining| remaining.without(target));
                }
                let test = (!subject.ty.is_subset_of(target)).then(|| {
                    typed(
                        ir::ExprKind::TypeTest { value: Box::new(read(subject.ty)), ty: tested },
                        Type::Bool,
                        case.span,
                    )
                });
                (test, binding.as_ref(), Some(narrowed))
            }
            ast::Pattern::In { set, binding } => {
                let set = self.expr(set)?;
                let test = self.membership_test(read(subject.ty), set, case.span)?;
                (Some(test), binding.as_ref(), None)
            }
        };
        let binding = binding_name.map(|name| {
            let ty = narrowed.unwrap_or(current);
            let local = self.declare(name, Some(ty), false, true);
            (local, read(ty))
        });
        // The conditions see the binding: `in Int g, g >= 10` (§10.3).
        let mut conditions = Vec::new();
        for condition in &case.conditions {
            conditions.push(self.condition(condition, "match")?);
        }
        let conditions = conditions.into_iter().reduce(|lhs, rhs| {
            let span = lhs.span.to(rhs.span);
            typed(ir::ExprKind::And { lhs: Box::new(lhs), rhs: Box::new(rhs) }, Type::Bool, span)
        });
        let conditions = match (conditions, &binding) {
            (Some(conditions), Some((local, value))) => {
                let span = conditions.span;
                let kind = ir::ExprKind::Let {
                    local: *local,
                    value: Box::new(value.clone()),
                    body: Box::new(conditions),
                };
                Some(typed(kind, Type::Bool, span))
            }
            (conditions, _) => conditions,
        };
        let test = match (pattern_test, conditions) {
            (Some(test), Some(conditions)) => {
                let span = test.span.to(conditions.span);
                Some(typed(
                    ir::ExprKind::And { lhs: Box::new(test), rhs: Box::new(conditions) },
                    Type::Bool,
                    span,
                ))
            }
            (test, None) => test,
            (None, conditions) => conditions,
        };
        Some(CheckedCase {
            test,
            binding,
            narrowed: narrowed.or(Some(current)).filter(|ty| *ty != subject.ty),
        })
    }

    /// A value pattern covers `none`, `true` or `false` entirely.
    fn cover_value(&self, coverage: &mut Coverage, value: &ir::Expr) {
        match value.kind {
            ir::ExprKind::None => {
                coverage.remaining = coverage.remaining.and_then(|remaining| remaining.without(Type::None));
            }
            ir::ExprKind::Bool(value) => {
                coverage.booleans[usize::from(value)] = true;
                if coverage.booleans == [true; 2] {
                    coverage.remaining =
                        coverage.remaining.and_then(|remaining| remaining.without(Type::Bool));
                }
            }
            ir::ExprKind::Enum { enumeration, value } => {
                let Some(covered) = coverage.values.get_mut(&enumeration) else { return };
                covered[value as usize] = true;
                if covered.iter().all(|&covered| covered) {
                    let ty = Type::Enum(enumeration);
                    coverage.remaining = coverage.remaining.and_then(|remaining| remaining.without(ty));
                }
            }
            _ => {}
        }
    }

    /// A case after the cases that cover every value can never be reached (D77).
    fn warn_if_unreachable(&mut self, coverage: &Coverage, case: &ast::Case) {
        if coverage.remaining.is_none() {
            self.diagnostics.push(
                Diagnostic::new(Severity::Warning, "this case can never be reached")
                    .with_primary(case.span, "")
                    .with_note("the cases above it already cover every value (§10.3)"),
            );
        }
    }

    /// Every possible value needs a case (§10.3).
    fn check_exhaustive<'c>(
        &mut self,
        subject: &Subject,
        coverage: &Coverage,
        mut cases: impl Iterator<Item = &'c ast::Case>,
        span: Span,
    ) -> bool {
        let Some(remaining) = coverage.remaining else { return true };
        let has_otherwise = cases.any(|case| matches!(case.pattern, ast::Pattern::Otherwise));
        let missing = match remaining {
            Type::Bool if coverage.booleans[1] => "`false`".to_string(),
            Type::Bool if coverage.booleans[0] => "`true`".to_string(),
            Type::None => "`none`".to_string(),
            Type::Enum(enumeration) if coverage.values[&enumeration].contains(&true) => {
                let values = enumeration.values();
                let missing: Vec<String> = coverage.values[&enumeration]
                    .iter()
                    .zip(&values)
                    .filter(|(covered, _)| !**covered)
                    .map(|(_, value)| format!("`{value}`"))
                    .collect();
                missing.join(", ")
            }
            _ if remaining == subject.ty && !matches!(remaining, Type::Union(_)) => {
                format!("some values of {}", article(remaining))
            }
            _ => article(remaining),
        };
        let help = match remaining {
            Type::Enum(_) => "add a case for each value, or `otherwise` at the end".to_string(),
            _ => format!("add a case such as `in {} ...`, or `otherwise` at the end", remaining.members()[0]),
        };
        let mut error = Diagnostic::error(format!("this `match` has no case for {missing}"))
            .with_primary(span, "")
            .with_help(help);
        if has_otherwise {
            error = error.with_note("the case `otherwise` has conditions, so it does not cover every value");
        }
        self.diagnostics.push(error);
        false
    }
}

/// `value` in the scope of the binding of its case.
fn bind(binding: Option<(ir::LocalId, ir::Expr)>, value: ir::Expr, ty: Type) -> ir::Expr {
    match binding {
        None => value,
        Some((local, bound)) => {
            let span = value.span;
            typed(ir::ExprKind::Let { local, value: Box::new(bound), body: Box::new(value) }, ty, span)
        }
    }
}
