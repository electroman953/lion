//! Forms that the author added to the language on 2026-11-16: the Maps written with
//! their values, `{"a" -> 1}` (C112), `value otherwise default` (C113), and the
//! elements of tuples, `t[1]` and `let (a, b) = t` (C115), and the text of a value
//! given by its type, `fun T.text() in Text` (C116).

use lion_diagnostics::{Diagnostic, Span};
use lion_ir::{self as ir, Type};
use lion_syntax::ast;

use crate::{Checker, article, typed};

impl Checker<'_> {
    /// `{k -> v, …}`: a Map whose keys and values have one type each, or the types of
    /// the Map expected (C112). A key written twice keeps its last value.
    pub(crate) fn map_literal(
        &mut self,
        entries: &[(ast::Expr, ast::Expr)],
        span: Span,
        expected: Option<Type>,
    ) -> Option<ir::Expr> {
        let parts = expected.and_then(|ty| ty.members().into_iter().find_map(Type::map_parts));
        let mut keys = Vec::new();
        let mut values = Vec::new();
        for (key, value) in entries {
            let key = match parts {
                Some((k, _)) => self.expr_expecting(key, k),
                None => self.expr(key),
            };
            let value = match parts {
                Some((_, v)) => self.expr_expecting(value, v),
                None => self.expr(value),
            };
            keys.push(key);
            values.push(value);
        }
        let keys: Vec<ir::Expr> = keys.into_iter().collect::<Option<_>>()?;
        let values: Vec<ir::Expr> = values.into_iter().collect::<Option<_>>()?;
        let (key_ty, value_ty) = match parts {
            Some(parts) => parts,
            None => (self.common_type(&keys, span, "keys")?, self.common_type(&values, span, "values")?),
        };
        let mut checked = Vec::new();
        for (key, value) in keys.into_iter().zip(values) {
            let key = self.coerce(key, key_ty, None)?;
            let value = self.coerce(value, value_ty, None)?;
            checked.push((key, value));
        }
        // A key written twice: the last value stays, in the place of the first key.
        let mut entries: Vec<(ir::Expr, ir::Expr)> = Vec::new();
        for (key, value) in checked {
            let same =
                |other: &ir::Expr| constant_key(other).is_some() && constant_key(other) == constant_key(&key);
            match entries.iter_mut().find(|(other, _)| same(other)) {
                Some(entry) => entry.1 = value,
                None => entries.push((key, value)),
            }
        }
        Some(typed(ir::ExprKind::Map(entries), Type::map(key_ty, value_ty), span))
    }

    /// `value otherwise default`: the value, unless it is `none`; then `default`, which
    /// is computed only in that case (C113).
    pub(crate) fn otherwise(
        &mut self,
        value: &ast::Expr,
        default: &ast::Expr,
        span: Span,
        expected: Option<Type>,
    ) -> Option<ir::Expr> {
        let value = match expected {
            Some(expected) => self.expr_expecting(value, Type::maybe(expected)),
            None => self.expr(value),
        }?;
        let Some(present) = value.ty.without(Type::None).filter(|_| Type::None.is_subset_of(value.ty)) else {
            self.diagnostics.push(
                Diagnostic::error(format!("this value is never `none`: it is {}", article(value.ty)))
                    .with_primary(value.span, "")
                    .with_note("`otherwise` gives a value in place of `none` (C113)")
                    .with_help("remove `otherwise` and its value"),
            );
            self.expr(default);
            return None;
        };
        let default = match expected.or(Some(present)) {
            Some(wanted) if wanted.members().len() == 1 => self.expr_expecting(default, wanted),
            _ => self.expr(default),
        }?;
        let ty = match (present, default.ty) {
            (a, b) if a == b => a,
            (Type::Int, Type::Float) | (Type::Float, Type::Int) => Type::Float,
            (a, b) => Type::union([a, b]),
        };
        let temp = self.temporary(value.ty, value.span);
        let read = |ty: Type| typed(ir::ExprKind::Local(temp), ty, value.span);
        let given = crate::collections::widen(read(present), ty);
        let default = crate::collections::widen(default, ty);
        let cond = typed(
            ir::ExprKind::TypeTest { value: Box::new(read(value.ty)), ty: Type::None },
            Type::Bool,
            span,
        );
        let choice = typed(
            ir::ExprKind::If { cond: Box::new(cond), then: Box::new(default), otherwise: Box::new(given) },
            ty,
            span,
        );
        Some(typed(
            ir::ExprKind::Let { local: temp, value: Box::new(value), body: Box::new(choice) },
            ty,
            span,
        ))
    }
}

impl Checker<'_> {
    /// `t[1]`: the element of a tuple at a position written in the program, from 1, as
    /// in a List (C115).
    pub(crate) fn tuple_element(
        &mut self,
        tuple: ir::Expr,
        index: &ast::Expr,
        span: Span,
    ) -> Option<ir::Expr> {
        let Type::Tuple(parts) = tuple.ty else { unreachable!("a tuple") };
        let elements = parts.elements();
        let position = match &index.kind {
            ast::ExprKind::Int(n) => Some(*n),
            _ => None,
        };
        let Some(position) = position.filter(|n| *n >= 1 && *n as usize <= elements.len()) else {
            self.diagnostics.push(
                Diagnostic::error(format!(
                    "a tuple of {} elements is indexed by a number from 1 to {}",
                    elements.len(),
                    elements.len()
                ))
                .with_primary(index.span, "")
                .with_note(
                    "the position is written in the program, so that the type of the element is known (C115)",
                ),
            );
            return None;
        };
        let ty = elements[position as usize - 1];
        Some(typed(ir::ExprKind::Field { object: Box::new(tuple), field: position as u32 - 1 }, ty, span))
    }

    /// `let (a, b) = t`: one variable per element of the tuple (C115).
    pub(crate) fn destructure(
        &mut self,
        mutable: bool,
        names: &[ast::Ident],
        value: &ast::Expr,
        span: Span,
    ) -> Option<ir::Stmt> {
        let value = self.expr(value)?;
        let Type::Tuple(parts) = value.ty else {
            self.diagnostics.push(
                Diagnostic::error(format!("only a tuple is taken apart; this is {}", article(value.ty)))
                    .with_primary(value.span, "")
                    .with_note("`let (a, b) = t` gives the elements of a tuple (C115)"),
            );
            return None;
        };
        let elements = parts.elements();
        if elements.len() != names.len() {
            self.diagnostics.push(
                Diagnostic::error(format!(
                    "this tuple has {} elements, for {} names",
                    elements.len(),
                    names.len()
                ))
                .with_primary(value.span, format!("this is {}", article(value.ty)))
                .with_secondary(names[0].span.to(names[names.len() - 1].span), ""),
            );
            return None;
        }
        let temp = self.temporary(value.ty, value.span);
        let mut stmts = vec![ir::Stmt::Assign { place: ir::Place::Local(temp), value }];
        for (position, (name, ty)) in names.iter().zip(elements).enumerate() {
            let local = self.declare(name, Some(ty), mutable, true);
            let object = Box::new(typed(ir::ExprKind::Local(temp), Type::Tuple(parts), span));
            stmts.push(ir::Stmt::Assign {
                place: ir::Place::Local(local),
                value: typed(ir::ExprKind::Field { object, field: position as u32 }, ty, span),
            });
        }
        Some(ir::Stmt::Seq(stmts))
    }
}

impl Checker<'_> {
    /// The text that `show` and `{x}` give for the value: its method `text()` when its
    /// type, a structure or an enumeration, has one without parameters that gives a Text
    /// (C116); `None` otherwise. Inside that method, `{self}` keeps the usual text.
    pub(crate) fn own_text(&mut self, value: &ir::Expr) -> Option<ir::Expr> {
        if !matches!(value.ty, Type::Struct(_) | Type::Enum(_)) {
            return None;
        }
        let method = self.visible_method(value.ty, "text")?;
        let function = &self.functions[method];
        let fits = !function.var_self
            && function.declared_ret == Some(Type::Text)
            && function.signature.as_ref().is_some_and(|params| params.len() == 1);
        let inside = matches!(self.ctx.kind, crate::ContextKind::Function(instance) if self.instance_function(instance) == Some(method));
        if !fits || inside {
            return None;
        }
        let span = value.span;
        self.call_with(
            method,
            Some(crate::functions::Pending::Value(value.clone())),
            Vec::new(),
            span,
            &[],
            span,
        )
    }
}

/// A key known when the program is compiled, to find a key written twice.
fn constant_key(key: &ir::Expr) -> Option<String> {
    match &key.kind {
        ir::ExprKind::Int(n) => Some(format!("i{n}")),
        ir::ExprKind::Text(t) => Some(format!("t{t}")),
        ir::ExprKind::Bool(b) => Some(format!("b{b}")),
        ir::ExprKind::Enum { enumeration, value } => Some(format!("e{enumeration:?}{value}")),
        _ => None,
    }
}
