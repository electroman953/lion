//! Lists and texts as sequences (spec §16): literals, comprehensions, indices,
//! extracts, properties and changes in place.

use std::collections::HashSet;

use lion_diagnostics::{Diagnostic, Span};
use lion_ir::{self as ir, Type};
use lion_syntax::ast;

use crate::flow::Assigned;
use crate::names::Resolved;
use crate::{Checker, GlobalType, Scope, article, typed};

/// One element of a comprehension after the output (§16.4).
enum Part {
    Generator { var: ir::LocalId, iterable: ir::Expr },
    Condition(ir::Expr),
}

impl Checker<'_> {
    /// `[a, b, c]`, or a comprehension when an element is `v in X` with a new `v`.
    pub(crate) fn list(&mut self, elements: &[ast::Expr], span: Span) -> Option<ir::Expr> {
        let generators = self.generators(elements);
        if generators.iter().any(|&is_generator| is_generator) {
            return self.comprehension(elements, &generators, span);
        }
        if elements.is_empty() {
            self.diagnostics.push(
                Diagnostic::error("the type of this empty list is not known")
                    .with_primary(span, "")
                    .with_help("write its type: `var l = [] in List of Int`, or `[] as List of Int`"),
            );
            return None;
        }
        let values: Vec<Option<ir::Expr>> = elements.iter().map(|element| self.expr(element)).collect();
        let values: Vec<ir::Expr> = values.into_iter().collect::<Option<_>>()?;
        let element = self.common_type(&values, span, "the elements of a list")?;
        let values = values.into_iter().map(|value| widen(value, element)).collect();
        Some(typed(ir::ExprKind::List(values), Type::list(element), span))
    }

    /// An empty list takes the type expected where it is written.
    pub(crate) fn empty_list(&mut self, expr: &ast::Expr, expected: Type) -> Option<ir::Expr> {
        match (&expr.kind, expected) {
            (ast::ExprKind::List(elements), Type::List(_)) if elements.is_empty() => {
                Some(typed(ir::ExprKind::List(Vec::new()), expected, expr.span))
            }
            (ast::ExprKind::Paren(inner), _) => self.empty_list(inner, expected),
            _ => None,
        }
    }

    /// Checks an expression where a value of type `expected` is required.
    pub(crate) fn expr_expecting(&mut self, expr: &ast::Expr, expected: Type) -> Option<ir::Expr> {
        match self.empty_list(expr, expected) {
            Some(empty) => Some(empty),
            None => self.expr(expr),
        }
    }

    /// One type for several values: the same, or Float when Int and Float are mixed (§8.5).
    pub(crate) fn common_type(&mut self, values: &[ir::Expr], span: Span, what: &str) -> Option<Type> {
        let first = values[0].ty;
        if values.iter().all(|value| value.ty == first) {
            return Some(first);
        }
        if values.iter().all(|value| value.ty.is_numeric()) {
            return Some(Type::Float);
        }
        self.not_implemented(
            span,
            &format!("{what} of different types (the type would be a union such as `Int or Text`)"),
            "§7.3",
        );
        None
    }

    /// Which elements are generators: `v in X` where `v` is new at that point (§16.4).
    fn generators(&self, elements: &[ast::Expr]) -> Vec<bool> {
        let mut new_names = HashSet::new();
        elements
            .iter()
            .map(|element| match &element.kind {
                ast::ExprKind::Binary { op: ast::BinaryOp::In, lhs, .. } => match &lhs.kind {
                    ast::ExprKind::Name(name) if !self.is_known(name) && !new_names.contains(name) => {
                        new_names.insert(name.clone());
                        true
                    }
                    _ => false,
                },
                _ => false,
            })
            .collect()
    }

    /// `[output, v in X, condition, ...]`: the generators nest from left to right and
    /// the conditions filter, like loops and `if`s (§16.4).
    fn comprehension(&mut self, elements: &[ast::Expr], generators: &[bool], span: Span) -> Option<ir::Expr> {
        let implicit_output = generators[0];
        if implicit_output && generators.iter().filter(|&&is_generator| is_generator).count() > 1 {
            self.diagnostics.push(
                Diagnostic::error("with several generators, a comprehension starts with its result")
                    .with_primary(elements[0].span, "")
                    .with_help("write the result first: `[(a, b), a in A, b in B]` (§16.4)"),
            );
            return None;
        }
        self.ctx.scopes.push(Scope::default());
        let rest = if implicit_output { elements } else { &elements[1..] };
        let rest_generators = if implicit_output { generators } else { &generators[1..] };
        let mut parts = Vec::new();
        let mut valid = true;
        let mut first_var = None;
        for (element, &is_generator) in rest.iter().zip(rest_generators) {
            let part = if is_generator { self.generator(element) } else { self.filter(element) };
            match part {
                Some(Part::Generator { var, iterable }) => {
                    first_var.get_or_insert(var);
                    parts.push(Part::Generator { var, iterable });
                }
                Some(part) => parts.push(part),
                None => valid = false,
            }
        }
        let output = if implicit_output {
            first_var.map(|var| {
                let ty = self.ctx.locals[var.index()].ty.unwrap_or(Type::None);
                typed(ir::ExprKind::Local(var), ty, elements[0].span)
            })
        } else {
            self.expr(&elements[0])
        };
        self.ctx.scopes.pop();
        let output = output.filter(|_| valid)?;
        let list_type = Type::list(output.ty);
        let result = self.temporary(list_type, span);
        let mut body =
            vec![ir::Stmt::Add { root: ir::Place::Local(result), indices: Vec::new(), value: output }];
        for part in parts.into_iter().rev() {
            body = vec![match part {
                Part::Generator { var, iterable } => ir::Stmt::For { var, iterable, body },
                Part::Condition(cond) => ir::Stmt::If { cond, then: body, otherwise: Vec::new() },
            }];
        }
        let empty = typed(ir::ExprKind::List(Vec::new()), list_type, span);
        let mut stmts = vec![ir::Stmt::Assign { place: ir::Place::Local(result), value: empty }];
        stmts.extend(body);
        let value = typed(ir::ExprKind::Local(result), list_type, span);
        Some(typed(ir::ExprKind::Block { stmts, value: Box::new(value) }, list_type, span))
    }

    fn generator(&mut self, element: &ast::Expr) -> Option<Part> {
        let ast::ExprKind::Binary { lhs, rhs, .. } = &element.kind else {
            unreachable!("a generator is `v in X`")
        };
        let ast::ExprKind::Name(name) = &lhs.kind else { unreachable!("a generator is `v in X`") };
        if let ast::ExprKind::TypeName(_) = rhs.kind {
            self.not_implemented(element.span, "comprehensions over a type, which make a `Domain`", "§16.5");
            return None;
        }
        let iterable = self.expr(rhs)?;
        let Some(element_type) = iterable.ty.element() else {
            self.diagnostics.push(
                Diagnostic::error(format!("a generator cannot go through {}", article(iterable.ty)))
                    .with_primary(rhs.span, "")
                    .with_note("a generator goes through an interval or a list (§16.4)"),
            );
            return None;
        };
        let ident = ast::Ident { name: name.clone(), span: lhs.span };
        let var = self.declare(&ident, Some(element_type), false, true);
        self.ctx.locals[var.index()].loop_variable = true;
        Some(Part::Generator { var, iterable })
    }

    fn filter(&mut self, element: &ast::Expr) -> Option<Part> {
        let cond = self.expr(element)?;
        if cond.ty != Type::Bool {
            self.diagnostics.push(
                Diagnostic::error("a condition of a comprehension must be a Bool")
                    .with_primary(cond.span, format!("this is {}", article(cond.ty)))
                    .with_note(
                        "after the result, each element is a generator `v in X` or a condition (§16.4)",
                    ),
            );
            return None;
        }
        Some(Part::Condition(cond))
    }

    /// `l[i]`, `l[a..b]`, `t[i]` (§16.2).
    pub(crate) fn index(&mut self, object: &ast::Expr, index: &ast::Expr, span: Span) -> Option<ir::Expr> {
        let object = self.expr(object);
        let index = self.expr(index);
        let (object, index) = (object?, index?);
        let element = match object.ty {
            Type::List(element) => element.get(),
            Type::Text => Type::Text,
            other => {
                self.diagnostics.push(
                    Diagnostic::error(format!("{} has no elements to index", capitalize(&article(other))))
                        .with_primary(object.span, "")
                        .with_note("a List and a Text are indexed from 1: `l[1]` (§16.2)"),
                );
                return None;
            }
        };
        let (object, index) = (Box::new(object), Box::new(index));
        match index.ty {
            Type::Int => Some(typed(ir::ExprKind::Index { object, index }, element, span)),
            Type::Range => {
                let ty = object.ty;
                Some(typed(ir::ExprKind::Slice { object, range: index }, ty, span))
            }
            other => {
                self.diagnostics.push(
                    Diagnostic::error("an index is an Int, or an interval for an extract")
                        .with_primary(index.span, format!("this is {}", article(other))),
                );
                None
            }
        }
    }

    /// `l.size`, `l.first`, `l.last`, `t.size`, `r.size` (§16.2, D38).
    pub(crate) fn property(&mut self, object: &ast::Expr, name: &ast::Ident, span: Span) -> Option<ir::Expr> {
        let object = self.expr(object)?;
        let (property, ty) = match (object.ty, name.name.as_str()) {
            (Type::List(_) | Type::Text | Type::Range, "size") => (ir::Property::Size, Type::Int),
            (Type::List(element), "first") => (ir::Property::First, element.get()),
            (Type::List(element), "last") => (ir::Property::Last, element.get()),
            (Type::List(_), "add") => {
                self.diagnostics.push(
                    Diagnostic::error("`add` is called on its own line: `l.add(value)`")
                        .with_primary(span, "")
                        .with_note("`add` changes the list and gives no value"),
                );
                return None;
            }
            (other, field) => {
                let known = match other {
                    Type::List(_) => "`size`, `first`, `last` and `add`",
                    Type::Text | Type::Range => "`size`",
                    _ => "no field",
                };
                self.diagnostics.push(
                    Diagnostic::error(format!("{} has no `{field}`", capitalize(&article(other))))
                        .with_primary(name.span, "")
                        .with_note(format!("it has {known} (§16)")),
                );
                return None;
            }
        };
        let kind = ir::ExprKind::Property { object: Box::new(object), property };
        Some(typed(kind, ty, span))
    }

    /// `x in l`: compared with `==` (§16.2).
    pub(crate) fn list_membership(
        &mut self,
        value: ir::Expr,
        list: ir::Expr,
        span: Span,
    ) -> Option<ir::Expr> {
        let Type::List(element) = list.ty else { unreachable!("a list") };
        let element = element.get();
        let value =
            if value.ty == Type::Int && element == Type::Float { widen(value, Type::Float) } else { value };
        if value.ty != element {
            self.diagnostics.push(
                Diagnostic::error(format!("a {} does not hold {}", list.ty, article(value.ty)))
                    .with_primary(value.span, format!("this is {}", article(value.ty)))
                    .with_note("Lion 0.1 does not define membership between values of different types"),
            );
            return None;
        }
        let kind =
            ir::ExprKind::Binary { op: ir::BinaryOp::InList, lhs: Box::new(value), rhs: Box::new(list) };
        Some(typed(kind, Type::Bool, span))
    }

    /// `sum(values)` of a List of Int or Float, or of a Range (§23).
    pub(crate) fn sum(&mut self, args: &[ast::Arg], span: Span) -> Option<ir::Expr> {
        let [arg] = args else {
            self.diagnostics.push(
                Diagnostic::error(format!("`sum` takes one list, not {} values", args.len()))
                    .with_primary(span, ""),
            );
            return None;
        };
        let values = self.expr(&arg.value)?;
        let ty = match values.ty.element() {
            Some(element @ (Type::Int | Type::Float)) => element,
            _ => {
                self.diagnostics.push(
                    Diagnostic::error(format!("`sum` adds a list of numbers, not {}", article(values.ty)))
                        .with_primary(values.span, ""),
                );
                return None;
            }
        };
        let kind = ir::ExprKind::CallBuiltin { builtin: ir::Builtin::Sum, args: vec![values] };
        Some(typed(kind, ty, span))
    }

    /// `l[i] = value` and `l.add(value)`: a change inside a `var` variable (§6.4).
    pub(crate) fn change_in_place(
        &mut self,
        target: &ast::Expr,
        value: &ast::Expr,
        adding: bool,
    ) -> Option<ir::Stmt> {
        let (root, indices, ty) = self.place_path(target)?;
        let expected = if adding {
            match ty {
                Type::List(element) => element.get(),
                other => {
                    self.diagnostics.push(
                        Diagnostic::error(format!("`add` adds to a List, not to {}", article(other)))
                            .with_primary(target.span, ""),
                    );
                    return None;
                }
            }
        } else {
            ty
        };
        let value = self.expr_expecting(value, expected)?;
        let context = (target.span, format!("this is {}", article(expected)));
        let value = self.coerce(value, expected, Some(context))?;
        Some(if adding {
            ir::Stmt::Add { root, indices, value }
        } else {
            ir::Stmt::AssignElement { root, indices, value }
        })
    }

    /// `l[i] += value`: the indices are evaluated once, into temporaries (§9.2).
    pub(crate) fn compound_element(
        &mut self,
        target: &ast::Expr,
        op: ast::BinaryOp,
        value: &ast::Expr,
        span: Span,
    ) -> Option<ir::Stmt> {
        let (root, indices, ty) = self.place_path(target)?;
        let rhs = self.expr(value)?;
        let mut stmts = Vec::new();
        let (mut current, mut current_ty) = match root {
            ir::Place::Local(local) => (ir::ExprKind::Local(local), self.ctx.locals[local.index()].ty?),
            ir::Place::Global(local) => {
                let GlobalType::Known(ty) = self.globals[&self.global_names[&local]].ty else { return None };
                (ir::ExprKind::Global(local), ty?)
            }
        };
        let mut stored = Vec::new();
        for index in indices {
            let temp = self.temporary(Type::Int, index.span);
            let index_span = index.span;
            stmts.push(ir::Stmt::Assign { place: ir::Place::Local(temp), value: index });
            let index = typed(ir::ExprKind::Local(temp), Type::Int, index_span);
            stored.push(index.clone());
            let Type::List(element) = current_ty else { unreachable!("checked by place_path") };
            let object = typed(current, current_ty, target.span);
            current = ir::ExprKind::Index { object: Box::new(object), index: Box::new(index) };
            current_ty = element.get();
        }
        let current = typed(current, ty, target.span);
        let result = self.arithmetic(op, span, current, rhs, span)?;
        if result.ty != ty && !(result.ty == Type::Int && ty == Type::Float) {
            self.diagnostics.push(
                Diagnostic::error("mismatched types")
                    .with_primary(span, format!("the result is {}", article(result.ty)))
                    .with_note(format!("expected: {ty}"))
                    .with_note(format!("found: {}", result.ty)),
            );
            return None;
        }
        stmts.push(ir::Stmt::AssignElement { root, indices: stored, value: widen(result, ty) });
        Some(ir::Stmt::Seq(stmts))
    }

    /// The variable and the indices that `l[i][j]` designates, and the type found there.
    fn place_path(&mut self, target: &ast::Expr) -> Option<(ir::Place, Vec<ir::Expr>, Type)> {
        match &target.kind {
            ast::ExprKind::Name(name) => self.changeable_variable(name, target.span),
            ast::ExprKind::Paren(inner) => self.place_path(inner),
            ast::ExprKind::Index { object, index } => {
                let (root, mut indices, ty) = self.place_path(object)?;
                let index = self.expr(index)?;
                let element = match (ty, index.ty) {
                    (Type::List(element), Type::Int) => element.get(),
                    (Type::Text, _) => {
                        self.diagnostics.push(
                            Diagnostic::error("a Text cannot be changed character by character")
                                .with_primary(target.span, "")
                                .with_help("build a new text with interpolation, and assign it"),
                        );
                        return None;
                    }
                    (Type::List(_), other) => {
                        self.diagnostics.push(
                            Diagnostic::error("the index of an element to change is an Int")
                                .with_primary(index.span, format!("this is {}", article(other))),
                        );
                        return None;
                    }
                    (other, _) => {
                        self.diagnostics.push(
                            Diagnostic::error(format!(
                                "{} has no elements to change",
                                capitalize(&article(other))
                            ))
                            .with_primary(target.span, ""),
                        );
                        return None;
                    }
                };
                indices.push(index);
                Some((root, indices, element))
            }
            _ => {
                self.diagnostics.push(
                    Diagnostic::error("only a variable, or an element of one, can be changed")
                        .with_primary(target.span, "")
                        .with_note("a temporary value would lose the change"),
                );
                None
            }
        }
    }

    /// A variable whose content changes: a `var` with a value (§6.4).
    fn changeable_variable(&mut self, name: &str, span: Span) -> Option<(ir::Place, Vec<ir::Expr>, Type)> {
        match self.resolve(name, span) {
            Resolved::Local(local) => {
                if !self.check_has_value(local, span) {
                    return None;
                }
                let info = &self.ctx.locals[local.index()];
                if !info.mutable {
                    let decl_span = info.decl_span;
                    self.diagnostics.push(
                        Diagnostic::error(format!("cannot change the content of the constant `{name}`"))
                            .with_primary(span, "")
                            .with_secondary(decl_span, "declared with `let`")
                            .with_note("a value held by `let` never changes, nor does its content (§6.4)")
                            .with_help(format!("to change it, declare it with `var`: `var {name} = ...`")),
                    );
                    return None;
                }
                let ty = info.ty?;
                self.ctx.flow.set(local, Assigned::Yes);
                Some((ir::Place::Local(local), Vec::new(), ty))
            }
            Resolved::Global(local) => {
                if !self.check_global_assignment(local, span) {
                    return None;
                }
                self.ctx.reads.push(local);
                let GlobalType::Known(ty) = self.globals[&self.global_names[&local]].ty else { return None };
                Some((ir::Place::Global(local), Vec::new(), ty?))
            }
            Resolved::Function(_) | Resolved::Standard(_) => {
                self.diagnostics.push(
                    Diagnostic::error(format!("`{name}` is a function, not a variable"))
                        .with_primary(span, ""),
                );
                None
            }
            Resolved::Nothing => None,
        }
    }
}

/// Converts an Int to Float where a Float is expected (§8.5).
pub(crate) fn widen(value: ir::Expr, ty: Type) -> ir::Expr {
    if value.ty == Type::Int && ty == Type::Float {
        let span = value.span;
        typed(
            ir::ExprKind::Convert { conversion: ir::Conversion::IntToFloat, value: Box::new(value) },
            Type::Float,
            span,
        )
    } else {
        value
    }
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}
