//! Domains (spec §16.5, §16.6): the values of a type that have a property, as
//! `{x in Int, x > 0}`. A Domain is only tested for membership, so its value is the
//! function that tells whether a value belongs to it (C74). A generator over `Int`
//! whose conditions bound it on both sides makes a Set instead: the values between the
//! bounds, filtered by the other conditions.

use std::collections::HashSet;

use lion_diagnostics::{Diagnostic, Span};
use lion_ir::{self as ir, Type};
use lion_syntax::ast;

use crate::{Checker, Scope, article, typed};

/// A condition of a comprehension that bounds the variable of a generator over `Int`:
/// one side of a comparison is the variable, the other does not depend on it (§16.5).
#[derive(Clone, Copy)]
pub(crate) struct Bound<'e> {
    /// The element of the comprehension that holds the comparison.
    pub(crate) element: usize,
    /// The expression the variable is compared with.
    pub(crate) expr: &'e ast::Expr,
}

/// The two bounds of a generator over `Int`, the first ones written.
#[derive(Clone, Copy)]
pub(crate) struct Bounds<'e> {
    pub(crate) lower: Bound<'e>,
    pub(crate) upper: Bound<'e>,
}

/// The locals that hold the bounds, once evaluated.
#[derive(Clone, Copy)]
pub(crate) struct BoundLocals<'e> {
    pub(crate) bounds: Bounds<'e>,
    pub(crate) lower: ir::LocalId,
    pub(crate) upper: ir::LocalId,
}

/// `x in T` written as a generator, with `T` a type that is not an enumeration.
pub(crate) fn type_generator(element: &ast::Expr) -> Option<(&str, &ast::TypeExpr)> {
    let ast::ExprKind::TypeTest { value, ty } = &element.kind else { return None };
    let ast::ExprKind::Name(name) = &value.kind else { return None };
    Some((name, ty))
}

/// Whether the type written is `Int`.
fn is_int(ty: &ast::TypeExpr) -> bool {
    matches!(&ty.kind, ast::TypeExprKind::Named { module, name, args }
        if module.is_empty() && args.is_empty() && name.name == "Int")
}

/// The bounds of the variable `var` of the generator at `generator`, found among the
/// conditions after it. A bound does not depend on `var` nor on the generators after it.
pub(crate) fn int_bounds<'e>(
    elements: &'e [ast::Expr],
    generators: &[bool],
    generator: usize,
    var: &str,
    ty: &ast::TypeExpr,
) -> Option<Bounds<'e>> {
    if !is_int(ty) {
        return None;
    }
    let mut excluded: HashSet<String> = HashSet::from([var.to_string()]);
    for (element, _) in elements.iter().zip(generators).skip(generator).filter(|(_, is)| **is) {
        if let ast::ExprKind::TypeTest { value, .. } | ast::ExprKind::Binary { lhs: value, .. } =
            &element.kind
            && let ast::ExprKind::Name(name) = &value.kind
        {
            excluded.insert(name.clone());
        }
    }
    let independent = |expr: &ast::Expr| {
        let mut names = Vec::new();
        crate::closures::names_in_expr(expr, &mut names);
        names.iter().all(|(name, _)| !excluded.contains(name))
    };
    let is_var = |expr: &ast::Expr| matches!(&expr.kind, ast::ExprKind::Name(name) if name == var);
    let (mut lower, mut upper) = (None, None);
    for (index, element) in elements.iter().enumerate().skip(generator + 1) {
        if generators[index] {
            continue;
        }
        let ast::ExprKind::Compare { first, rest } = &element.kind else { continue };
        let operands: Vec<&ast::Expr> =
            std::iter::once(&**first).chain(rest.iter().map(|c| &c.rhs)).collect();
        for (link, comparison) in rest.iter().enumerate() {
            let (left, right) = (operands[link], operands[link + 1]);
            // `x < e` bounds from above; `e < x` from below.
            let (bound, below) = match comparison.op {
                ast::CompareOp::Lt | ast::CompareOp::Le if is_var(left) => (right, false),
                ast::CompareOp::Lt | ast::CompareOp::Le if is_var(right) => (left, true),
                ast::CompareOp::Gt | ast::CompareOp::Ge if is_var(left) => (right, true),
                ast::CompareOp::Gt | ast::CompareOp::Ge if is_var(right) => (left, false),
                _ => continue,
            };
            if is_var(bound) || !independent(bound) {
                continue;
            }
            let found = Bound { element: index, expr: bound };
            if below {
                lower.get_or_insert(found)
            } else {
                upper.get_or_insert(found)
            };
        }
    }
    Some(Bounds { lower: lower?, upper: upper? })
}

impl Checker<'_> {
    /// The iterable of a bounded generator over `Int`: the values between the bounds,
    /// each evaluated once (§16.5).
    pub(crate) fn bounded_range<'e>(
        &mut self,
        bounds: Bounds<'e>,
        span: Span,
    ) -> Option<(ir::Expr, BoundLocals<'e>)> {
        let lower = self.expr(bounds.lower.expr);
        let upper = self.expr(bounds.upper.expr);
        let (lower, upper) = (lower?, upper?);
        let mut valid = true;
        for bound in [&lower, &upper] {
            if bound.ty != Type::Int {
                self.diagnostics.push(
                    Diagnostic::error("the bounds of a generator over `Int` are Int values")
                        .with_primary(bound.span, format!("this is {}", article(bound.ty)))
                        .with_note("the conditions bound the variable on both sides, so the values between the bounds make a Set (§16.5, C74)"),
                );
                valid = false;
            }
        }
        if !valid {
            return None;
        }
        let (low, high) = (self.temporary(Type::Int, lower.span), self.temporary(Type::Int, upper.span));
        let read = |local: ir::LocalId| Box::new(typed(ir::ExprKind::Local(local), Type::Int, span));
        let range = typed(ir::ExprKind::Range { start: read(low), end: read(high) }, Type::Range, span);
        let inner = typed(
            ir::ExprKind::Let { local: high, value: Box::new(upper), body: Box::new(range) },
            Type::Range,
            span,
        );
        let iterable = typed(
            ir::ExprKind::Let { local: low, value: Box::new(lower), body: Box::new(inner) },
            Type::Range,
            span,
        );
        Some((iterable, BoundLocals { bounds, lower: low, upper: high }))
    }

    /// A condition that holds a bound: the bound is read from its local, not evaluated
    /// again.
    pub(crate) fn bound_condition(&mut self, element: &ast::Expr, locals: &BoundLocals) -> Option<ir::Expr> {
        let ast::ExprKind::Compare { first, rest } = &element.kind else {
            unreachable!("a bound is a comparison")
        };
        let operands: Vec<&ast::Expr> =
            std::iter::once(&**first).chain(rest.iter().map(|c| &c.rhs)).collect();
        let mut checked = Vec::new();
        for operand in operands {
            let local = if std::ptr::eq(operand, locals.bounds.lower.expr) {
                Some(locals.lower)
            } else if std::ptr::eq(operand, locals.bounds.upper.expr) {
                Some(locals.upper)
            } else {
                None
            };
            checked.push(match local {
                Some(local) => Some(typed(ir::ExprKind::Local(local), Type::Int, operand.span)),
                None => self.expr(operand),
            });
        }
        let checked: Vec<ir::Expr> = checked.into_iter().collect::<Option<_>>()?;
        let mut result: Option<ir::Expr> = None;
        for (link, comparison) in rest.iter().enumerate() {
            let test = self.compare_values(
                comparison.op,
                comparison.op_span,
                checked[link].clone(),
                checked[link + 1].clone(),
                element.span,
            )?;
            result = Some(match result {
                None => test,
                Some(before) => typed(
                    ir::ExprKind::And { lhs: Box::new(before), rhs: Box::new(test) },
                    Type::Bool,
                    element.span,
                ),
            });
        }
        result
    }

    /// `{x in T, conditions}` without bounds: the Domain of the values of `T` that
    /// satisfy the conditions (§16.5).
    pub(crate) fn domain_comprehension(
        &mut self,
        var: &ast::Ident,
        ty: &ast::TypeExpr,
        conditions: &[ast::Expr],
        span: Span,
    ) -> Option<ir::Expr> {
        let element = self.resolve_type(ty)?;
        let body = conditions.iter().cloned().reduce(|all, condition| ast::Expr {
            span: all.span.to(condition.span),
            kind: ast::ExprKind::Binary {
                op: ast::BinaryOp::And,
                op_span: condition.span,
                lhs: Box::new(all),
                rhs: Box::new(condition),
            },
        });
        let body = body.unwrap_or(ast::Expr { kind: ast::ExprKind::Bool(true), span });
        self.domain_of_property(var.clone(), element, body, span)
    }

    /// The Domain of the values of `element` for which `body`, reading `var`, holds. It
    /// changes nothing: it may be tested anywhere, also in parallel (C74).
    fn domain_of_property(
        &mut self,
        var: ast::Ident,
        element: Type,
        body: ast::Expr,
        span: Span,
    ) -> Option<ir::Expr> {
        let at = |name: &str| ast::Ident { name: name.to_string(), span };
        let decl = ast::FunDecl {
            private: None,
            foreign: None,
            infix: false,
            receiver: None,
            name: at("fun"),
            params: vec![ast::Param { var: None, name: var, ty: None, default: None }],
            ret: None,
            type_params: Vec::new(),
            modifies: Vec::new(),
            body: ast::FunBody::Expr(body),
        };
        let data = ir::FunData { params: vec![element], required: 1, ret: Type::Bool };
        let closure = self.closure(&decl, span, Some(data))?;
        if let ir::ExprKind::Closure { function, .. } = &closure.kind {
            self.check_like_parallel(function.0 as usize, span);
        }
        Some(ir::Expr { ty: Type::domain(element), ..closure })
    }

    /// All the values of a type, written as the operand of `union`, `inter`, `minus` or
    /// `subset`: `Int minus positives` (§16.6).
    pub(crate) fn universe(&mut self, name: &str, span: Span) -> Option<ir::Expr> {
        let ty = ast::TypeExpr {
            kind: ast::TypeExprKind::Named {
                module: Vec::new(),
                name: ast::Ident { name: name.to_string(), span },
                args: Vec::new(),
            },
            span,
        };
        let element = self.resolve_type(&ty)?;
        let var = ast::Ident { name: "·x".to_string(), span };
        self.domain_of_property(var, element, ast::Expr { kind: ast::ExprKind::Bool(true), span }, span)
    }

    /// `v in d`, with `d` a Domain: the function of the Domain, applied to the value.
    pub(crate) fn domain_membership(
        &mut self,
        value: ir::Expr,
        domain: ir::Expr,
        span: Span,
    ) -> Option<ir::Expr> {
        let Type::Domain(element) = domain.ty else { unreachable!("a Domain") };
        let element = element.get();
        let context = (domain.span, format!("this is {}", article(domain.ty)));
        let value = self.coerce(value, element, Some(context))?;
        Some(domain_call(domain, value, span))
    }

    /// `union`, `inter`, `minus` or `subset` with a Domain (§16.6).
    pub(crate) fn domain_operation(
        &mut self,
        op: ast::BinaryOp,
        op_span: Span,
        lhs: ir::Expr,
        rhs: ir::Expr,
        span: Span,
    ) -> Option<ir::Expr> {
        let word = op.as_str();
        let element_of = |ty: Type| match ty {
            Type::Set(element) | Type::Domain(element) => Some(element.get()),
            _ => None,
        };
        let element = match (element_of(lhs.ty), element_of(rhs.ty)) {
            (Some(a), Some(b)) if a == b => a,
            _ => {
                self.diagnostics.push(
                    Diagnostic::error(format!(
                        "`{word}` needs Sets or Domains of the same type, not {} and {}",
                        article(lhs.ty),
                        article(rhs.ty)
                    ))
                    .with_primary(op_span, "")
                    .with_secondary(lhs.span, format!("this is {}", article(lhs.ty)))
                    .with_secondary(rhs.span, format!("this is {}", article(rhs.ty))),
                );
                return None;
            }
        };
        let set = |ty: Type| matches!(ty, Type::Set(_));
        match op {
            ast::BinaryOp::Subset if !set(lhs.ty) => {
                self.diagnostics.push(
                    Diagnostic::error("whether a Domain is part of another set cannot be computed")
                        .with_primary(lhs.span, format!("this is {}", article(lhs.ty)))
                        .with_note(
                            "a Domain is known by a property: only its membership can be tested (§16.5, C74)",
                        ),
                );
                None
            }
            ast::BinaryOp::Subset => Some(self.set_filter(lhs, rhs, Filter::All, span)),
            // A filter of the Set (§16.6).
            ast::BinaryOp::Inter if set(lhs.ty) => Some(self.set_filter(lhs, rhs, Filter::Keep, span)),
            ast::BinaryOp::Inter if set(rhs.ty) => Some(self.set_filter(rhs, lhs, Filter::KeepAfter, span)),
            ast::BinaryOp::Minus if set(lhs.ty) => Some(self.set_filter(lhs, rhs, Filter::Remove, span)),
            _ => self.combined_domain(op, lhs, rhs, element, span),
        }
    }

    /// The Domain of a union, an intersection or a difference, whose operands are
    /// evaluated once, now: `{x in T, x in a or x in b}`.
    fn combined_domain(
        &mut self,
        op: ast::BinaryOp,
        lhs: ir::Expr,
        rhs: ir::Expr,
        element: Type,
        span: Span,
    ) -> Option<ir::Expr> {
        self.ctx.scopes.push(Scope::default());
        let at = |name: &str| ast::Ident { name: name.to_string(), span };
        let (a, b) = (at("·a"), at("·b"));
        let first = self.declare(&a, Some(lhs.ty), false, true);
        let second = self.declare(&b, Some(rhs.ty), false, true);
        let expr = |kind: ast::ExprKind| ast::Expr { kind, span };
        let name = |ident: &ast::Ident| expr(ast::ExprKind::Name(ident.name.clone()));
        let binary = |op, lhs, rhs| {
            expr(ast::ExprKind::Binary { op, op_span: span, lhs: Box::new(lhs), rhs: Box::new(rhs) })
        };
        let x = at("·x");
        let in_a = binary(ast::BinaryOp::In, name(&x), name(&a));
        let in_b = binary(ast::BinaryOp::In, name(&x), name(&b));
        let body = match op {
            ast::BinaryOp::Union => binary(ast::BinaryOp::Or, in_a, in_b),
            ast::BinaryOp::Inter => binary(ast::BinaryOp::And, in_a, in_b),
            _ => {
                let not_b = expr(ast::ExprKind::Unary { op: ast::UnaryOp::Not, operand: Box::new(in_b) });
                binary(ast::BinaryOp::And, in_a, not_b)
            }
        };
        let domain = self.domain_of_property(x, element, body, span);
        self.ctx.scopes.pop();
        let domain = domain?;
        let ty = domain.ty;
        let stmts = vec![
            ir::Stmt::Assign { place: ir::Place::Local(first), value: lhs },
            ir::Stmt::Assign { place: ir::Place::Local(second), value: rhs },
        ];
        Some(typed(ir::ExprKind::Block { stmts, value: Box::new(domain) }, ty, span))
    }

    /// Goes through the Set `set`, testing each element in `domain`.
    fn set_filter(&mut self, set: ir::Expr, domain: ir::Expr, filter: Filter, span: Span) -> ir::Expr {
        let (set_type, domain_type) = (set.ty, domain.ty);
        let element = set_type.element().expect("a Set has elements");
        let set_local = self.temporary(set_type, set.span);
        let domain_local = self.temporary(domain_type, domain.span);
        let var = self.temporary(element, span);
        let result_type = if filter == Filter::All { Type::Bool } else { set_type };
        let result = self.temporary(result_type, span);
        let read = |local: ir::LocalId, ty: Type| typed(ir::ExprKind::Local(local), ty, span);
        let test = domain_call(read(domain_local, domain_type), read(var, element), span);
        let not = |test: ir::Expr| {
            typed(ir::ExprKind::Unary { op: ir::UnaryOp::Not, operand: Box::new(test) }, Type::Bool, span)
        };
        let (start, body) = match filter {
            Filter::All => (
                typed(ir::ExprKind::Bool(true), Type::Bool, span),
                ir::Stmt::If {
                    cond: not(test),
                    then: vec![
                        ir::Stmt::Assign {
                            place: ir::Place::Local(result),
                            value: typed(ir::ExprKind::Bool(false), Type::Bool, span),
                        },
                        ir::Stmt::Break,
                    ],
                    otherwise: Vec::new(),
                },
            ),
            Filter::Keep | Filter::KeepAfter | Filter::Remove => {
                let cond = if filter == Filter::Remove { not(test) } else { test };
                let add = ir::Stmt::Add {
                    root: ir::Place::Local(result),
                    path: Vec::new(),
                    value: read(var, element),
                };
                (
                    typed(ir::ExprKind::Set(Vec::new()), set_type, span),
                    ir::Stmt::If { cond, then: vec![add], otherwise: Vec::new() },
                )
            }
        };
        // The operands are evaluated from left to right (§9.2).
        let (first, second) = match filter {
            Filter::KeepAfter => (
                ir::Stmt::Assign { place: ir::Place::Local(domain_local), value: domain },
                ir::Stmt::Assign { place: ir::Place::Local(set_local), value: set },
            ),
            _ => (
                ir::Stmt::Assign { place: ir::Place::Local(set_local), value: set },
                ir::Stmt::Assign { place: ir::Place::Local(domain_local), value: domain },
            ),
        };
        let stmts = vec![
            first,
            second,
            ir::Stmt::Assign { place: ir::Place::Local(result), value: start },
            ir::Stmt::For { var, iterable: read(set_local, set_type), body: vec![body] },
        ];
        typed(ir::ExprKind::Block { stmts, value: Box::new(read(result, result_type)) }, result_type, span)
    }

    /// An error for a value that may hold a Domain, which cannot be shown.
    pub(crate) fn check_not_domain(&mut self, value: &ir::Expr) -> bool {
        if !holds_domain(value.ty) {
            return true;
        }
        self.diagnostics.push(
            Diagnostic::error(format!("{} cannot be shown", crate::capitalize(&article(value.ty))))
                .with_primary(value.span, "")
                .with_note("a Domain is known by a property: only its membership can be tested (§16.5, C74)"),
        );
        false
    }
}

/// What `set_filter` computes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    /// Whether every element is in the Domain: `subset`.
    All,
    /// The elements in the Domain: `set inter domain`.
    Keep,
    /// The same, written `domain inter set`: the Domain is evaluated first.
    KeepAfter,
    /// The elements not in the Domain: `set minus domain`.
    Remove,
}

/// The call of the function of a Domain.
fn domain_call(domain: ir::Expr, value: ir::Expr, span: Span) -> ir::Expr {
    let Type::Domain(element) = domain.ty else { unreachable!("a Domain") };
    let function = Type::function(vec![element.get()], 1, Type::Bool);
    let callee = Box::new(ir::Expr { ty: function, ..domain });
    typed(ir::ExprKind::CallValue { callee, args: vec![value] }, Type::Bool, span)
}

/// Whether a value of this type may be or hold a Domain.
pub(crate) fn holds_domain(ty: Type) -> bool {
    ty.members().into_iter().any(|member| match member {
        Type::Domain(_) => true,
        Type::List(element) | Type::Set(element) | Type::Task(element) => holds_domain(element.get()),
        Type::Tuple(elements) => elements.elements().into_iter().any(holds_domain),
        _ => false,
    })
}
