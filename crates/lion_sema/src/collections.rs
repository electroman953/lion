//! Lists and texts as sequences (spec §16): literals, comprehensions, indices,
//! extracts, properties and changes in place.

use std::collections::HashSet;

use lion_diagnostics::{Diagnostic, Span};
use lion_ir::{self as ir, Type};
use lion_syntax::ast;

use crate::{Checker, Scope, article, capitalize, typed};

/// The two collections written with elements: `[...]` and `{...}` (§16.1).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Collection {
    List,
    Set,
}

impl Collection {
    fn ty(self, element: Type) -> Type {
        match self {
            Collection::List => Type::list(element),
            Collection::Set => Type::set(element),
        }
    }

    fn build(self, values: Vec<ir::Expr>, element: Type, span: Span) -> ir::Expr {
        match self {
            Collection::List => typed(ir::ExprKind::List(values), Type::list(element), span),
            Collection::Set => typed(ir::ExprKind::Set(values), Type::set(element), span),
        }
    }
}

/// The type of the elements of a collection type.
fn output_type(collection: Type) -> Type {
    collection.element().expect("a collection has elements")
}

/// One element of a comprehension after the output (§16.4).
enum Part {
    Generator { var: ir::LocalId, iterable: ir::Expr },
    Condition(ir::Expr),
}

impl Checker<'_> {
    /// `[a, b, c]`, or a comprehension when an element is `v in X` with a new `v`.
    pub(crate) fn list(&mut self, elements: &[ast::Expr], span: Span) -> Option<ir::Expr> {
        self.collection(Collection::List, elements, span)
    }

    /// `[a, b]` or `{a, b}`, or a comprehension (§16.1, §16.4).
    pub(crate) fn collection(
        &mut self,
        kind: Collection,
        elements: &[ast::Expr],
        span: Span,
    ) -> Option<ir::Expr> {
        let generators = self.generators(elements);
        if generators.iter().any(|&is_generator| is_generator) {
            return self.comprehension(kind, elements, &generators, span);
        }
        if elements.is_empty() {
            let (what, example) = match kind {
                Collection::List => ("list", "var l = [] in List of Int`, or `[] as List of Int"),
                Collection::Set => ("set", "var s = {} in Set of Int`, or `{} as Set of Int"),
            };
            self.diagnostics.push(
                Diagnostic::error(format!("the type of this empty {what} is not known"))
                    .with_primary(span, "")
                    .with_help(format!("write its type: `{example}`")),
            );
            return None;
        }
        // An empty list among the elements takes the type of the others: `[[1], []]`.
        let is_empty =
            |element: &ast::Expr| matches!(&element.kind, ast::ExprKind::List(inner) if inner.is_empty());
        let checked: Vec<Option<ir::Expr>> =
            elements.iter().filter(|element| !is_empty(element)).map(|element| self.expr(element)).collect();
        let checked: Vec<ir::Expr> = checked.into_iter().collect::<Option<_>>()?;
        if checked.is_empty() {
            self.diagnostics.push(
                Diagnostic::error("the type of these empty lists is not known")
                    .with_primary(span, "")
                    .with_help("write the type: `[] as List of List of Int`"),
            );
            return None;
        }
        let element = self.common_type(&checked, span, "the elements of a list")?;
        let mut checked = checked.into_iter();
        let mut values = Vec::new();
        for element_ast in elements {
            if is_empty(element_ast) {
                values.push(self.empty_list(element_ast, element).or_else(|| {
                    self.diagnostics.push(
                        Diagnostic::error(format!("an empty list is not {}", article(element)))
                            .with_primary(element_ast.span, ""),
                    );
                    None
                })?);
            } else {
                values.push(widen(checked.next().expect("one value per element"), element));
            }
        }
        Some(kind.build(values, element, span))
    }

    /// An empty list takes the type expected where it is written.
    pub(crate) fn empty_list(&mut self, expr: &ast::Expr, expected: Type) -> Option<ir::Expr> {
        let member = |pick: fn(&Type) -> bool| expected.members().into_iter().find(pick);
        match &expr.kind {
            ast::ExprKind::List(elements) if elements.is_empty() => {
                let ty = member(|ty| matches!(ty, Type::List(_)))?;
                Some(typed(ir::ExprKind::List(Vec::new()), ty, expr.span))
            }
            ast::ExprKind::Set(elements) if elements.is_empty() => {
                let ty = member(|ty| matches!(ty, Type::Set(_)))?;
                Some(typed(ir::ExprKind::Set(Vec::new()), ty, expr.span))
            }
            ast::ExprKind::Paren(inner) => self.empty_list(inner, expected),
            _ => None,
        }
    }

    /// Checks an expression where a value of type `expected` is required.
    pub(crate) fn expr_expecting(&mut self, expr: &ast::Expr, expected: Type) -> Option<ir::Expr> {
        if let Some(empty) = self.empty_list(expr, expected) {
            return Some(empty);
        }
        if let Some(value) = self.expected_enum_value(expr, expected) {
            return Some(value);
        }
        match &expr.kind {
            ast::ExprKind::Paren(inner) => {
                self.expr_expecting(inner, expected).map(|value| ir::Expr { span: expr.span, ..value })
            }
            ast::ExprKind::If { branches, otherwise } => {
                self.if_expr(branches, otherwise.as_deref(), expr.span, Some(expected))
            }
            // A function value takes the types of the function type expected (§11).
            ast::ExprKind::Fun(decl) => self.anonymous_function(decl, expr.span, Some(expected)),
            ast::ExprKind::Name(name)
                if matches!(expected, Type::Fun(_)) && !self.is_variable(name) && self.is_function(name) =>
            {
                match self.resolve(name, expr.span) {
                    crate::names::Resolved::Function(index) => {
                        self.function_value(index, expr.span, Some(expected))
                    }
                    _ => self.expr(expr),
                }
            }
            // The elements of a list expect the type of the elements: `[red, blue] in List of Color`.
            ast::ExprKind::List(elements) | ast::ExprKind::Set(elements)
                if !elements.is_empty() && !self.generators(elements).contains(&true) =>
            {
                let kind = if matches!(expr.kind, ast::ExprKind::List(_)) {
                    Collection::List
                } else {
                    Collection::Set
                };
                let Some(element) = expected.members().into_iter().find_map(|member| match (kind, member) {
                    (Collection::List, Type::List(element)) | (Collection::Set, Type::Set(element)) => {
                        Some(element.get())
                    }
                    _ => None,
                }) else {
                    return self.expr(expr);
                };
                let values: Vec<Option<ir::Expr>> = elements
                    .iter()
                    .map(|value| {
                        let checked = self.expr_expecting(value, element)?;
                        self.coerce(checked, element, None)
                    })
                    .collect();
                let values = values.into_iter().collect::<Option<Vec<_>>>()?;
                Some(kind.build(values, element, expr.span))
            }
            _ => self.expr(expr),
        }
    }

    /// A name that designates nothing here: maybe a value of an enumeration, whose type
    /// the context gives (D32).
    pub(crate) fn is_bare_unknown_name(&self, expr: &ast::Expr) -> bool {
        match &expr.kind {
            ast::ExprKind::Name(name) => !self.is_variable(name) && !self.enums_with_value(name).is_empty(),
            ast::ExprKind::Paren(inner) => self.is_bare_unknown_name(inner),
            _ => false,
        }
    }

    /// One type for several values: the same; Float when only Int and Float are mixed
    /// (§8.5); otherwise their union, such as `Int or Text` (§7.3).
    pub(crate) fn common_type(&mut self, values: &[ir::Expr], span: Span, what: &str) -> Option<Type> {
        let first = values[0].ty;
        if values.iter().all(|value| value.ty == first) {
            return Some(first);
        }
        if values.iter().all(|value| matches!(value.ty, Type::Int | Type::Float)) {
            return Some(Type::Float);
        }
        let union = Type::union(values.iter().map(|value| value.ty));
        let lists = union.members().into_iter().filter(|member| matches!(member, Type::List(_))).count();
        if lists > 1 {
            self.not_implemented(span, &format!("{what} that are lists of different types"), "§7.3");
            return None;
        }
        Some(union)
    }

    /// Which elements are generators: `v in X` where `v` is new at that point (§16.4).
    fn generators(&self, elements: &[ast::Expr]) -> Vec<bool> {
        let mut new_names = HashSet::new();
        elements
            .iter()
            .map(|element| match &element.kind {
                ast::ExprKind::Binary { op: ast::BinaryOp::In, lhs, .. }
                | ast::ExprKind::TypeTest { value: lhs, .. } => match &lhs.kind {
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
    fn comprehension(
        &mut self,
        kind: Collection,
        elements: &[ast::Expr],
        generators: &[bool],
        span: Span,
    ) -> Option<ir::Expr> {
        let implicit_output = generators[0];
        if implicit_output && generators.iter().filter(|&&is_generator| is_generator).count() > 1 {
            let example = match kind {
                Collection::List => "[(a, b), a in A, b in B]",
                Collection::Set => "{(a, b), a in A, b in B}",
            };
            self.diagnostics.push(
                Diagnostic::error("with several generators, a comprehension starts with its result")
                    .with_primary(elements[0].span, "")
                    .with_help(format!("write the result first: `{example}` (§16.4)")),
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
                None => {
                    // What follows may use the variable of a failed generator.
                    valid = false;
                    break;
                }
            }
        }
        let output = if !valid {
            None
        } else if implicit_output {
            first_var.map(|var| {
                let ty = self.ctx.locals[var.index()].ty.unwrap_or(Type::None);
                typed(ir::ExprKind::Local(var), ty, elements[0].span)
            })
        } else {
            self.expr(&elements[0])
        };
        self.ctx.scopes.pop();
        let output = output.filter(|_| valid)?;
        let list_type = kind.ty(output.ty);
        let result = self.temporary(list_type, span);
        let mut body =
            vec![ir::Stmt::Add { root: ir::Place::Local(result), path: Vec::new(), value: output }];
        for part in parts.into_iter().rev() {
            body = vec![match part {
                Part::Generator { var, iterable } => ir::Stmt::For { var, iterable, body },
                Part::Condition(cond) => ir::Stmt::If { cond, then: body, otherwise: Vec::new() },
            }];
        }
        let empty = kind.build(Vec::new(), output_type(list_type), span);
        let mut stmts = vec![ir::Stmt::Assign { place: ir::Place::Local(result), value: empty }];
        stmts.extend(body);
        let value = typed(ir::ExprKind::Local(result), list_type, span);
        Some(typed(ir::ExprKind::Block { stmts, value: Box::new(value) }, list_type, span))
    }

    fn generator(&mut self, element: &ast::Expr) -> Option<Part> {
        let (lhs, iterable) = match &element.kind {
            ast::ExprKind::Binary { lhs, rhs, .. } => (lhs, self.expr(rhs)),
            // `d in Days` goes through an enumeration; `x in Int` would make a Domain.
            ast::ExprKind::TypeTest { value, ty } => {
                let values = match &ty.kind {
                    ast::TypeExprKind::Named { module, name, args }
                        if module.is_empty()
                            && args.is_empty()
                            && self.tables.named_types.contains_key(&name.name) =>
                    {
                        self.enum_values(&name.name, ty.span)
                    }
                    _ => None,
                };
                if values.is_none() {
                    self.not_implemented(
                        element.span,
                        "comprehensions over a type, which make a `Domain`",
                        "§16.5",
                    );
                    return None;
                }
                (value, values)
            }
            _ => unreachable!("a generator is `v in X`"),
        };
        let ast::ExprKind::Name(name) = &lhs.kind else { unreachable!("a generator is `v in X`") };
        let rhs = element;
        let iterable = iterable?;
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

    /// `l.size`, `l.first`, `l.last`, `t.size`, `r.size` (§16.2, D38), and the fields of
    /// structures (§12.1).
    pub(crate) fn property(&mut self, object: &ast::Expr, name: &ast::Ident, span: Span) -> Option<ir::Expr> {
        let object = self.expr(object)?;
        let object = self.within_try(object);
        if let Some(method) = self.visible_method(object.ty, &name.name) {
            return self.detached_method(method, object, name, span);
        }
        if object.ty.members().iter().any(|member| matches!(member, Type::Struct(_))) {
            let (field, ty) = self.field_of(object.ty, name, object.span)?;
            return Some(typed(ir::ExprKind::Field { object: Box::new(object), field }, ty, span));
        }
        let (property, ty) = match (object.ty, name.name.as_str()) {
            (Type::List(_) | Type::Set(_) | Type::Text | Type::Range, "size") => {
                (ir::Property::Size, Type::Int)
            }
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
        let (Type::List(element) | Type::Set(element)) = list.ty else { unreachable!("a list or a set") };
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
        let op = if matches!(list.ty, Type::Set(_)) { ir::BinaryOp::InSet } else { ir::BinaryOp::InList };
        let kind = ir::ExprKind::Binary { op, lhs: Box::new(value), rhs: Box::new(list) };
        Some(typed(kind, Type::Bool, span))
    }

    /// `sum(values)` of a List of numbers, or of a Range (§23).
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
            Some(element @ (Type::Int | Type::Float | Type::Rational)) => element,
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

    /// `l[i] = value`, `s.grade = value` and `l.add(value)`: a change inside a `var`
    /// variable (§6.4), then the check of the structures that contain it (§12.3).
    pub(crate) fn change_in_place(
        &mut self,
        target: &ast::Expr,
        value: &ast::Expr,
        adding: bool,
    ) -> Option<ir::Stmt> {
        let path = self.place_path(target)?;
        let ty = path.ty();
        let expected = if adding {
            match ty {
                Type::List(element) | Type::Set(element) => element.get(),
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
        let (mut stmts, path) = self.stabilize(path);
        let (root, steps) = (path.root, path.steps.clone());
        stmts.push(if adding {
            ir::Stmt::Add { root, path: steps, value }
        } else {
            ir::Stmt::AssignElement { root, path: steps, value }
        });
        // `add` changes the list at the end of the path, which is part of the structures.
        let checks = self.check_invariants(&path, adding);
        stmts.extend(checks);
        Some(if stmts.len() == 1 { stmts.pop().expect("one statement") } else { ir::Stmt::Seq(stmts) })
    }

    /// `l[i] += value`, `s.grade += value`: the indices are evaluated once, into
    /// temporaries (§9.2).
    pub(crate) fn compound_element(
        &mut self,
        target: &ast::Expr,
        op: ast::BinaryOp,
        value: &ast::Expr,
        span: Span,
    ) -> Option<ir::Stmt> {
        let path = self.place_path(target)?;
        let ty = path.ty();
        let rhs = self.expr(value)?;
        let (mut stmts, path) = self.stabilize(path);
        let current = self.read_path(&path, path.steps.len());
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
        let value = widen(result, ty);
        stmts.push(ir::Stmt::AssignElement { root: path.root, path: path.steps.clone(), value });
        let checks = self.check_invariants(&path, false);
        stmts.extend(checks);
        Some(ir::Stmt::Seq(stmts))
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
