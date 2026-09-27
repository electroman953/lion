//! Type checking of expressions (spec §7 to §9).
//!
//! Each function returns `None` when the expression has an error, after reporting
//! it; the callers then stay silent, so one mistake produces one message.

use lion_diagnostics::{Diagnostic, Span};
use lion_ir::{self as ir, Type};
use lion_syntax::ast;

use crate::collections::Collection;
use crate::names::{IMPLEMENTED_FUNCTIONS, PLANNED_FUNCTIONS, Resolved};
use crate::structs::Given;
use crate::{Checker, ContextKind, GlobalType, article, typed};

/// A comparison between two operands, specialised to their types.
#[derive(Clone, Copy)]
struct Comparison {
    op: ir::BinaryOp,
    /// Int operands are converted to Float first.
    on_floats: bool,
    /// Values of an ordered enumeration are compared by their positions (D33).
    on_positions: bool,
}

impl Checker<'_> {
    pub(crate) fn expr(&mut self, expr: &ast::Expr) -> Option<ir::Expr> {
        let span = expr.span;
        match &expr.kind {
            ast::ExprKind::Int(value) => Some(typed(ir::ExprKind::Int(*value), Type::Int, span)),
            ast::ExprKind::Float(value) => Some(typed(ir::ExprKind::Float(*value), Type::Float, span)),
            ast::ExprKind::Bool(value) => Some(typed(ir::ExprKind::Bool(*value), Type::Bool, span)),
            ast::ExprKind::None => Some(typed(ir::ExprKind::None, Type::None, span)),
            ast::ExprKind::Text(parts) => self.text(parts, span),
            ast::ExprKind::Name(name) => self.name(name, span),
            ast::ExprKind::TypeName(_) => {
                self.not_implemented(span, "types used as values", "§7.1, §12.2");
                None
            }
            ast::ExprKind::Paren(inner) => self.expr(inner).map(|inner| ir::Expr { span, ..inner }),
            ast::ExprKind::Unary { op, operand } => self.unary(*op, operand, span),
            ast::ExprKind::Binary { op, op_span, lhs, rhs } => self.binary(*op, *op_span, lhs, rhs, span),
            ast::ExprKind::Compare { first, rest } => self.compare(first, rest, span),
            ast::ExprKind::As { value, ty } => self.convert(value, ty, span),
            ast::ExprKind::Call { callee, args } => self.call(callee, args, span),
            ast::ExprKind::Field { object, name } => match &object.kind {
                ast::ExprKind::TypeName(type_name) => self.enum_member(type_name, object.span, name),
                // `m.x`: a variable of a module (§20.2).
                ast::ExprKind::Name(module) if self.imported_module(module).is_some() => {
                    let module = self.imported_module(module).expect("checked");
                    self.module_global(module, name, span)
                }
                // `m.Color.red`: a value of an enumeration of a module.
                ast::ExprKind::Field { object: inner, name: type_name } if matches!(&inner.kind, ast::ExprKind::Name(module) if self.imported_module(module).is_some()) =>
                {
                    let ast::ExprKind::Name(module) = &inner.kind else { unreachable!("checked") };
                    let module = self.imported_module(module).expect("checked");
                    let previous = self.enter_module(module);
                    let value = self.enum_member(&type_name.name, object.span, name);
                    self.enter_module(previous);
                    value
                }
                _ => self.property(object, name, span),
            },
            ast::ExprKind::Index { object, index } => self.index(object, index, span),
            ast::ExprKind::List(elements) => self.list(elements, span),
            ast::ExprKind::If { branches, otherwise } => {
                self.if_expr(branches, otherwise.as_deref(), span, None)
            }
            ast::ExprKind::TypeTest { value, ty } => self.type_test(value, ty, span),
            ast::ExprKind::Try(value) => self.try_expr(value, span),
            ast::ExprKind::Match { scrutinee, cases } => self.match_expr(scrutinee, cases, span),
            ast::ExprKind::Set(elements) => self.collection(Collection::Set, elements, span),
            ast::ExprKind::Tuple(elements) => self.tuple(elements, span),
            ast::ExprKind::Parallel(inner) => self.parallel_expr(inner, span),
            ast::ExprKind::Fun(decl) => self.anonymous_function(decl, span, None),
        }
    }

    /// The condition of an `if`, `elif` or `while`, which must be a Bool.
    pub(crate) fn condition(&mut self, cond: &ast::Expr, keyword: &str) -> Option<ir::Expr> {
        let cond = self.expr(cond)?;
        if cond.ty == Type::Bool {
            return Some(cond);
        }
        let mut error = Diagnostic::error(format!("the condition of `{keyword}` must be a Bool"))
            .with_primary(cond.span, format!("this is {}", article(cond.ty)));
        if cond.ty.is_numeric() {
            error = error.with_help("compare the number, for instance `x != 0`");
        }
        self.diagnostics.push(error);
        None
    }

    /// `if c then a elif d then b else e` (§10.1). The branches have one type; an Int
    /// branch next to a Float one is converted, as everywhere else (§8.5).
    /// `if c then a elif d then b else e`; each value expects the type the whole
    /// expression is expected to have, if any.
    pub(crate) fn if_expr(
        &mut self,
        branches: &[(ast::Expr, ast::Expr)],
        otherwise: Option<&ast::Expr>,
        span: Span,
        expected: Option<Type>,
    ) -> Option<ir::Expr> {
        let value_of = |checker: &mut Self, value: &ast::Expr| match expected {
            Some(expected) => checker.expr_expecting(value, expected),
            None => checker.expr(value),
        };
        // Each value is checked where its condition holds, and the next condition
        // where the previous ones failed (§7.4).
        let before = self.ctx.flow.clone();
        let mut checked = Vec::new();
        for (cond, value) in branches {
            let cond = self.condition(cond, "if");
            let facts = cond.as_ref().map(|cond| self.facts(cond)).unwrap_or_default();
            let outside = self.ctx.flow.clone();
            self.apply(&facts.when_true);
            let value = value_of(self, value);
            self.ctx.flow = outside;
            self.apply(&facts.when_false);
            checked.push((cond, value));
        }
        // Without `else`, the value is `none` when no condition holds (§10.1).
        let otherwise = match otherwise {
            Some(otherwise) => value_of(self, otherwise),
            None => Some(typed(ir::ExprKind::None, Type::None, span)),
        };
        self.ctx.flow = before;
        let mut valid = otherwise.is_some();
        let mut values = Vec::new();
        let mut conds = Vec::new();
        for (cond, value) in checked {
            valid &= cond.is_some() && value.is_some();
            conds.extend(cond);
            values.extend(value);
        }
        if !valid {
            return None;
        }
        values.push(otherwise.expect("checked above"));
        let ty = self.common_type(&values, span, "the values of an `if`")?;
        let mut values = values.into_iter().map(|value| crate::collections::widen(value, ty));
        let mut result = values.next_back().expect("an `else` value");
        for (cond, then) in conds.into_iter().zip(values).rev() {
            let kind =
                ir::ExprKind::If { cond: Box::new(cond), then: Box::new(then), otherwise: Box::new(result) };
            result = typed(kind, ty, span);
        }
        Some(result)
    }

    /// A text literal; interpolated values are written as `show` would (§4.5, D24).
    fn text(&mut self, parts: &[ast::TextPart], span: Span) -> Option<ir::Expr> {
        match parts {
            [] => return Some(typed(ir::ExprKind::Text(String::new()), Type::Text, span)),
            [ast::TextPart::Literal(text)] => {
                return Some(typed(ir::ExprKind::Text(text.clone()), Type::Text, span));
            }
            _ => {}
        }
        let mut pieces = Vec::new();
        let mut valid = true;
        for part in parts {
            match part {
                ast::TextPart::Literal(text) => {
                    pieces.push(typed(ir::ExprKind::Text(text.clone()), Type::Text, span));
                }
                ast::TextPart::Interpolation(value) => match self.expr(value) {
                    Some(value) if value.ty == Type::Text => pieces.push(value),
                    Some(value) => pieces.push(convert(ir::Conversion::ToText, value, Type::Text)),
                    None => valid = false,
                },
            }
        }
        valid.then(|| typed(ir::ExprKind::Concat(pieces), Type::Text, span))
    }

    fn name(&mut self, name: &str, span: Span) -> Option<ir::Expr> {
        if name == "self" && self.lookup(name).is_none() {
            self.diagnostics.push(
                Diagnostic::error("`self` exists only in methods").with_primary(span, "").with_note(
                    "a method is declared `fun Type.name(...)`, and reads its object as `self` (§12.4)",
                ),
            );
            return None;
        }
        match self.resolve(name, span) {
            Resolved::Local(local) => {
                let ty = self.local_type(local)?;
                self.check_has_value(local, span).then(|| typed(ir::ExprKind::Local(local), ty, span))
            }
            // Whether it has a value is checked at the calls of the script (C3).
            Resolved::Global(local) => {
                self.ctx.reads.push(local);
                let GlobalType::Known(ty) = self.global_info(local).ty else { return None };
                Some(typed(ir::ExprKind::Global(local), ty?, span))
            }
            Resolved::Function(index) => self.function_value(index, span, None),
            Resolved::Standard(_) => {
                self.not_implemented(span, "standard functions used as values", "§11, §23");
                None
            }
            Resolved::Nothing => None,
        }
    }

    fn unary(&mut self, op: ast::UnaryOp, operand: &ast::Expr, span: Span) -> Option<ir::Expr> {
        let operand = self.expr(operand)?;
        let (ir_op, ty) = match (op, operand.ty) {
            (ast::UnaryOp::Neg, Type::Int) => (ir::UnaryOp::NegInt, Type::Int),
            (ast::UnaryOp::Neg, Type::Float) => (ir::UnaryOp::NegFloat, Type::Float),
            (ast::UnaryOp::Not, Type::Bool) => (ir::UnaryOp::Not, Type::Bool),
            (ast::UnaryOp::Neg, found) => {
                self.diagnostics.push(
                    Diagnostic::error(format!("`-` cannot be applied to {}", article(found)))
                        .with_primary(operand.span, format!("this is {}", article(found)))
                        .with_note("`-` negates an Int or a Float"),
                );
                return None;
            }
            (ast::UnaryOp::Not, found) => {
                self.diagnostics.push(
                    Diagnostic::error("`not` expects a Bool")
                        .with_primary(operand.span, format!("this is {}", article(found))),
                );
                return None;
            }
        };
        Some(typed(ir::ExprKind::Unary { op: ir_op, operand: Box::new(operand) }, ty, span))
    }

    fn binary(
        &mut self,
        op: ast::BinaryOp,
        op_span: Span,
        lhs: &ast::Expr,
        rhs: &ast::Expr,
        span: Span,
    ) -> Option<ir::Expr> {
        use ast::BinaryOp::*;
        let unsupported = match op {
            Over => Some(("Rational numbers (`over`)", "§8.3")),
            Same => Some(("`same`", "§9.4, §17.2")),
            _ => None,
        };
        if let Some((what, section)) = unsupported {
            self.not_implemented(op_span, what, section);
            return None;
        }
        if matches!(op, And | Or) {
            // The right side is checked where the left side has decided nothing yet:
            // `x != none and x > 3` knows `x` is not `none` on the right (§7.4).
            let lhs = self.expr(lhs);
            let facts = lhs.as_ref().map(|lhs| self.facts(lhs)).unwrap_or_default();
            let outside = self.ctx.flow.clone();
            self.apply(if op == And { &facts.when_true } else { &facts.when_false });
            let rhs = self.expr(rhs);
            self.ctx.flow = outside;
            return self.logical(op, lhs?, rhs?, span);
        }
        // `blue in colors`: a value alone takes the type of the elements (D32, C54).
        let (lhs, rhs) = if op == In && self.is_bare_unknown_name(lhs) {
            let rhs = self.expr(rhs);
            let lhs = match rhs.as_ref().and_then(|set| set.ty.element()) {
                Some(element) => self.expr_expecting(lhs, element),
                None => self.expr(lhs),
            };
            (lhs, rhs)
        } else {
            (self.expr(lhs), self.expr(rhs))
        };
        let (lhs, rhs) = (lhs?, rhs?);
        match op {
            And | Or => unreachable!("handled above"),
            Range => self.range(lhs, rhs, span),
            In => self.membership(lhs, rhs, op_span, span),
            Union | Inter | Minus | Subset => self.set_operation(op, op_span, lhs, rhs, span),
            _ => self.arithmetic(op, op_span, lhs, rhs, span),
        }
    }

    /// `a..b`: the integers from `a` to `b` included, increasing (§16.3).
    fn range(&mut self, start: ir::Expr, end: ir::Expr, span: Span) -> Option<ir::Expr> {
        let mut valid = true;
        for bound in [&start, &end] {
            if bound.ty != Type::Int {
                self.diagnostics.push(
                    Diagnostic::error("an interval goes from an Int to an Int")
                        .with_primary(bound.span, format!("this is {}", article(bound.ty)))
                        .with_note("`a..b` holds the integers from `a` to `b`; there is no interval of Float (§16.3)"),
                );
                valid = false;
            }
        }
        valid.then(|| {
            typed(ir::ExprKind::Range { start: Box::new(start), end: Box::new(end) }, Type::Range, span)
        })
    }

    /// `x in values`, for the patterns `in set` of `match` (D76).
    pub(crate) fn membership_test(&mut self, value: ir::Expr, set: ir::Expr, span: Span) -> Option<ir::Expr> {
        let set_span = set.span;
        self.membership(value, set, set_span, span)
    }

    /// `x in values` (§16).
    fn membership(&mut self, value: ir::Expr, set: ir::Expr, op_span: Span, span: Span) -> Option<ir::Expr> {
        match set.ty {
            Type::Range if value.ty == Type::Int => {
                let kind = ir::ExprKind::Binary {
                    op: ir::BinaryOp::InRange,
                    lhs: Box::new(value),
                    rhs: Box::new(set),
                };
                Some(typed(kind, Type::Bool, span))
            }
            Type::List(_) | Type::Set(_) => self.list_membership(value, set, span),
            Type::Range => {
                self.diagnostics.push(
                    Diagnostic::error(format!("an interval holds Int values, not {}", article(value.ty)))
                        .with_primary(value.span, format!("this is {}", article(value.ty)))
                        .with_note("Lion 0.1 does not define membership between values of different types"),
                );
                None
            }
            other => {
                self.not_implemented(op_span, &format!("membership tests in {}", article(other)), "§16");
                None
            }
        }
    }

    /// `x in T`: whether the value belongs to the type (§7.1).
    fn type_test(&mut self, value: &ast::Expr, ty: &ast::TypeExpr, span: Span) -> Option<ir::Expr> {
        // `(...) in Student`: whether these values build a valid Student (§12.3).
        if let ast::ExprKind::Tuple(elements) = &value.kind {
            let Type::Struct(structure) = self.resolve_type(ty)? else {
                self.not_implemented(value.span, "tuples as values", "§16");
                return None;
            };
            let index = self.struct_index(structure);
            return self.construction_test(index, &Given::elements(elements), span);
        }
        let value = self.expr(value);
        let target = self.resolve_type(ty);
        let (value, target) = (value?, target?);
        let Some(tested) = value.ty.intersection(target) else {
            self.diagnostics.push(
                Diagnostic::error(format!("this test is always false: the value is {}", article(value.ty)))
                    .with_primary(value.span, "")
                    .with_note(format!("{} is never {}", article(value.ty), article(target))),
            );
            return None;
        };
        if !self.distinguishable(value.ty, tested, span) {
            return None;
        }
        let kind = ir::ExprKind::TypeTest { value: Box::new(value), ty: tested };
        Some(typed(kind, Type::Bool, span))
    }

    /// Whether a value of `whole` can be told to be in `part` while the program runs: a
    /// value carries its kind (Int, List...), but not the type of the elements of a list.
    pub(crate) fn distinguishable(&mut self, whole: Type, part: Type, span: Span) -> bool {
        // The collections whose values do not carry the types of their elements.
        let kind = |ty: &Type| match ty {
            Type::List(_) => Some("list"),
            Type::Set(_) => Some("set"),
            Type::Tuple(_) => Some("tuple"),
            _ => None,
        };
        for name in ["list", "set", "tuple"] {
            let all = whole.members().iter().filter(|member| kind(member) == Some(name)).count();
            let tested = part.members().iter().filter(|member| kind(member) == Some(name)).count();
            if all > 1 && tested > 0 && tested < all {
                self.not_implemented(span, &format!("telling apart several {name} types in a union"), "§7.3");
                return false;
            }
        }
        true
    }

    /// `(a, b)`: a tuple of values (§4.5); names in a tuple build a structure (§12.2).
    fn tuple(&mut self, elements: &[ast::Element], span: Span) -> Option<ir::Expr> {
        if let Some(name) = elements.iter().find_map(|element| element.name.as_ref()) {
            self.diagnostics.push(
                Diagnostic::error("the elements of a tuple have no name")
                    .with_primary(name.span, "")
                    .with_help("names give the fields of a structure: `(...) as Student` (§12.2)"),
            );
            return None;
        }
        let values: Vec<Option<ir::Expr>> =
            elements.iter().map(|element| self.expr(&element.value)).collect();
        let values: Vec<ir::Expr> = values.into_iter().collect::<Option<_>>()?;
        let ty = Type::tuple(values.iter().map(|value| value.ty).collect());
        Some(typed(ir::ExprKind::Tuple(values), ty, span))
    }

    /// `a union b`, `a inter b`, `a minus b`, `a subset b`, on Sets of the same type (§16.6).
    fn set_operation(
        &mut self,
        op: ast::BinaryOp,
        op_span: Span,
        lhs: ir::Expr,
        rhs: ir::Expr,
        span: Span,
    ) -> Option<ir::Expr> {
        let (lhs, rhs) = (self.within_try(lhs), self.within_try(rhs));
        let word = op.as_str();
        if !matches!(lhs.ty, Type::Set(_)) || lhs.ty != rhs.ty {
            let mut error = Diagnostic::error(format!(
                "`{word}` needs two Sets of the same type, not {} and {}",
                article(lhs.ty),
                article(rhs.ty)
            ))
            .with_primary(op_span, "")
            .with_secondary(lhs.span, format!("this is {}", article(lhs.ty)))
            .with_secondary(rhs.span, format!("this is {}", article(rhs.ty)));
            if matches!(lhs.ty, Type::List(_)) || matches!(rhs.ty, Type::List(_)) {
                error = error
                    .with_help("a list keeps order and repetitions; build a Set with `{x, x in l}` (§16.4)");
            }
            self.diagnostics.push(error);
            return None;
        }
        let (ir_op, ty) = match op {
            ast::BinaryOp::Union => (ir::BinaryOp::SetUnion, lhs.ty),
            ast::BinaryOp::Inter => (ir::BinaryOp::SetInter, lhs.ty),
            ast::BinaryOp::Minus => (ir::BinaryOp::SetMinus, lhs.ty),
            _ => (ir::BinaryOp::Subset, Type::Bool),
        };
        let kind = ir::ExprKind::Binary { op: ir_op, lhs: Box::new(lhs), rhs: Box::new(rhs) };
        Some(typed(kind, ty, span))
    }

    /// `try expr` (§18.3): the value without its Error, which leaves the function.
    fn try_expr(&mut self, inner: &ast::Expr, span: Span) -> Option<ir::Expr> {
        if !self.try_allowed(span) {
            return None;
        }
        self.ctx.in_try += 1;
        let value = self.expr(inner);
        self.ctx.in_try -= 1;
        let value = value?;
        if !value.ty.members().contains(&Type::Error) {
            self.diagnostics.push(
                Diagnostic::error(format!(
                    "`try` needs a value that may be an Error; this is {}",
                    article(value.ty)
                ))
                .with_primary(value.span, "")
                .with_note("`try` gives the value, or makes the function return the Error (§18.3)"),
            );
            return None;
        }
        self.unwrap_error(value, span)
    }

    /// `try` is allowed where an Error can leave: in a function whose return type may be
    /// an Error, or in the script, which stops (§18.3, D80).
    fn try_allowed(&mut self, span: Span) -> bool {
        let instance = match self.ctx.kind {
            ContextKind::Script => return true,
            ContextKind::Function(instance) => instance,
            ContextKind::Init(_) => {
                self.diagnostics.push(
                    Diagnostic::error(
                        "`try` has no function to leave in the value of a variable of a module",
                    )
                    .with_primary(span, "")
                    .with_help("handle the error with `if x in Error`, or `match` (§18.3)"),
                );
                return false;
            }
            ContextKind::Structure(_) => {
                self.diagnostics.push(
                    Diagnostic::error("`try` has no function to leave in the conditions of a structure")
                        .with_primary(span, "")
                        .with_help("handle the error with `if x in Error`, or `match` (§18.3)"),
                );
                return false;
            }
        };
        match self.declared_return(instance) {
            Some(ret) if !ret.members().contains(&Type::Error) => {
                self.diagnostics.push(
                    Diagnostic::error(format!(
                        "`try` cannot return an Error from a function that returns {}",
                        article(ret)
                    ))
                    .with_primary(span, "")
                    .with_help(format!(
                        "declare the function `in {ret} or Error`, or handle the error with `if x in Error`"
                    )),
                );
                false
            }
            // An inferred return type gets `Error` from the `try`.
            _ => true,
        }
    }

    /// The value without its Error, which leaves the function or stops the script.
    fn unwrap_error(&mut self, value: ir::Expr, span: Span) -> Option<ir::Expr> {
        let Some(rest) = value.ty.without(Type::Error) else {
            self.diagnostics
                .push(Diagnostic::error("this value is always an Error").with_primary(value.span, ""));
            return None;
        };
        if let ContextKind::Function(_) = self.ctx.kind {
            self.ctx.returns.push((Some(Type::Error), span));
        }
        Some(typed(ir::ExprKind::Try(Box::new(value)), rest, span))
    }

    /// Inside `try`, every step that may give an Error is covered (§18.3, D35).
    pub(crate) fn within_try(&mut self, value: ir::Expr) -> ir::Expr {
        if self.ctx.in_try == 0 || !value.ty.members().contains(&Type::Error) || value.ty == Type::Error {
            return value;
        }
        let span = value.span;
        self.unwrap_error(value.clone(), span).unwrap_or(value)
    }

    /// `and` and `or`, which only evaluate their right side when needed (§9.2).
    fn logical(&mut self, op: ast::BinaryOp, lhs: ir::Expr, rhs: ir::Expr, span: Span) -> Option<ir::Expr> {
        let mut valid = true;
        for operand in [&lhs, &rhs] {
            if operand.ty != Type::Bool {
                self.diagnostics.push(
                    Diagnostic::error(format!("`{}` expects a Bool on each side", op.as_str()))
                        .with_primary(operand.span, format!("this is {}", article(operand.ty))),
                );
                valid = false;
            }
        }
        if !valid {
            return None;
        }
        let (lhs, rhs) = (Box::new(lhs), Box::new(rhs));
        let kind = if op == ast::BinaryOp::And {
            ir::ExprKind::And { lhs, rhs }
        } else {
            ir::ExprKind::Or { lhs, rhs }
        };
        Some(typed(kind, Type::Bool, span))
    }

    /// `+ - * / div mod ^` (§8). Int with Int stays Int, except `/` which always gives
    /// a Float; as soon as a Float is involved, the Int side is converted (§8.5).
    pub(crate) fn arithmetic(
        &mut self,
        op: ast::BinaryOp,
        op_span: Span,
        lhs: ir::Expr,
        rhs: ir::Expr,
        span: Span,
    ) -> Option<ir::Expr> {
        use ast::BinaryOp as Op;
        let lhs = self.within_try(lhs);
        let rhs = self.within_try(rhs);
        if !(lhs.ty.is_numeric() && rhs.ty.is_numeric()) {
            self.arithmetic_type_error(op, op_span, &lhs, &rhs);
            return None;
        }
        let ints = lhs.ty == Type::Int && rhs.ty == Type::Int;
        let (ir_op, ty) = match op {
            Op::Add if ints => (ir::BinaryOp::AddInt, Type::Int),
            Op::Sub if ints => (ir::BinaryOp::SubInt, Type::Int),
            Op::Mul if ints => (ir::BinaryOp::MulInt, Type::Int),
            Op::Pow if ints => (ir::BinaryOp::PowInt, Type::Int),
            Op::IntDiv if ints => (ir::BinaryOp::DivInt, Type::Int),
            Op::Mod if ints => (ir::BinaryOp::ModInt, Type::Int),
            Op::IntDiv | Op::Mod => {
                let float = if lhs.ty == Type::Float { &lhs } else { &rhs };
                self.diagnostics.push(
                    Diagnostic::error(format!("`{}` is defined only for Int values", op.as_str()))
                        .with_primary(float.span, "this is a Float")
                        .with_note("Lion 0.1 defines `div` and `mod` on Int values only (§8.1)")
                        .with_help("use `/` to divide Floats, or convert the value with `as Int`"),
                );
                return None;
            }
            Op::Add => (ir::BinaryOp::AddFloat, Type::Float),
            Op::Sub => (ir::BinaryOp::SubFloat, Type::Float),
            Op::Mul => (ir::BinaryOp::MulFloat, Type::Float),
            Op::Div => (ir::BinaryOp::DivFloat, Type::Float),
            Op::Pow => (ir::BinaryOp::PowFloat, Type::Float),
            _ => unreachable!("`{}` is not an arithmetic operator", op.as_str()),
        };
        let (lhs, rhs) = if ty == Type::Float { (to_float(lhs), to_float(rhs)) } else { (lhs, rhs) };
        Some(typed(ir::ExprKind::Binary { op: ir_op, lhs: Box::new(lhs), rhs: Box::new(rhs) }, ty, span))
    }

    fn arithmetic_type_error(&mut self, op: ast::BinaryOp, op_span: Span, lhs: &ir::Expr, rhs: &ir::Expr) {
        let symbol = op.as_str();
        let mut error =
            Diagnostic::error(format!("`{symbol}` cannot be applied to {} and {}", lhs.ty, rhs.ty))
                .with_primary(op_span, "");
        for operand in [lhs, rhs] {
            if !operand.ty.is_numeric() {
                error = error.with_secondary(operand.span, format!("this is {}", article(operand.ty)));
            }
        }
        error = error.with_note(format!("`{symbol}` is defined for Int and Float values"));
        if op == ast::BinaryOp::Add && (lhs.ty == Type::Text || rhs.ty == Type::Text) {
            error = error.with_help("to join texts, use interpolation: \"{a}{b}\" (§4.5)");
        }
        if let Some(help) = [lhs, rhs].iter().find_map(|operand| union_help(operand.ty)) {
            error = error.with_help(help);
        }
        self.diagnostics.push(error);
    }

    /// One comparison, or a chain such as `0 <= grade <= 20` (§9.3).
    fn compare(&mut self, first: &ast::Expr, rest: &[ast::Comparison], span: Span) -> Option<ir::Expr> {
        // `status == idle`: a value of an enumeration written alone takes the type of
        // the operand beside it (D32, C54).
        let asts: Vec<&ast::Expr> = std::iter::once(first).chain(rest.iter().map(|c| &c.rhs)).collect();
        let bare: Vec<bool> = asts.iter().map(|expr| self.is_bare_unknown_name(expr)).collect();
        let mut operands: Vec<Option<ir::Expr>> =
            asts.iter().zip(&bare).map(|(expr, &bare)| if bare { None } else { self.expr(expr) }).collect();
        for index in 0..asts.len() {
            if !bare[index] {
                continue;
            }
            let neighbour = [index.checked_sub(1), Some(index + 1)]
                .into_iter()
                .flatten()
                .find_map(|other| operands.get(other).and_then(|value| value.as_ref().map(|value| value.ty)));
            operands[index] = match neighbour {
                Some(ty) => self.expr_expecting(asts[index], ty),
                None => self.expr(asts[index]),
            };
        }
        let mut comparisons = Vec::new();
        for (index, comparison) in rest.iter().enumerate() {
            if let (Some(lhs), Some(rhs)) = (&operands[index], &operands[index + 1])
                && let Some(checked) = self.comparison(comparison.op, comparison.op_span, lhs, rhs)
            {
                comparisons.push(checked);
            }
        }
        if comparisons.len() != rest.len() {
            return None;
        }
        let operands: Vec<ir::Expr> = operands.into_iter().collect::<Option<_>>()?;
        Some(self.chain(operands, &comparisons, span))
    }

    fn comparison(
        &mut self,
        op: ast::CompareOp,
        op_span: Span,
        lhs: &ir::Expr,
        rhs: &ir::Expr,
    ) -> Option<Comparison> {
        use ast::CompareOp::*;
        use ir::BinaryOp as B;
        let equality = matches!(op, Eq | Ne);
        let pick = |ops: [B; 6]| match op {
            Eq => ops[0],
            Ne => ops[1],
            Lt => ops[2],
            Le => ops[3],
            Gt => ops[4],
            Ge => ops[5],
        };
        let int_ops = [B::EqInt, B::NeInt, B::LtInt, B::LeInt, B::GtInt, B::GeInt];
        let float_ops = [B::EqFloat, B::NeFloat, B::LtFloat, B::LeFloat, B::GtFloat, B::GeFloat];
        let (lty, rty) = (lhs.ty, rhs.ty);
        let equality_op = |eq: B, ne: B| if op == Eq { eq } else { ne };
        let plain = |op| Comparison { op, on_floats: false, on_positions: false };
        if matches!(lty, Type::Fun(_)) || matches!(rty, Type::Fun(_)) {
            self.diagnostics.push(
                Diagnostic::error("functions cannot be compared")
                    .with_primary(op_span, "")
                    .with_note("Lion 0.1 does not define `==` nor an order on functions (C65)"),
            );
            return None;
        }
        let checked = match (lty, rty) {
            (Type::Int, Type::Int) => plain(pick(int_ops)),
            _ if lty.is_numeric() && rty.is_numeric() => {
                Comparison { op: pick(float_ops), on_floats: true, on_positions: false }
            }
            (Type::Bool, Type::Bool) if equality => plain(equality_op(B::EqBool, B::NeBool)),
            (Type::Text, Type::Text) if equality => plain(equality_op(B::EqText, B::NeText)),
            (Type::None, Type::None) if equality => plain(equality_op(B::EqNone, B::NeNone)),
            // An ordered enumeration compares the positions of its values (D33).
            (Type::Enum(enumeration), _) if !equality && lty == rty && enumeration.is_ordered() => {
                Comparison { op: pick(int_ops), on_floats: false, on_positions: true }
            }
            (Type::Enum(enumeration), _) if !equality && lty == rty => {
                self.diagnostics.push(
                    Diagnostic::error(format!("`{}` is not defined for {lty} values", op.as_str()))
                        .with_primary(op_span, "")
                        .with_note(format!(
                            "the values of `{enumeration:?}` have no order: it is declared with `{{...}}`"
                        ))
                        .with_help("declare it with brackets, `[...]`, to order its values (§13.1, D33)"),
                );
                return None;
            }
            // Collections compare their content, structures their fields (§9.4, §12.5).
            (
                Type::List(_)
                | Type::Set(_)
                | Type::Tuple(_)
                | Type::Range
                | Type::Union(_)
                | Type::Struct(_)
                | Type::Enum(_),
                _,
            ) if equality && lty == rty => plain(equality_op(B::EqValue, B::NeValue)),
            // A value of a union compares with a value of one of its members, as in
            // `x == none` (§7.4).
            _ if equality && (lty.is_subset_of(rty) || rty.is_subset_of(lty)) => {
                plain(equality_op(B::EqValue, B::NeValue))
            }
            _ if lty == rty => {
                let mut error =
                    Diagnostic::error(format!("`{}` is not defined for {lty} values", op.as_str()))
                        .with_primary(op_span, "")
                        .with_note("`<`, `>`, `<=` and `>=` compare Int and Float values")
                        .with_note(format!("Lion 0.1 does not define an order on {lty}"));
                if let Type::Set(_) = lty {
                    error = error
                        .with_help("to test the inclusion of a Set in another, write `a subset b` (§16.6)");
                }
                self.diagnostics.push(error);
                return None;
            }
            _ => {
                self.diagnostics.push(
                    Diagnostic::error(format!("cannot compare {} with {}", article(lty), article(rty)))
                        .with_primary(op_span, "")
                        .with_secondary(lhs.span, format!("this is {}", article(lty)))
                        .with_secondary(rhs.span, format!("this is {}", article(rty)))
                        .with_note("Lion 0.1 does not define comparisons between values of different types"),
                );
                return None;
            }
        };
        Some(checked)
    }

    /// Builds the comparisons of a chain. Each operand is evaluated once, left to
    /// right, and evaluation stops at the first false comparison (§9.3, D69):
    /// `a < b < c` becomes `let t0 = a in let t1 = b in (t0 < t1) and (t1 < c)`.
    fn chain(&mut self, operands: Vec<ir::Expr>, comparisons: &[Comparison], span: Span) -> ir::Expr {
        let mut operands = operands.into_iter();
        let first = operands.next().expect("a comparison has two operands");
        if comparisons.len() == 1 {
            let second = operands.next().expect("a comparison has two operands");
            return compare_pair(comparisons[0], first, second, span);
        }
        let (first_ref, first_temp) = self.evaluate_once(first);
        let rest: Vec<ir::Expr> = operands.collect();
        let body = self.chain_links(first_ref, comparisons, rest, span);
        bind(first_temp, body, span)
    }

    /// `prev op[0] rest[0] and rest[0] op[1] rest[1] ...`, where `prev` is already evaluated.
    fn chain_links(
        &mut self,
        prev: ir::Expr,
        comparisons: &[Comparison],
        mut rest: Vec<ir::Expr>,
        span: Span,
    ) -> ir::Expr {
        let next = rest.remove(0);
        if rest.is_empty() {
            return compare_pair(comparisons[0], prev, next, span);
        }
        let (next_ref, next_temp) = self.evaluate_once(next);
        let head = compare_pair(comparisons[0], prev, next_ref.clone(), span);
        let tail = self.chain_links(next_ref, &comparisons[1..], rest, span);
        let both = typed(ir::ExprKind::And { lhs: Box::new(head), rhs: Box::new(tail) }, Type::Bool, span);
        bind(next_temp, both, span)
    }

    /// An expression that reads `expr` without evaluating it again: a literal stays
    /// as it is; anything else is stored in a temporary.
    fn evaluate_once(&mut self, expr: ir::Expr) -> (ir::Expr, Option<(ir::LocalId, ir::Expr)>) {
        let is_literal = matches!(
            expr.kind,
            ir::ExprKind::Int(_)
                | ir::ExprKind::Float(_)
                | ir::ExprKind::Bool(_)
                | ir::ExprKind::Text(_)
                | ir::ExprKind::None
        );
        if is_literal {
            return (expr, None);
        }
        let temp = self.temporary(expr.ty, expr.span);
        (typed(ir::ExprKind::Local(temp), expr.ty, expr.span), Some((temp, expr)))
    }

    /// `x as T` (§8.5).
    fn convert(&mut self, value: &ast::Expr, ty: &ast::TypeExpr, span: Span) -> Option<ir::Expr> {
        let target = self.resolve_type(ty);
        // `(...) as Student` builds a Student (§12.2).
        if let ast::ExprKind::Tuple(elements) = &value.kind {
            let Type::Struct(structure) = target? else {
                self.not_implemented(value.span, "tuples as values", "§16");
                return None;
            };
            let index = self.struct_index(structure);
            return self.construct(index, &Given::elements(elements), span);
        }
        let value = match target {
            Some(target) => self.expr_expecting(value, target),
            None => self.expr(value),
        };
        let (value, target) = (value?, target?);
        let value = self.within_try(value);
        let conversion = match (value.ty, target) {
            (from, to) if from == to => return Some(ir::Expr { span, ..value }),
            (from, to) if from.is_subset_of(to) => return Some(ir::Expr { span, ..value }),
            (Type::Int, Type::Float) => ir::Conversion::IntToFloat,
            (Type::Float, Type::Int) => ir::Conversion::FloatToInt,
            (Type::Int | Type::Float, Type::Text) => ir::Conversion::ToText,
            // From a Text, the conversion may fail: the result says why (§8.5, D14).
            (Type::Text, Type::Int) => {
                let ty = Type::union([Type::Int, Type::Error]);
                return Some(ir::Expr { span, ..convert(ir::Conversion::TextToInt, value, ty) });
            }
            (Type::Text, Type::Float) => {
                let ty = Type::union([Type::Float, Type::Error]);
                return Some(ir::Expr { span, ..convert(ir::Conversion::TextToFloat, value, ty) });
            }
            (from, to) => {
                let mut error = Diagnostic::error(format!("cannot convert {} to {to} with `as`", article(from)))
                    .with_primary(span, "")
                    .with_note("Lion 0.1 defines `as` between Int and Float, from Text to a number, and from a number to Text (§8.5)");
                if let Type::Struct(_) = to {
                    error = error.with_help(format!("a {to} is built from the values of its fields: `{to}(...)` or `(...) as {to}` (§12.2)"));
                }
                self.diagnostics.push(error);
                return None;
            }
        };
        Some(ir::Expr { span, ..convert(conversion, value, target) })
    }

    fn call(&mut self, callee: &ast::Expr, args: &[ast::Arg], span: Span) -> Option<ir::Expr> {
        let ast::ExprKind::Name(name) = &callee.kind else {
            return match callee.kind {
                ast::ExprKind::TypeName(ref name) => self.type_call(name, callee.span, args, span),
                ast::ExprKind::Field { ref name, .. } if name.name == "add" => {
                    self.diagnostics.push(
                        Diagnostic::error("`add` is called on its own line: `l.add(value)`")
                            .with_primary(span, "")
                            .with_note("`add` changes the list and gives no value"),
                    );
                    None
                }
                ast::ExprKind::Field { ref object, ref name } if name.name == "message" => {
                    self.message(object, args, span)
                }
                ast::ExprKind::Field { ref object, ref name } => {
                    if let ast::ExprKind::Name(module) = &object.kind
                        && let Some(module) = self.imported_module(module)
                    {
                        return self.module_member_call(module, name, args, span);
                    }
                    self.method_call(object, name, args, span)
                }
                // `f(1)(2)`, `handlers[1](x)`: a computed function value.
                _ => {
                    let value = self.expr(callee)?;
                    self.call_value(value, args, span)
                }
            };
        };
        let (decl_span, ty) = match self.resolve(name, callee.span) {
            Resolved::Function(index) => return self.call_function(index, callee.span, args, span),
            Resolved::Standard("show") => return self.show(args, span),
            Resolved::Standard("sum") => return self.sum(args, span),
            Resolved::Standard("error") => return self.error_value(args, span),
            Resolved::Standard(
                standard @ ("ask" | "exit" | "reverse" | "floor" | "ceil" | "round" | "isqrt"),
            ) => {
                return self.standard_call(standard, args, span);
            }
            Resolved::Standard(standard) => {
                self.not_implemented(callee.span, &format!("the standard function `{standard}`"), "§23");
                return None;
            }
            Resolved::Nothing => return None,
            Resolved::Local(local) => {
                let info = &self.ctx.locals[local.index()];
                (info.decl_span, info.ty)
            }
            Resolved::Global(local) => {
                let global = &self.global_info(local);
                let ty = match global.ty {
                    GlobalType::Known(ty) => ty,
                    GlobalType::Unknown => None,
                };
                (global.decl.name.span, ty)
            }
        };
        // A variable that holds a function (§11).
        if let Some(Type::Fun(_)) = ty {
            let value = self.expr(callee)?;
            return self.call_value(value, args, span);
        }
        let mut error = Diagnostic::error(format!("`{name}` is not a function"))
            .with_primary(callee.span, ty.map_or(String::new(), |ty| format!("`{name}` is {}", article(ty))))
            .with_secondary(decl_span, "declared here");
        if IMPLEMENTED_FUNCTIONS.contains(&name.as_str()) || PLANNED_FUNCTIONS.contains(&name.as_str()) {
            error = error.with_note(format!("this declaration hides the standard function `{name}`"));
        }
        self.diagnostics.push(error);
        None
    }

    /// `m.f(...)` or `m.Student(...)`: a function or a structure of a module (§20.2).
    fn module_member_call(
        &mut self,
        module: usize,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> Option<ir::Expr> {
        if !name.name.starts_with(char::is_uppercase) {
            return self.module_call(module, name, args, span);
        }
        let Some(&index) = self.table_of(module).struct_names.get(&name.name) else {
            let module_name = self.modules[module].name.clone();
            self.diagnostics.push(
                Diagnostic::error(format!("the module `{module_name}` has no structure `{}`", name.name))
                    .with_primary(name.span, ""),
            );
            return None;
        };
        self.construct(index, &Given::args(args), span)
    }

    /// `Student(...)`: builds a structure (§12.2).
    fn type_call(&mut self, name: &str, callee: Span, args: &[ast::Arg], span: Span) -> Option<ir::Expr> {
        if let Some(&index) = self.tables.struct_names.get(name) {
            return self.construct(index, &Given::args(args), span);
        }
        let ty = ast::TypeExpr {
            kind: ast::TypeExprKind::Named {
                module: Vec::new(),
                name: ast::Ident { name: name.to_string(), span: callee },
                args: Vec::new(),
            },
            span: callee,
        };
        let ty = self.resolve_type(&ty)?;
        let mut error = Diagnostic::error(format!("`{name}` is not a structure"))
            .with_primary(callee, "")
            .with_note("`Type(...)` builds a structure (§12.2)");
        if matches!(ty, Type::Int | Type::Float | Type::Text) {
            error = error.with_help(format!("to convert a value, write `x as {name}` (§8.5)"));
        }
        self.diagnostics.push(error);
        None
    }

    /// `error(message)`: a simple Error (§18.2, D65).
    fn error_value(&mut self, args: &[ast::Arg], span: Span) -> Option<ir::Expr> {
        let [arg] = args else {
            self.diagnostics.push(
                Diagnostic::error(format!("`error` takes one message, not {} values", args.len()))
                    .with_primary(span, ""),
            );
            return None;
        };
        let message = self.expr(&arg.value)?;
        let message = self.coerce(message, Type::Text, None)?;
        let kind = ir::ExprKind::CallBuiltin { builtin: ir::Builtin::Error, args: vec![message] };
        Some(typed(kind, Type::Error, span))
    }

    /// `e.message()`: the text of an Error (§18.2, D66).
    fn message(&mut self, object: &ast::Expr, args: &[ast::Arg], span: Span) -> Option<ir::Expr> {
        let error = self.expr(object)?;
        let error = self.within_try(error);
        if error.ty != Type::Error {
            let mut diagnostic = Diagnostic::error(format!("{} has no `message()`", article(error.ty)))
                .with_primary(error.span, "")
                .with_note("`message()` gives the text of an Error (§18.2)");
            if error.ty.members().contains(&Type::Error) {
                diagnostic =
                    diagnostic.with_help("test it first: `if x in Error: show(x.message()) ;` (§7.4)");
            }
            self.diagnostics.push(diagnostic);
            return None;
        }
        if !args.is_empty() {
            self.diagnostics.push(Diagnostic::error("`message()` takes no argument").with_primary(span, ""));
            return None;
        }
        let kind = ir::ExprKind::CallBuiltin { builtin: ir::Builtin::Message, args: vec![error] };
        Some(typed(kind, Type::Text, span))
    }

    /// `show(value)`: any value can be shown (§23, D29).
    fn show(&mut self, args: &[ast::Arg], span: Span) -> Option<ir::Expr> {
        let mut valid = true;
        for arg in args {
            if let Some(var_span) = arg.var_marker {
                self.diagnostics.push(
                    Diagnostic::error("`show` does not modify its argument")
                        .with_primary(var_span, "remove this `var`")
                        .with_note("`var` at a call marks an argument that the function modifies (§11.2)"),
                );
                valid = false;
            }
            if let Some(name) = &arg.name {
                self.diagnostics.push(
                    Diagnostic::error("`show` does not take named arguments")
                        .with_primary(name.span, "")
                        .with_note("Lion 0.1 does not name the parameters of the standard functions"),
                );
                valid = false;
            }
        }
        if args.len() != 1 {
            let mut error = Diagnostic::error(format!("`show` takes exactly one value, not {}", args.len()))
                .with_primary(span, "");
            if args.len() > 1 {
                error = error.with_help("to show several values, use interpolation: show(\"{a} {b}\")");
            }
            self.diagnostics.push(error);
            valid = false;
        }
        let values: Vec<Option<ir::Expr>> = args.iter().map(|arg| self.expr(&arg.value)).collect();
        if !valid {
            return None;
        }
        let value = values.into_iter().next().flatten()?;
        let kind = ir::ExprKind::CallBuiltin { builtin: ir::Builtin::Show, args: vec![value] };
        Some(typed(kind, Type::None, span))
    }

    /// Checks that `expr` fits where a value of type `expected` is required. The only
    /// implicit conversion is Int to Float (§6.3, §8.5).
    pub(crate) fn coerce(
        &mut self,
        expr: ir::Expr,
        expected: Type,
        context: Option<(Span, String)>,
    ) -> Option<ir::Expr> {
        if expr.ty == expected || expr.ty.is_subset_of(expected) {
            return Some(expr);
        }
        // A function with default values goes where fewer arguments are required.
        if let (Type::Fun(given), Type::Fun(wanted)) = (expr.ty, expected) {
            let (given, wanted) = (given.get(), wanted.get());
            if given.params == wanted.params
                && given.ret.is_subset_of(wanted.ret)
                && given.required <= wanted.required
            {
                return Some(ir::Expr { ty: expected, ..expr });
            }
        }
        // An Int goes where a Float is expected, also in a union (§8.5).
        let members = expected.members();
        if expr.ty == Type::Int && members.contains(&Type::Float) && !members.contains(&Type::Int) {
            return Some(to_float(expr));
        }
        let expr = self.within_try(expr);
        if expr.ty.is_subset_of(expected) {
            return Some(expr);
        }
        let mut error = Diagnostic::error("mismatched types")
            .with_primary(expr.span, format!("this is {}", article(expr.ty)));
        if let Some((span, message)) = context {
            error = error.with_secondary(span, message);
        }
        error = error.with_note(format!("expected: {expected}")).with_note(format!("found: {}", expr.ty));
        if expected.is_subset_of(expr.ty)
            && let Some(help) = union_help(expr.ty)
        {
            error = error.with_help(help);
        }
        self.diagnostics.push(error);
        None
    }
}

/// How to use a value of a union type as one of its members (§7.4, §18).
pub(crate) fn union_help(ty: Type) -> Option<String> {
    let members = ty.members();
    if members.len() < 2 {
        return None;
    }
    Some(if members.contains(&Type::Error) {
        "an Error must be handled first: `if x in Error: ... ;`, or `try x` (§18.3)".to_string()
    } else if members.contains(&Type::None) {
        "the value may be `none`: test it first, for instance `if x == none: return ;` (§7.4)".to_string()
    } else {
        format!("test the type of the value first, for instance `if x in {}: ... ;` (§7.4)", members[0])
    })
}

fn convert(conversion: ir::Conversion, value: ir::Expr, ty: Type) -> ir::Expr {
    let span = value.span;
    typed(ir::ExprKind::Convert { conversion, value: Box::new(value) }, ty, span)
}

fn to_float(expr: ir::Expr) -> ir::Expr {
    if expr.ty == Type::Int { convert(ir::Conversion::IntToFloat, expr, Type::Float) } else { expr }
}

impl Checker<'_> {
    /// `lhs == rhs`, for the value patterns of `match` (§10.3).
    pub(crate) fn equality_test(&mut self, lhs: ir::Expr, rhs: ir::Expr, span: Span) -> Option<ir::Expr> {
        let comparison = self.comparison(ast::CompareOp::Eq, span, &lhs, &rhs)?;
        Some(compare_pair(comparison, lhs, rhs, span))
    }
}

fn compare_pair(comparison: Comparison, lhs: ir::Expr, rhs: ir::Expr, span: Span) -> ir::Expr {
    let (lhs, rhs) = if comparison.on_floats { (to_float(lhs), to_float(rhs)) } else { (lhs, rhs) };
    let position = |value: ir::Expr| convert(ir::Conversion::EnumPosition, value, Type::Int);
    let (lhs, rhs) = if comparison.on_positions { (position(lhs), position(rhs)) } else { (lhs, rhs) };
    let kind = ir::ExprKind::Binary { op: comparison.op, lhs: Box::new(lhs), rhs: Box::new(rhs) };
    typed(kind, Type::Bool, span)
}

/// Wraps `body` in the binding of a temporary, if there is one.
fn bind(temp: Option<(ir::LocalId, ir::Expr)>, body: ir::Expr, span: Span) -> ir::Expr {
    match temp {
        None => body,
        Some((local, value)) => {
            let ty = body.ty;
            let kind = ir::ExprKind::Let { local, value: Box::new(value), body: Box::new(body) };
            typed(kind, ty, span)
        }
    }
}
