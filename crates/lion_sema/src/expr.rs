//! Type checking of expressions (spec §7 to §9).
//!
//! Each function returns `None` when the expression has an error, after reporting
//! it; the callers then stay silent, so one mistake produces one message.

use lion_diagnostics::{Diagnostic, Span};
use lion_ir::{self as ir, Type};
use lion_syntax::ast;

use crate::names::{IMPLEMENTED_FUNCTIONS, PLANNED_FUNCTIONS, Resolved};
use crate::{Checker, GlobalType, article, typed};

/// A comparison between two operands, specialised to their types.
#[derive(Clone, Copy)]
struct Comparison {
    op: ir::BinaryOp,
    /// Int operands are converted to Float first.
    on_floats: bool,
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
            ast::ExprKind::Field { object, name } => self.property(object, name, span),
            ast::ExprKind::Index { object, index } => self.index(object, index, span),
            ast::ExprKind::List(elements) => self.list(elements, span),
            ast::ExprKind::If { branches, otherwise } => self.if_expr(branches, otherwise.as_deref(), span),
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
    fn if_expr(
        &mut self,
        branches: &[(ast::Expr, ast::Expr)],
        otherwise: Option<&ast::Expr>,
        span: Span,
    ) -> Option<ir::Expr> {
        let mut checked = Vec::new();
        for (cond, value) in branches {
            checked.push((self.condition(cond, "if"), self.expr(value)));
        }
        let Some(otherwise) = otherwise else {
            self.not_implemented(
                span,
                "`if ... then` without `else` (its value is a `maybe`, which needs union types)",
                "§10.1, §7.3",
            );
            return None;
        };
        let otherwise = self.expr(otherwise);
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
        let first = values[0].ty;
        let ty = if values.iter().all(|value| value.ty == first) {
            first
        } else if values.iter().all(|value| value.ty.is_numeric()) {
            Type::Float
        } else {
            self.not_implemented(
                span,
                "`if` expressions whose branches have different types (the value would be a union such as `Int or Text`)",
                "§7.3, §10.1",
            );
            return None;
        };
        let mut values =
            values.into_iter().map(|value| if ty == Type::Float { to_float(value) } else { value });
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
        if name == "self" {
            self.not_implemented(span, "methods and `self`", "§12.4");
            return None;
        }
        match self.resolve(name, span) {
            Resolved::Local(local) => {
                let ty = self.ctx.locals[local.index()].ty?;
                self.check_has_value(local, span).then(|| typed(ir::ExprKind::Local(local), ty, span))
            }
            // Whether it has a value is checked at the calls of the script (C3).
            Resolved::Global(local) => {
                self.ctx.reads.push(local);
                let GlobalType::Known(ty) = self.globals[&self.global_names[&local]].ty else { return None };
                Some(typed(ir::ExprKind::Global(local), ty?, span))
            }
            Resolved::Function(_) | Resolved::Standard(_) => {
                self.not_implemented(span, "functions used as values", "§11.3");
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
            Inter | Union | Minus | Subset => Some(("set operations", "§16.6")),
            In if matches!(rhs.kind, ast::ExprKind::TypeName(_)) => Some(("type tests (`x in T`)", "§7.1")),
            Same => Some(("`same`", "§9.4, §17.2")),
            _ => None,
        };
        if let Some((what, section)) = unsupported {
            self.not_implemented(op_span, what, section);
            return None;
        }
        let lhs = self.expr(lhs);
        let rhs = self.expr(rhs);
        let (lhs, rhs) = (lhs?, rhs?);
        match op {
            And | Or => self.logical(op, lhs, rhs, span),
            Range => self.range(lhs, rhs, span),
            In => self.membership(lhs, rhs, op_span, span),
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
            Type::List(_) => self.list_membership(value, set, span),
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
        self.diagnostics.push(error);
    }

    /// One comparison, or a chain such as `0 <= grade <= 20` (§9.3).
    fn compare(&mut self, first: &ast::Expr, rest: &[ast::Comparison], span: Span) -> Option<ir::Expr> {
        let mut operands = vec![self.expr(first)];
        for comparison in rest {
            operands.push(self.expr(&comparison.rhs));
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
        let checked = match (lty, rty) {
            (Type::Int, Type::Int) => Comparison { op: pick(int_ops), on_floats: false },
            _ if lty.is_numeric() && rty.is_numeric() => Comparison { op: pick(float_ops), on_floats: true },
            (Type::Bool, Type::Bool) if equality => {
                Comparison { op: equality_op(B::EqBool, B::NeBool), on_floats: false }
            }
            (Type::Text, Type::Text) if equality => {
                Comparison { op: equality_op(B::EqText, B::NeText), on_floats: false }
            }
            (Type::None, Type::None) if equality => {
                Comparison { op: equality_op(B::EqNone, B::NeNone), on_floats: false }
            }
            // Collections compare their content (§9.4).
            (Type::List(_) | Type::Range, _) if equality && lty == rty => {
                Comparison { op: equality_op(B::EqValue, B::NeValue), on_floats: false }
            }
            _ if lty == rty => {
                self.diagnostics.push(
                    Diagnostic::error(format!("`{}` is not defined for {lty} values", op.as_str()))
                        .with_primary(op_span, "")
                        .with_note("`<`, `>`, `<=` and `>=` compare Int and Float values")
                        .with_note(format!("Lion 0.1 does not define an order on {lty}")),
                );
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
        let value = match target {
            Some(target) => self.expr_expecting(value, target),
            None => self.expr(value),
        };
        let (value, target) = (value?, target?);
        let conversion = match (value.ty, target) {
            (from, to) if from == to => return Some(ir::Expr { span, ..value }),
            (Type::Int, Type::Float) => ir::Conversion::IntToFloat,
            (Type::Float, Type::Int) => ir::Conversion::FloatToInt,
            (Type::Int | Type::Float, Type::Text) => ir::Conversion::ToText,
            (Type::Text, Type::Int | Type::Float) => {
                self.not_implemented(
                    span,
                    "converting a Text to a number (its result, `Int or Error`, needs union types)",
                    "§8.5, §18",
                );
                return None;
            }
            (from, to) => {
                self.diagnostics.push(
                    Diagnostic::error(format!("cannot convert {} to {to} with `as`", article(from)))
                        .with_primary(span, "")
                        .with_note("Lion 0.1 defines `as` between Int and Float, from Text to a number, and from a number to Text (§8.5)"),
                );
                return None;
            }
        };
        Some(ir::Expr { span, ..convert(conversion, value, target) })
    }

    fn call(&mut self, callee: &ast::Expr, args: &[ast::Arg], span: Span) -> Option<ir::Expr> {
        let ast::ExprKind::Name(name) = &callee.kind else {
            let (what, section) = match callee.kind {
                ast::ExprKind::TypeName(_) => ("building structures", "§12.2"),
                ast::ExprKind::Field { ref name, .. } if name.name == "add" => {
                    self.diagnostics.push(
                        Diagnostic::error("`add` is called on its own line: `l.add(value)`")
                            .with_primary(span, "")
                            .with_note("`add` changes the list and gives no value"),
                    );
                    return None;
                }
                ast::ExprKind::Field { .. } => ("methods", "§12.4"),
                _ => ("calling a computed function", "§11.3"),
            };
            self.not_implemented(callee.span, what, section);
            return None;
        };
        let (decl_span, ty) = match self.resolve(name, callee.span) {
            Resolved::Function(index) => return self.call_function(index, callee.span, args, span),
            Resolved::Standard("show") => return self.show(args, span),
            Resolved::Standard("sum") => return self.sum(args, span),
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
                let global = &self.globals[&self.global_names[&local]];
                let ty = match global.ty {
                    GlobalType::Known(ty) => ty,
                    GlobalType::Unknown => None,
                };
                (global.decl.name.span, ty)
            }
        };
        let mut error = Diagnostic::error(format!("`{name}` is not a function"))
            .with_primary(callee.span, ty.map_or(String::new(), |ty| format!("`{name}` is {}", article(ty))))
            .with_secondary(decl_span, "declared here");
        if IMPLEMENTED_FUNCTIONS.contains(&name.as_str()) || PLANNED_FUNCTIONS.contains(&name.as_str()) {
            error = error.with_note(format!("this declaration hides the standard function `{name}`"));
        }
        self.diagnostics.push(error);
        None
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
        if expr.ty == expected {
            return Some(expr);
        }
        if expr.ty == Type::Int && expected == Type::Float {
            return Some(to_float(expr));
        }
        let mut error = Diagnostic::error("mismatched types")
            .with_primary(expr.span, format!("this is {}", article(expr.ty)));
        if let Some((span, message)) = context {
            error = error.with_secondary(span, message);
        }
        error = error.with_note(format!("expected: {expected}")).with_note(format!("found: {}", expr.ty));
        self.diagnostics.push(error);
        None
    }
}

fn convert(conversion: ir::Conversion, value: ir::Expr, ty: Type) -> ir::Expr {
    let span = value.span;
    typed(ir::ExprKind::Convert { conversion, value: Box::new(value) }, ty, span)
}

fn to_float(expr: ir::Expr) -> ir::Expr {
    if expr.ty == Type::Int { convert(ir::Conversion::IntToFloat, expr, Type::Float) } else { expr }
}

fn compare_pair(comparison: Comparison, lhs: ir::Expr, rhs: ir::Expr, span: Span) -> ir::Expr {
    let (lhs, rhs) = if comparison.on_floats { (to_float(lhs), to_float(rhs)) } else { (lhs, rhs) };
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
