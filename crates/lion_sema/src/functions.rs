//! Functions (spec §11): declarations, bodies, calls, and the globals they reach.
//!
//! Top-level functions are registered before the script is checked, so they can be
//! called before their declaration (C4). Each body records the globals it reads and
//! the functions it calls; once every body is known, the reads are closed over the
//! calls, and each call made by the script is checked: the function must not read a
//! global that may have no value at that point (§6.1, C3).

use std::collections::HashSet;

use lion_diagnostics::{Diagnostic, Span};
use lion_ir::{self as ir, Type};
use lion_syntax::ast;

use crate::flow::Assigned;
use crate::names::Resolved;
use crate::{Checker, Context, ContextKind, GlobalInfo, GlobalType, LocalInfo, article, ir_locals, typed};

pub(crate) struct FunctionInfo<'a> {
    pub(crate) decl: &'a ast::FunDecl,
    /// `None` when the declaration has an error or needs what is not implemented.
    signature: Option<Vec<ParamInfo>>,
    ret: Ret,
    state: BodyState,
    /// The globals named after `modifies` (§11.5).
    modifies: Vec<ir::LocalId>,
    /// Known once the body is checked.
    reads: Vec<ir::LocalId>,
    calls: Vec<usize>,
    checked: Option<CheckedBody>,
}

#[derive(Clone)]
struct ParamInfo {
    name: String,
    ty: Type,
    by_reference: bool,
    has_default: bool,
    span: Span,
}

#[derive(Clone, Copy)]
enum Ret {
    Declared(Type),
    /// Not written: inferred from the body (C12).
    Unknown,
    /// The body is being checked to find it.
    Inferring,
    Inferred(Type),
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BodyState {
    Unchecked,
    Checking,
    Checked,
}

struct CheckedBody {
    locals: Vec<LocalInfo>,
    body: Vec<ir::Stmt>,
    defaults: Vec<(u32, ir::Expr)>,
}

/// A call made by the script.
pub(crate) struct ScriptCall {
    function: usize,
    span: Span,
    /// The globals that may have no value at the call.
    unassigned: Vec<ir::LocalId>,
}

/// An argument once checked, before the call is built.
enum Pending {
    Value(ir::Expr),
    /// A variable given to a `var` parameter.
    Place(ir::Place),
    /// Any other value given to a `var` parameter: the modification is lost (§11.2).
    Temporary(ir::Expr),
}

impl FunctionInfo<'_> {
    pub(crate) fn into_ir(self) -> ir::Function {
        let checked = self.checked.expect("every function of a valid program is checked");
        let ret = match self.ret {
            Ret::Declared(ty) | Ret::Inferred(ty) => ty,
            _ => unreachable!("a valid program knows every return type"),
        };
        ir::Function {
            name: self.decl.name.name.clone(),
            params: self.signature.map_or(0, |params| params.len() as u32),
            defaults: checked.defaults,
            ret,
            locals: ir_locals(checked.locals),
            body: checked.body,
            span: Some(self.decl.name.span),
        }
    }
}

impl<'a> Checker<'a> {
    /// Registers the functions and globals of the file before anything is checked.
    pub(crate) fn register_top_level(&mut self, module: &'a ast::Module) {
        let mut declarations: Vec<&'a ast::LetStmt> = Vec::new();
        for stmt in &module.stmts {
            match &stmt.kind {
                ast::StmtKind::Fun(decl) => self.register_function(decl),
                ast::StmtKind::Let(decl) => declarations.push(decl),
                _ => {}
            }
        }
        for decl in &declarations {
            let count = declarations.iter().filter(|other| other.name.name == decl.name.name).count();
            if self.globals.contains_key(&decl.name.name) {
                continue;
            }
            // A name declared once gets its local now, so that functions can refer to it.
            let local = (count == 1).then(|| {
                let info = LocalInfo::variable(&decl.name, None, decl.mutable, decl.value.is_some());
                let local = self.push_local(info);
                self.global_names.insert(local, decl.name.name.clone());
                local
            });
            self.globals.insert(decl.name.name.clone(), GlobalInfo { decl, local, ty: GlobalType::Unknown });
            if let Some(&function) = self.function_names.get(&decl.name.name) {
                self.diagnostics.push(
                    Diagnostic::error(format!("`{}` names both a function and a variable", decl.name.name))
                        .with_primary(decl.name.span, "variable declared here")
                        .with_secondary(self.functions[function].decl.name.span, "function declared here")
                        .with_help("choose different names"),
                );
            }
        }
        for index in 0..self.functions.len() {
            self.resolve_signature(index);
        }
    }

    fn register_function(&mut self, decl: &'a ast::FunDecl) {
        if let Some(&previous) = self.function_names.get(&decl.name.name) {
            self.diagnostics.push(
                Diagnostic::error(format!("the function `{}` is already declared", decl.name.name))
                    .with_primary(decl.name.span, "declared again here")
                    .with_secondary(self.functions[previous].decl.name.span, "first declared here")
                    .with_note("each function has its own name: Lion has no overloading"),
            );
            return;
        }
        self.function_names.insert(decl.name.name.clone(), self.functions.len());
        self.functions.push(FunctionInfo {
            decl,
            signature: None,
            ret: Ret::Unknown,
            state: BodyState::Unchecked,
            modifies: Vec::new(),
            reads: Vec::new(),
            calls: Vec::new(),
            checked: None,
        });
    }

    /// The types of the parameters, the return type and the `modifies` clause (§11.1).
    fn resolve_signature(&mut self, index: usize) {
        let decl = self.functions[index].decl;
        let mut supported = true;
        if decl.infix {
            self.not_implemented(decl.name.span, "`infix` functions", "§9.5");
            supported = false;
        }
        if let Some(receiver) = &decl.receiver {
            self.not_implemented(receiver.span.to(decl.name.span), "methods", "§12.4");
            supported = false;
        }
        if let Some((name, _)) = decl.type_params.first() {
            self.not_implemented(name.span, "type variables", "§15.2");
            supported = false;
        }
        let mut params = Vec::new();
        let mut defaults_started = false;
        for (position, param) in decl.params.iter().enumerate() {
            if let Some(previous) = decl.params[..position].iter().find(|p| p.name.name == param.name.name) {
                self.diagnostics.push(
                    Diagnostic::error(format!("the parameter `{}` is declared twice", param.name.name))
                        .with_primary(param.name.span, "")
                        .with_secondary(previous.name.span, "first declared here"),
                );
                supported = false;
            }
            if param.name.name == "self" && decl.receiver.is_none() {
                self.diagnostics.push(
                    Diagnostic::error("`self` is a parameter of methods only")
                        .with_primary(param.name.span, "")
                        .with_note("a method is declared `fun Type.name(...)` (§12.4)"),
                );
                supported = false;
                continue;
            }
            if let (Some(var_span), Some(default)) = (param.var, &param.default) {
                self.diagnostics.push(
                    Diagnostic::error("a `var` parameter cannot have a default value")
                        .with_primary(default.span, "")
                        .with_secondary(var_span, "this parameter works on a variable of the caller")
                        .with_note("the caller always gives the variable of a `var` parameter (§11.2)"),
                );
                supported = false;
            }
            if param.default.is_some() {
                defaults_started = true;
            } else if defaults_started {
                self.diagnostics.push(
                    Diagnostic::error(format!("the parameter `{}` needs a default value", param.name.name))
                        .with_primary(param.name.span, "")
                        .with_note("parameters with a default value come last (§11.2)"),
                );
                supported = false;
            }
            let ty = match &param.ty {
                Some(ty) => self.resolve_type(ty),
                None => {
                    self.not_implemented(
                        param.name.span,
                        "parameters without a type, which make the function generic",
                        "§11.1, §15.3",
                    );
                    None
                }
            };
            match ty {
                Some(ty) => params.push(ParamInfo {
                    name: param.name.name.clone(),
                    ty,
                    by_reference: param.var.is_some(),
                    has_default: param.default.is_some(),
                    span: param.name.span,
                }),
                None => supported = false,
            }
        }
        let ret = match &decl.ret {
            Some(ty) => match self.resolve_type(ty) {
                Some(ty) => Ret::Declared(ty),
                None => {
                    supported = false;
                    Ret::Failed
                }
            },
            None => Ret::Unknown,
        };
        let modifies = decl.modifies.iter().filter_map(|name| self.modified_global(name)).collect();
        let function = &mut self.functions[index];
        function.ret = ret;
        function.modifies = modifies;
        function.signature = supported.then_some(params);
    }

    /// A name after `modifies`: a `var` of the script, declared once.
    fn modified_global(&mut self, name: &ast::Ident) -> Option<ir::LocalId> {
        let Some(global) = self.globals.get(&name.name) else {
            self.diagnostics.push(
                Diagnostic::error(format!("`{}` is not a variable of the script", name.name))
                    .with_primary(name.span, "")
                    .with_note(
                        "`modifies` names the variables of the script that a function changes (§11.5)",
                    ),
            );
            return None;
        };
        let (local, decl) = (global.local, global.decl);
        if !decl.mutable {
            self.diagnostics.push(
                Diagnostic::error(format!("`{}` is a constant", name.name))
                    .with_primary(name.span, "")
                    .with_secondary(decl.name.span, "declared with `let`")
                    .with_help("declare it with `var` for a function to change it"),
            );
            return None;
        }
        if local.is_none() {
            self.diagnostics.push(
                Diagnostic::error(format!(
                    "`{}` is declared several times at the top level of the script",
                    name.name
                ))
                .with_primary(name.span, "a function cannot tell which declaration this is")
                .with_help("give the top-level declarations different names"),
            );
        }
        local
    }

    /// Checks the bodies that no call required earlier, so that their errors are reported.
    pub(crate) fn check_remaining_functions(&mut self) {
        for index in 0..self.functions.len() {
            self.check_function_body(index);
        }
    }

    fn check_function_body(&mut self, index: usize) {
        if self.functions[index].state != BodyState::Unchecked {
            return;
        }
        let Some(params) = self.functions[index].signature.clone() else {
            let function = &mut self.functions[index];
            function.state = BodyState::Checked;
            if let Ret::Unknown = function.ret {
                function.ret = Ret::Failed;
            }
            return;
        };
        let function = &mut self.functions[index];
        function.state = BodyState::Checking;
        if let Ret::Unknown = function.ret {
            function.ret = Ret::Inferring;
        }
        let interrupted = std::mem::replace(&mut self.ctx, Context::new(ContextKind::Function(index)));
        let checked = self.function_body(index, &params);
        let ctx = std::mem::replace(&mut self.ctx, interrupted);
        let function = &mut self.functions[index];
        function.reads = ctx.reads;
        function.calls = ctx.calls;
        function.checked = Some(CheckedBody { locals: ctx.locals, body: checked.0, defaults: checked.1 });
        function.state = BodyState::Checked;
    }

    fn function_body(&mut self, index: usize, params: &[ParamInfo]) -> (Vec<ir::Stmt>, Vec<(u32, ir::Expr)>) {
        let decl = self.functions[index].decl;
        // The parameters are the first locals (C6: they belong to the outermost block).
        let ids: Vec<ir::LocalId> = params
            .iter()
            .map(|param| {
                let id = self.push_local(LocalInfo {
                    name: param.name.clone(),
                    ty: Some(param.ty),
                    mutable: param.by_reference,
                    temporary: false,
                    by_reference: param.by_reference,
                    decl_span: param.span,
                    initialized: true,
                    first_assignment: None,
                });
                self.ctx.flow.set(id, Assigned::Yes);
                id
            })
            .collect();
        // A default value sees the globals and the parameters before it (C10).
        let mut defaults = Vec::new();
        for (position, (param, id)) in params.iter().zip(&ids).enumerate() {
            if let Some(default) = &decl.params[position].default
                && let Some(value) = self.expr(default)
                && let Some(value) =
                    self.coerce(value, param.ty, Some((param.span, "parameter declared here".to_string())))
            {
                defaults.push((position as u32, value));
            }
            let scope = self.ctx.scopes.first_mut().expect("the outermost block");
            scope.names.insert(param.name.clone(), *id);
        }
        let mut body = match &decl.body {
            ast::FunBody::Block(block) => self.stmts(&block.stmts),
            // `fun f(x) = expr` returns `expr` (§11.1).
            ast::FunBody::Expr(value) => {
                self.return_in_function(index, Some(value), value.span).into_iter().collect()
            }
        };
        let falls_through = self.ctx.flow.is_reachable();
        self.close_scope();
        let ret = self.settle_return_type(index);
        if falls_through && ret.is_some_and(|ty| ty != Type::None) {
            let name = &decl.name.name;
            self.diagnostics.push(
                Diagnostic::error(format!("`{name}` does not return a value on every path"))
                    .with_primary(decl.name.span, "this function can reach its end without `return`")
                    .with_note(
                        "when a `return` of a function gives a value, every path must end with one (§11.4)",
                    ),
            );
        }
        if ret == Some(Type::Float) {
            widen_returns(&mut body);
        }
        (body, defaults)
    }

    /// The declared return type, or the one inferred from the `return` statements (C12).
    fn settle_return_type(&mut self, index: usize) -> Option<Type> {
        match self.functions[index].ret {
            Ret::Declared(ty) => return Some(ty),
            Ret::Inferring => {}
            _ => return None,
        }
        if self.ctx.failed_return {
            self.functions[index].ret = Ret::Failed;
            return None;
        }
        let returns = std::mem::take(&mut self.ctx.returns);
        let values: Vec<(Type, Span)> =
            returns.iter().filter_map(|&(ty, span)| ty.map(|ty| (ty, span))).collect();
        let ret = if values.is_empty() {
            Some(Type::None)
        } else if let Some(&(_, bare)) = returns.iter().find(|(ty, _)| ty.is_none()) {
            let name = &self.functions[index].decl.name.name;
            self.diagnostics.push(
                Diagnostic::error(format!(
                    "this `return` has no value, but other returns of `{name}` give one"
                ))
                .with_primary(bare, "")
                .with_secondary(values[0].1, "a value is returned here")
                .with_note(
                    "when a `return` of a function gives a value, every path must end with one (§11.4)",
                ),
            );
            None
        } else if values.iter().all(|(ty, _)| *ty == values[0].0) {
            Some(values[0].0)
        } else if values.iter().all(|(ty, _)| ty.is_numeric()) {
            Some(Type::Float)
        } else {
            let span = self.functions[index].decl.name.span;
            self.not_implemented(
                span,
                "functions that return values of different types (the result would be a union such as `Int or Text`)",
                "§7.3, §11.4",
            );
            None
        };
        self.functions[index].ret = ret.map_or(Ret::Failed, Ret::Inferred);
        ret
    }

    /// The return type of a function, checking its body first if it is inferred.
    fn return_type_of(&mut self, index: usize, call: Span) -> Option<Type> {
        if let Ret::Unknown = self.functions[index].ret {
            self.demands.push(call);
            self.check_function_body(index);
            self.demands.pop();
        }
        match self.functions[index].ret {
            Ret::Declared(ty) | Ret::Inferred(ty) => Some(ty),
            Ret::Inferring => {
                let decl = self.functions[index].decl;
                let name = &decl.name.name;
                self.diagnostics.push(
                    Diagnostic::error(format!("the return type of `{name}` must be written"))
                        .with_primary(
                            call,
                            format!("`{name}` is called here before its return type is known"),
                        )
                        .with_secondary(decl.name.span, "declared here")
                        .with_help(format!("write it after the parameters: `fun {name}(...) in Type`")),
                );
                None
            }
            Ret::Unknown | Ret::Failed => None,
        }
    }

    /// `return` inside a function (§11.4).
    pub(crate) fn return_in_function(
        &mut self,
        index: usize,
        value: Option<&ast::Expr>,
        span: Span,
    ) -> Option<ir::Stmt> {
        let declared = match self.functions[index].ret {
            Ret::Declared(ty) => Some(ty),
            _ => None,
        };
        let stmt = match value {
            None => {
                if let Some(ty) = declared.filter(|ty| *ty != Type::None) {
                    let name = &self.functions[index].decl.name.name;
                    self.diagnostics.push(
                        Diagnostic::error(format!(
                            "`{name}` returns {}: this `return` needs a value",
                            article(ty)
                        ))
                        .with_primary(span, ""),
                    );
                }
                self.ctx.returns.push((None, span));
                Some(ir::Stmt::Return(None))
            }
            Some(expr) => {
                let checked = self.expr(expr);
                let value = match (checked, declared) {
                    (Some(checked), Some(ty)) => {
                        let ret = self.functions[index].decl.ret.as_ref();
                        let context = ret.map(|ret| (ret.span, "return type declared here".to_string()));
                        self.coerce(checked, ty, context)
                    }
                    (Some(checked), None) => {
                        self.ctx.returns.push((Some(checked.ty), expr.span));
                        Some(checked)
                    }
                    (None, _) => {
                        self.ctx.failed_return = true;
                        None
                    }
                };
                value.map(|value| ir::Stmt::Return(Some(value)))
            }
        };
        self.ctx.flow = crate::flow::Flow::unreachable();
        stmt
    }

    /// A call of a function of the file (§11.2).
    pub(crate) fn call_function(
        &mut self,
        index: usize,
        callee: Span,
        args: &[ast::Arg],
        span: Span,
    ) -> Option<ir::Expr> {
        let name = self.functions[index].decl.name.name.clone();
        let Some(params) = self.functions[index].signature.clone() else {
            for arg in args {
                self.expr(&arg.value);
            }
            return None;
        };
        let required = params.iter().filter(|param| !param.has_default).count();
        if args.len() > params.len() || args.is_empty() && required > 0 {
            let expected = if required == params.len() {
                format!("{required}")
            } else {
                format!("{required} to {}", params.len())
            };
            let plural = if params.len() == 1 { "" } else { "s" };
            self.diagnostics.push(
                Diagnostic::error(format!("`{name}` takes {expected} argument{plural}, not {}", args.len()))
                    .with_primary(span, "")
                    .with_secondary(self.functions[index].decl.name.span, "declared here"),
            );
            for arg in args {
                self.expr(&arg.value);
            }
            return None;
        }
        if args.len() < required {
            self.not_implemented(
                span,
                "partial application (a call that gives only some of the arguments)",
                "§11.3",
            );
            return None;
        }
        let mut pending = Vec::new();
        for (arg, param) in args.iter().zip(&params) {
            pending.push(self.argument(&name, arg, param));
        }
        let ret = self.return_type_of(index, callee)?;
        let pending: Vec<Pending> = pending.into_iter().collect::<Option<_>>()?;
        self.ctx.calls.push(index);
        if self.ctx.kind == ContextKind::Script {
            let unassigned = self.unassigned_globals();
            self.script_calls.push(ScriptCall { function: index, span, unassigned });
        }
        Some(self.build_call(index, pending, ret, span))
    }

    fn argument(&mut self, function: &str, arg: &ast::Arg, param: &ParamInfo) -> Option<Pending> {
        let mut valid = true;
        if let Some(name) = &arg.name
            && name.name != param.name
        {
            self.diagnostics.push(
                Diagnostic::error(format!(
                    "this argument is named `{}`, but the parameter at this position is `{}`",
                    name.name, param.name
                ))
                .with_primary(name.span, "")
                .with_secondary(param.span, "parameter declared here")
                .with_note("named arguments keep the order of the parameters (§11.2)"),
            );
            valid = false;
        }
        if let Some(var_span) = arg.var_marker
            && !param.by_reference
        {
            self.diagnostics.push(
                Diagnostic::error(format!("the parameter `{}` of `{function}` is not `var`", param.name))
                    .with_primary(var_span, "remove this `var`")
                    .with_secondary(param.span, "parameter declared here")
                    .with_note("`var` at a call marks an argument that the function modifies (§11.2)"),
            );
            valid = false;
        }
        let checked = if param.by_reference {
            self.reference_argument(arg, param)
        } else {
            self.value_argument(arg, param)
        };
        checked.filter(|_| valid)
    }

    fn value_argument(&mut self, arg: &ast::Arg, param: &ParamInfo) -> Option<Pending> {
        let value = self.expr(&arg.value)?;
        let value =
            self.coerce(value, param.ty, Some((param.span, "parameter declared here".to_string())))?;
        Some(Pending::Value(value))
    }

    /// The argument of a `var` parameter: a `var` variable, which the call may change,
    /// or any other value, whose change is lost (§11.2).
    fn reference_argument(&mut self, arg: &ast::Arg, param: &ParamInfo) -> Option<Pending> {
        let ast::ExprKind::Name(name) = &arg.value.kind else {
            let value = self.expr(&arg.value)?;
            let value =
                self.coerce(value, param.ty, Some((param.span, "parameter declared here".to_string())))?;
            return Some(Pending::Temporary(value));
        };
        let span = arg.value.span;
        let (place, ty, mutable, decl_span) = match self.resolve(name, span) {
            Resolved::Local(local) => {
                // An argument of a `var` parameter must have a value (C8).
                if !self.check_has_value(local, span) {
                    return None;
                }
                let info = &self.ctx.locals[local.index()];
                (ir::Place::Local(local), info.ty?, info.mutable, info.decl_span)
            }
            Resolved::Global(local) => {
                // Giving a global to a `var` parameter changes it (C7).
                if !self.check_global_assignment(local, span) {
                    return None;
                }
                self.ctx.reads.push(local);
                let global = &self.globals[&self.global_names[&local]];
                let GlobalType::Known(ty) = global.ty else { return None };
                (ir::Place::Global(local), ty?, true, global.decl.name.span)
            }
            Resolved::Function(_) | Resolved::Standard(_) => {
                self.not_implemented(span, "functions used as values", "§11.3");
                return None;
            }
            Resolved::Nothing => return None,
        };
        if !mutable {
            self.diagnostics.push(
                Diagnostic::error(format!(
                    "the constant `{name}` cannot be given to the `var` parameter `{}`",
                    param.name
                ))
                .with_primary(span, "")
                .with_secondary(decl_span, "declared with `let`")
                .with_help(format!("declare `{name}` with `var`, or give a copy: `({name})`")),
            );
            return None;
        }
        if ty != param.ty {
            self.diagnostics.push(
                Diagnostic::error("mismatched types")
                    .with_primary(span, format!("this is {}", article(ty)))
                    .with_secondary(param.span, format!("this `var` parameter is {}", article(param.ty)))
                    .with_note("a `var` parameter works on the caller's variable itself, so both have the same type (§11.2)"),
            );
            return None;
        }
        Some(Pending::Place(place))
    }

    /// The call, keeping the left-to-right order of the arguments (§9.2) when some of
    /// them are stored in temporaries to be passed by reference.
    fn build_call(&mut self, index: usize, pending: Vec<Pending>, ret: Type, span: Span) -> ir::Expr {
        let needs_temporaries = pending.iter().any(|arg| matches!(arg, Pending::Temporary(_)));
        let mut lets = Vec::new();
        let mut args = Vec::new();
        for arg in pending {
            match arg {
                Pending::Value(value) if needs_temporaries => {
                    let temp = self.temporary(value.ty, value.span);
                    args.push(ir::Arg::Value(typed(ir::ExprKind::Local(temp), value.ty, value.span)));
                    lets.push((temp, value));
                }
                Pending::Value(value) => args.push(ir::Arg::Value(value)),
                Pending::Place(place) => args.push(ir::Arg::Reference(place)),
                Pending::Temporary(value) => {
                    let temp = self.temporary(value.ty, value.span);
                    args.push(ir::Arg::Reference(ir::Place::Local(temp)));
                    lets.push((temp, value));
                }
            }
        }
        let mut call = typed(ir::ExprKind::Call { function: ir::FunctionId(index as u32), args }, ret, span);
        for (local, value) in lets.into_iter().rev() {
            call =
                typed(ir::ExprKind::Let { local, value: Box::new(value), body: Box::new(call) }, ret, span);
        }
        call
    }

    /// The globals that may have no value at the current point of the script.
    fn unassigned_globals(&self) -> Vec<ir::LocalId> {
        self.globals
            .values()
            .filter_map(|global| global.local)
            .filter(|&local| self.ctx.flow.get(local) != Assigned::Yes)
            .collect()
    }

    /// Whether the current function may assign the global `local` (§11.5).
    pub(crate) fn check_global_assignment(&mut self, local: ir::LocalId, span: Span) -> bool {
        let ContextKind::Function(index) = self.ctx.kind else { return true };
        let global = &self.globals[&self.global_names[&local]];
        let (decl, name) = (global.decl, &global.decl.name.name);
        let error = if !decl.mutable && decl.value.is_none() {
            Diagnostic::error(format!(
                "the constant `{name}` receives its value in the script, not in a function"
            ))
            .with_primary(span, "")
            .with_secondary(decl.name.span, "declared here without a value")
        } else if !decl.mutable {
            Diagnostic::error(format!("cannot assign to the constant `{name}`"))
                .with_primary(span, "")
                .with_secondary(decl.name.span, "declared with `let`")
        } else if !self.functions[index].modifies.contains(&local) {
            let function = self.functions[index].decl;
            Diagnostic::error(format!("`{}` modifies `{name}` without saying so", function.name.name))
                .with_primary(span, "")
                .with_secondary(function.name.span, format!("add `modifies {name}` to this declaration"))
                .with_note("a function announces the variables of the script it changes (§11.5)")
        } else {
            return true;
        };
        self.diagnostics.push(error);
        false
    }

    /// Each call made by the script: the function must not read, directly or through
    /// the functions it calls, a global that may have no value there (C3).
    pub(crate) fn check_script_calls(&mut self) {
        let mut reads: Vec<HashSet<ir::LocalId>> =
            self.functions.iter().map(|function| function.reads.iter().copied().collect()).collect();
        loop {
            let mut changed = false;
            for caller in 0..self.functions.len() {
                for &callee in &self.functions[caller].calls {
                    let missing: Vec<ir::LocalId> =
                        reads[callee].difference(&reads[caller]).copied().collect();
                    if !missing.is_empty() {
                        reads[caller].extend(missing);
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        for call in std::mem::take(&mut self.script_calls) {
            let mut unassigned: Vec<ir::LocalId> = call
                .unassigned
                .iter()
                .copied()
                .filter(|global| reads[call.function].contains(global))
                .collect();
            unassigned.sort_by_key(|local| local.0);
            for global in unassigned {
                let decl = self.globals[&self.global_names[&global]].decl;
                let function = &self.functions[call.function].decl.name.name;
                self.diagnostics.push(
                    Diagnostic::error(format!("`{function}` may read `{}`, which may have no value at this call", decl.name.name))
                        .with_primary(call.span, "")
                        .with_secondary(decl.name.span, "declared here")
                        .with_note("a function may read a variable declared further down, but only once it has a value (§6.1, §6.5)"),
                );
            }
        }
    }
}

/// Converts Int return values to Float, once the return type is known to be Float.
fn widen_returns(stmts: &mut [ir::Stmt]) {
    for stmt in stmts {
        match stmt {
            ir::Stmt::Return(Some(value)) if value.ty == Type::Int => {
                let span = value.span;
                let int = std::mem::replace(value, typed(ir::ExprKind::None, Type::None, span));
                *value = typed(
                    ir::ExprKind::Convert { conversion: ir::Conversion::IntToFloat, value: Box::new(int) },
                    Type::Float,
                    span,
                );
            }
            ir::Stmt::If { then, otherwise, .. } => {
                widen_returns(then);
                widen_returns(otherwise);
            }
            ir::Stmt::While { body, .. } => widen_returns(body),
            _ => {}
        }
    }
}
