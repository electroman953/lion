//! Functions as values (spec §11): the functions of the file used as values, the
//! functions declared inside a body, anonymous functions, and calls of function values.
//!
//! A function declared inside a body is a *closure*: it reads a copy of the local
//! variables it uses, taken where it is created; the variables it names after
//! `modifies` are shared with it instead, in a cell that lives as long as the function
//! (§11.5, D71). The captured variables follow its parameters.

use std::collections::HashSet;
use std::rc::Rc;

use lion_diagnostics::{Diagnostic, Span};
use lion_ir::{self as ir, Type};
use lion_syntax::ast;

use crate::{Checker, ContextKind, article, typed};

/// The variables that a function declared inside a body captures.
#[derive(Clone, Default)]
pub(crate) struct ClosureInfo {
    pub(crate) captures: Vec<Capture>,
}

#[derive(Clone)]
pub(crate) struct Capture {
    pub(crate) name: String,
    pub(crate) ty: Type,
    /// Named after `modifies`: the function shares the variable (§11.5).
    pub(crate) by_reference: bool,
    /// The variable names a shared object, and so does the captured one (§17.2).
    pub(crate) sharing: Option<crate::sharing::Sharing>,
    pub(crate) span: Span,
}

impl Checker<'_> {
    /// `fun name(...)` inside a body: a local constant that holds the function (§11.5).
    pub(crate) fn local_function(&mut self, decl: &ast::FunDecl, span: Span) -> Option<ir::Stmt> {
        if let Some(receiver) = &decl.receiver {
            self.diagnostics.push(
                Diagnostic::error("a method is declared at the top level of a file")
                    .with_primary(receiver.span.to(decl.name.span), "")
                    .with_help("move this declaration out of the block (§12.4)"),
            );
            return None;
        }
        let value = self.closure(decl, span, None)?;
        let ty = value.ty;
        let local = self.declare(&decl.name, Some(ty), false, true);
        Some(ir::Stmt::Assign { place: ir::Place::Local(local), value })
    }

    /// `fun(x in Int) = x * 2`: an anonymous function (§11.1). Without the types of its
    /// parameters, it takes those of the function type expected there.
    pub(crate) fn anonymous_function(
        &mut self,
        decl: &ast::FunDecl,
        span: Span,
        expected: Option<Type>,
    ) -> Option<ir::Expr> {
        let expected = expected.and_then(|ty| {
            ty.members().into_iter().find_map(|member| match member {
                Type::Fun(function) => Some(function.get()),
                _ => None,
            })
        });
        self.closure(decl, span, expected)
    }

    /// Registers and checks a function declared inside a body, and gives the value that
    /// holds it with the variables it captures.
    pub(crate) fn closure(
        &mut self,
        decl: &ast::FunDecl,
        span: Span,
        expected: Option<ir::FunData>,
    ) -> Option<ir::Expr> {
        if let Some(default) = decl.params.iter().find_map(|param| param.default.as_ref()) {
            self.diagnostics.push(
                Diagnostic::error("a function declared inside a body has no default values")
                    .with_primary(default.span, "")
                    .with_note("give every argument at the call (C65)"),
            );
            return None;
        }
        if let Some(var) = decl.params.iter().find_map(|param| param.var) {
            self.diagnostics.push(
                Diagnostic::error("a function declared inside a body has no `var` parameters")
                    .with_primary(var, "")
                    .with_help("use `modifies` to change a variable around it (§11.5, C65)"),
            );
            return None;
        }
        if let Some(expected) = &expected
            && expected.params.len() != decl.params.len()
        {
            self.diagnostics.push(
                Diagnostic::error(format!(
                    "this function takes {} values, but {} is expected here",
                    decl.params.len(),
                    Type::function(expected.params.clone(), expected.required as usize, expected.ret)
                ))
                .with_primary(decl.name.span.to(span), ""),
            );
            return None;
        }
        let captures = self.captures(decl)?;
        let index = self.functions.len();
        self.register_nested_function(Rc::new(decl.clone()), captures.clone(), expected.clone());
        self.resolve_signature(index);
        let params = self.functions[index].signature.clone()?;
        if let Some(param) = params.iter().find(|param| param.ty.is_none()) {
            self.diagnostics.push(
                Diagnostic::error("the type of this parameter must be written")
                    .with_primary(param.span, "")
                    .with_note("a function declared inside a body, or used as a value, is not generic (C65)")
                    .with_help(format!("write it: `{} in Int`", param.name)),
            );
            return None;
        }
        let ret = self.functions[index]
            .declared_ret
            .map_or(crate::functions::Ret::Unknown, crate::functions::Ret::Declared);
        let instance = self.new_instance(index, Vec::new(), None, ret);
        self.functions[index].instance = Some(instance);
        // The body is checked now, where the captured variables have their values.
        let ret = self.return_type_of(instance, decl.name.span)?;
        self.ctx.calls.push(instance);
        self.record_early_call(instance, span);
        let param_types: Vec<Type> = params.iter().map(|param| param.ty.expect("checked above")).collect();
        let required = param_types.len();
        let ty = Type::function(param_types, required, ret);
        let values = captures
            .iter()
            .map(|capture| {
                let outer = self.lookup(&capture.name).expect("a captured variable is visible");
                let kind =
                    if capture.by_reference { ir::ExprKind::Cell(outer) } else { ir::ExprKind::Local(outer) };
                typed(kind, capture.ty, capture.span)
            })
            .collect();
        let function = ir::FunctionId(instance as u32);
        Some(typed(ir::ExprKind::Closure { function, captures: values }, ty, span))
    }

    /// The local variables of the enclosing code that the body of `decl` uses, in the
    /// order of their first use: a copy of each, or the variable itself with `modifies`.
    fn captures(&mut self, decl: &ast::FunDecl) -> Option<Vec<Capture>> {
        let params: HashSet<&str> = decl.params.iter().map(|param| param.name.name.as_str()).collect();
        let mut names = Vec::new();
        free_names_of_function(decl, &mut names);
        let mut captures = Vec::new();
        let mut valid = true;
        let mut seen = HashSet::new();
        let modified: Vec<&ast::Ident> = decl.modifies.iter().collect();
        for (name, span) in names {
            if params.contains(name.as_str()) || name == decl.name.name || !seen.insert(name.clone()) {
                continue;
            }
            let Some(outer) = self.lookup(&name) else { continue };
            // At the top level of the script, a function reads the globals as they are
            // when it runs (D71).
            if self.ctx.kind == ContextKind::Script
                && self.tables.globals.get(&name).is_some_and(|global| global.local == Some(outer))
            {
                continue;
            }
            let by_reference = modified.iter().any(|modified| modified.name == name);
            let info = &self.ctx.locals[outer.index()];
            if by_reference && (!info.mutable || info.by_reference || info.captured && !info.boxed) {
                let decl_span = info.decl_span;
                self.diagnostics.push(
                    Diagnostic::error(format!("`{name}` cannot be shared with the function"))
                        .with_primary(span, "")
                        .with_secondary(decl_span, "declared here")
                        .with_note("a function modifies a `var` variable declared around it, not a constant nor a parameter (§11.5, C65)"),
                );
                valid = false;
                continue;
            }
            if !by_reference && !self.check_has_value(outer, span) {
                valid = false;
                continue;
            }
            let ty = if by_reference { self.ctx.locals[outer.index()].ty } else { self.local_type(outer) };
            let Some(ty) = ty else {
                valid = false;
                continue;
            };
            if by_reference {
                self.ctx.locals[outer.index()].boxed = true;
            }
            // A copy of a `var shared` is a frozen copy, which is not shared (§17.2).
            let info = &self.ctx.locals[outer.index()];
            let sharing = info.shared.filter(|_| by_reference || !info.mutable);
            captures.push(Capture { name, ty, by_reference, sharing, span });
        }
        valid.then_some(captures)
    }

    /// `f` used as a value, for a function of the file (§11).
    pub(crate) fn function_value(
        &mut self,
        index: usize,
        span: Span,
        expected: Option<Type>,
    ) -> Option<ir::Expr> {
        let params = self.functions[index].signature.clone()?;
        let name = self.functions[index].decl.name.name.clone();
        if self.functions[index].closure.is_some() {
            self.not_implemented(
                span,
                "a function declared inside a body, used as a value in its own body",
                "§11.5",
            );
            return None;
        }
        if params.iter().any(|param| param.by_reference) {
            self.diagnostics.push(
                Diagnostic::error(format!("`{name}` has `var` parameters: it cannot be used as a value"))
                    .with_primary(span, "")
                    .with_note("the variables of a call are known only where the function is called by its name (C65)"),
            );
            return None;
        }
        let instance = match self.functions[index].instance {
            Some(instance) => instance,
            None => {
                // A generic function takes the types that the value is expected to have.
                let expected = expected.and_then(|ty| match ty {
                    Type::Fun(function) => Some(function.get()),
                    _ => None,
                });
                let Some(expected) = expected.filter(|expected| expected.params.len() == params.len()) else {
                    self.diagnostics.push(
                        Diagnostic::error(format!(
                            "the types of the parameters of `{name}` are not known here"
                        ))
                        .with_primary(span, "")
                        .with_note(format!("`{name}` is generic: it has parameters without a type (§15.3)"))
                        .with_help(format!(
                            "give the value a function type, as `let f = {name} in fun(Int) in Int`"
                        )),
                    );
                    return None;
                };
                self.instantiate(index, expected.params, span)?
            }
        };
        let ret = self.return_type_of(instance, span)?;
        self.ctx.calls.push(instance);
        self.record_early_call(instance, span);
        let types = self.instance_param_types(instance, &params);
        let required = params.iter().filter(|param| !param.has_default).count();
        let ty = Type::function(types, required, ret);
        let function = ir::FunctionId(instance as u32);
        Some(typed(ir::ExprKind::Closure { function, captures: Vec::new() }, ty, span))
    }

    /// `f(1)` with fewer arguments than `f` requires: the function that waits for the
    /// others, and runs as soon as they are all given (§11.3).
    pub(crate) fn partial(
        &mut self,
        callee: ir::Expr,
        data: &ir::FunData,
        args: &[ast::Arg],
        span: Span,
    ) -> Option<ir::Expr> {
        let mut values = Vec::new();
        let mut valid = true;
        for (arg, &ty) in args.iter().zip(&data.params) {
            if let Some(span) = arg.var_marker {
                self.diagnostics.push(Diagnostic::error("`var` is not written here").with_primary(span, ""));
                valid = false;
            }
            match self.expr_expecting(&arg.value, ty).and_then(|value| self.coerce(value, ty, None)) {
                Some(value) => values.push(value),
                None => valid = false,
            }
        }
        if !valid {
            return None;
        }
        let given = values.len();
        let ty = Type::function(data.params[given..].to_vec(), data.required as usize - given, data.ret);
        Some(typed(ir::ExprKind::Partial { callee: Box::new(callee), args: values }, ty, span))
    }

    /// `s.passes`: a function that reads a copy of `s` (§12.6).
    pub(crate) fn detached_method(
        &mut self,
        method: usize,
        object: ir::Expr,
        object_ast: &ast::Expr,
        name: &ast::Ident,
        span: Span,
    ) -> Option<ir::Expr> {
        if self.functions[method].var_self {
            // `score.increment`, `score` a `var shared`: the function changes that object.
            let sharing = self.sharing_of_expr(object_ast);
            if let Some((_, true)) = sharing {
                return self.detached_shared_method(method, object_ast, name, span);
            }
            let note = match sharing {
                Some(_) => "a `let shared` object never changes (§17.2)",
                None => {
                    "a detached method reads a copy of its object; only a `var shared` object is changed through it (§12.6)"
                }
            };
            self.diagnostics.push(
                Diagnostic::error(format!(
                    "`{}` changes its object: it cannot be detached from it",
                    name.name
                ))
                .with_primary(span, "")
                .with_note(note)
                .with_help(format!("call it: `.{}(...)`", name.name)),
            );
            return None;
        }
        let value = self.function_value(method, name.span, None)?;
        let Type::Fun(function) = value.ty else { unreachable!("a function value") };
        let data = function.get();
        let ty = Type::function(data.params[1..].to_vec(), data.required as usize - 1, data.ret);
        Some(typed(ir::ExprKind::Partial { callee: Box::new(value), args: vec![object] }, ty, span))
    }

    /// `f(args)` where `f` is a function value (§11.2). Its parameters have no names.
    pub(crate) fn call_value(&mut self, callee: ir::Expr, args: &[ast::Arg], span: Span) -> Option<ir::Expr> {
        let Type::Fun(function) = callee.ty else {
            let mut error =
                Diagnostic::error(format!("{} is not a function", crate::capitalize(&article(callee.ty))))
                    .with_primary(callee.span, format!("this is {}", article(callee.ty)));
            if let Some(help) = crate::expr::union_help(callee.ty) {
                error = error.with_help(help);
            }
            self.diagnostics.push(error);
            return None;
        };
        let data = function.get();
        let mut valid = true;
        for arg in args {
            if let Some(span) = arg.var_marker.or(arg.name.as_ref().map(|name| name.span)) {
                self.diagnostics.push(
                    Diagnostic::error("the arguments of a function value are written alone")
                        .with_primary(span, "")
                        .with_note("the parameters of a function type have no names, nor `var` (§7.2)"),
                );
                valid = false;
            }
        }
        if args.len() > data.params.len() || args.len() < data.required as usize {
            let expected = if data.required as usize == data.params.len() {
                format!("{}", data.required)
            } else {
                format!("{} to {}", data.required, data.params.len())
            };
            if args.len() < data.required as usize && !args.is_empty() {
                return self.partial(callee, &data, args, span);
            } else {
                let plural = if data.params.len() == 1 { "" } else { "s" };
                self.diagnostics.push(
                    Diagnostic::error(format!(
                        "this function takes {expected} argument{plural}, not {}",
                        args.len()
                    ))
                    .with_primary(span, "")
                    .with_note(format!("it is {}", article(callee.ty))),
                );
            }
            return None;
        }
        let mut values = Vec::new();
        for (arg, &ty) in args.iter().zip(&data.params) {
            let value = self.expr_expecting(&arg.value, ty).and_then(|value| self.coerce(value, ty, None));
            match value {
                Some(value) => values.push(value),
                None => valid = false,
            }
        }
        if !valid {
            return None;
        }
        if self.ctx.parallel.is_some() {
            self.diagnostics.push(
                Diagnostic::error("a function value cannot be called in a parallel part")
                    .with_primary(span, "")
                    .with_note("what it changes is not known where it is called (§19.3, C65)"),
            );
            return None;
        }
        // The function may change the variables that it shares with this code.
        let shared: Vec<ir::LocalId> = (0..self.ctx.locals.len() as u32)
            .map(ir::LocalId)
            .filter(|local| self.ctx.locals[local.index()].boxed)
            .collect();
        for local in shared {
            self.ctx.flow.narrow(local, None);
        }
        let kind = ir::ExprKind::CallValue { callee: Box::new(callee), args: values };
        Some(typed(kind, data.ret, span))
    }
}

/// The names that the body of a function reads or assigns, with where, in order.
fn free_names_of_function(decl: &ast::FunDecl, names: &mut Vec<(String, Span)>) {
    for param in &decl.params {
        if let Some(default) = &param.default {
            names_in_expr(default, names);
        }
    }
    match &decl.body {
        ast::FunBody::Block(block) => names_in_stmts(&block.stmts, names),
        ast::FunBody::Expr(value) => names_in_expr(value, names),
        ast::FunBody::Foreign | ast::FunBody::Required => {}
    }
    for name in &decl.modifies {
        names.push((name.name.clone(), name.span));
    }
}

fn names_in_stmts(stmts: &[ast::Stmt], names: &mut Vec<(String, Span)>) {
    for stmt in stmts {
        match &stmt.kind {
            ast::StmtKind::Let(decl) => {
                if let Some(value) = &decl.value {
                    names_in_expr(value, names);
                }
            }
            ast::StmtKind::Assign { target, value, .. } => {
                names_in_expr(target, names);
                names_in_expr(value, names);
            }
            ast::StmtKind::Expr(expr) => names_in_expr(expr, names),
            ast::StmtKind::If { branches, otherwise } => {
                for branch in branches {
                    names_in_expr(&branch.cond, names);
                    names_in_stmts(&branch.body.stmts, names);
                }
                if let Some(otherwise) = otherwise {
                    names_in_stmts(&otherwise.stmts, names);
                }
            }
            ast::StmtKind::While { cond, body } => {
                names_in_expr(cond, names);
                names_in_stmts(&body.stmts, names);
            }
            ast::StmtKind::For { iterable, body, .. } => {
                names_in_expr(iterable, names);
                names_in_stmts(&body.stmts, names);
            }
            ast::StmtKind::Return(Some(value)) => names_in_expr(value, names),
            ast::StmtKind::Fun(decl) => free_names_of_function(decl, names),
            ast::StmtKind::Match { scrutinee, cases } => {
                names_in_expr(scrutinee, names);
                for (case, body) in cases {
                    names_in_case(case, names);
                    names_in_stmts(&body.stmts, names);
                }
            }
            ast::StmtKind::Return(None)
            | ast::StmtKind::Break
            | ast::StmtKind::Continue
            | ast::StmtKind::Struct(_)
            | ast::StmtKind::TypeDef(_)
            | ast::StmtKind::Trait(_)
            | ast::StmtKind::Test { .. }
            | ast::StmtKind::Use(_) => {}
            ast::StmtKind::Expect(condition) => names_in_expr(&condition.expr, names),
        }
    }
}

fn names_in_case(case: &ast::Case, names: &mut Vec<(String, Span)>) {
    match &case.pattern {
        ast::Pattern::Value(value) => names_in_expr(value, names),
        ast::Pattern::In { set, .. } => names_in_expr(set, names),
        ast::Pattern::Type { .. } | ast::Pattern::Otherwise => {}
    }
    for condition in &case.conditions {
        names_in_expr(condition, names);
    }
}

pub(crate) fn names_in_expr(expr: &ast::Expr, names: &mut Vec<(String, Span)>) {
    use ast::ExprKind::*;
    match &expr.kind {
        Name(name) => names.push((name.clone(), expr.span)),
        Int(_) | Float(_) | Bool(_) | None | TypeName(_) => {}
        Text(parts) => {
            for part in parts {
                if let ast::TextPart::Interpolation(value) = part {
                    names_in_expr(value, names);
                }
            }
        }
        Paren(inner) | Try(inner) | Parallel(inner) | Task(inner) | Wait(inner) => {
            names_in_expr(inner, names)
        }
        Shared { value, .. } | Compile(value) => names_in_expr(value, names),
        List(elements) | Set(elements) => elements.iter().for_each(|element| names_in_expr(element, names)),
        Tuple(elements) => elements.iter().for_each(|element| names_in_expr(&element.value, names)),
        Unary { operand, .. } => names_in_expr(operand, names),
        Binary { lhs, rhs, .. } => {
            names_in_expr(lhs, names);
            names_in_expr(rhs, names);
        }
        Compare { first, rest } => {
            names_in_expr(first, names);
            rest.iter().for_each(|comparison| names_in_expr(&comparison.rhs, names));
        }
        As { value, .. } | TypeTest { value, .. } => names_in_expr(value, names),
        Call { callee, args } => {
            names_in_expr(callee, names);
            args.iter().for_each(|arg| names_in_expr(&arg.value, names));
        }
        Field { object, .. } => names_in_expr(object, names),
        Index { object, index } => {
            names_in_expr(object, names);
            names_in_expr(index, names);
        }
        Match { scrutinee, cases } => {
            names_in_expr(scrutinee, names);
            for (case, value) in cases {
                names_in_case(case, names);
                names_in_expr(value, names);
            }
        }
        If { branches, otherwise } => {
            for (cond, value) in branches {
                names_in_expr(cond, names);
                names_in_expr(value, names);
            }
            if let Some(otherwise) = otherwise {
                names_in_expr(otherwise, names);
            }
        }
        Fun(decl) => free_names_of_function(decl, names),
    }
}
