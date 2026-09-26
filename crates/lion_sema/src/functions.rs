//! Functions (spec §11): declarations, bodies, calls, and the globals they reach.
//!
//! Top-level functions are registered before the script is checked, so they can be
//! called before their declaration (C4). A function whose parameters all have a type
//! has one *instance*. A parameter without a type makes the function generic (C1): each
//! call creates, or reuses, the instance for the types of its arguments, checked and
//! compiled on its own (§15.3).
//!
//! Each instance records the globals it reads and the instances it calls; once every
//! body is known, the reads are closed over the calls, and each call made by the script
//! is checked: it must not read a global that may have no value at that point (§6.1, C3).

use std::collections::{HashMap, HashSet};

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
    /// The return type, when written.
    declared_ret: Option<Type>,
    /// The globals named after `modifies` (§11.5).
    modifies: Vec<ir::LocalId>,
    /// The instances of a generic function, by the types of the given arguments.
    instances: HashMap<Vec<Type>, usize>,
    /// The only instance of a function whose parameters all have a type.
    instance: Option<usize>,
}

impl FunctionInfo<'_> {
    fn is_generic(&self) -> bool {
        self.signature.as_ref().is_some_and(|params| params.iter().any(|param| param.ty.is_none()))
    }
}

#[derive(Clone)]
struct ParamInfo {
    name: String,
    /// `None` for a parameter without a type, which makes the function generic.
    ty: Option<Type>,
    by_reference: bool,
    has_default: bool,
    span: Span,
}

/// One checked version of a function.
pub(crate) struct Instance {
    function: usize,
    /// For a generic function: the types of the arguments given by the calls of this
    /// instance, which gave that many arguments. Empty otherwise.
    arg_types: Vec<Type>,
    /// The call that created a generic instance, for messages.
    origin: Option<Span>,
    ret: Ret,
    state: BodyState,
    /// Known once the body is checked.
    reads: Vec<ir::LocalId>,
    calls: Vec<usize>,
    checked: Option<CheckedBody>,
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
    params: Vec<Type>,
    locals: Vec<LocalInfo>,
    body: Vec<ir::Stmt>,
    defaults: Vec<(u32, ir::Expr)>,
}

/// A call made by the script.
pub(crate) struct ScriptCall {
    instance: usize,
    span: Span,
    /// The globals that may have no value at the call.
    unassigned: Vec<ir::LocalId>,
}

/// An argument once checked, before the call is built.
enum Pending {
    Value(ir::Expr),
    /// A variable given to a `var` parameter, and its type.
    Place(ir::Place, Type),
    /// Any other value given to a `var` parameter: the modification is lost (§11.2).
    Temporary(ir::Expr),
}

impl Pending {
    fn ty(&self) -> Type {
        match self {
            Pending::Value(value) | Pending::Temporary(value) => value.ty,
            Pending::Place(_, ty) => *ty,
        }
    }
}

impl Instance {
    pub(crate) fn into_ir(self, functions: &[FunctionInfo]) -> ir::Function {
        let decl = functions[self.function].decl;
        let checked = self.checked.expect("every instance of a valid program is checked");
        let ret = match self.ret {
            Ret::Declared(ty) | Ret::Inferred(ty) => ty,
            _ => unreachable!("a valid program knows every return type"),
        };
        let name = if self.arg_types.is_empty() {
            decl.name.name.clone()
        } else {
            let types: Vec<String> = checked.params.iter().map(Type::to_string).collect();
            format!("{}[{}]", decl.name.name, types.join(", "))
        };
        ir::Function {
            name,
            params: checked.params.len() as u32,
            defaults: checked.defaults,
            ret,
            locals: ir_locals(checked.locals),
            body: checked.body,
            span: Some(decl.name.span),
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
            let function = &self.functions[index];
            if function.signature.is_some() && !function.is_generic() {
                let ret = function.declared_ret.map_or(Ret::Unknown, Ret::Declared);
                let instance = self.new_instance(index, Vec::new(), None, ret);
                self.functions[index].instance = Some(instance);
            }
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
            declared_ret: None,
            modifies: Vec::new(),
            instances: HashMap::new(),
            instance: None,
        });
    }

    fn new_instance(
        &mut self,
        function: usize,
        arg_types: Vec<Type>,
        origin: Option<Span>,
        ret: Ret,
    ) -> usize {
        self.instances.push(Instance {
            function,
            arg_types,
            origin,
            ret,
            state: BodyState::Unchecked,
            reads: Vec::new(),
            calls: Vec::new(),
            checked: None,
        });
        self.instances.len() - 1
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
                Some(written) => match self.resolve_type(written) {
                    Some(ty) => Some(ty),
                    None => {
                        supported = false;
                        None
                    }
                },
                // Without a type, the parameter takes the type of each call's argument (C1).
                None => None,
            };
            params.push(ParamInfo {
                name: param.name.name.clone(),
                ty,
                by_reference: param.var.is_some(),
                has_default: param.default.is_some(),
                span: param.name.span,
            });
        }
        let declared_ret = match &decl.ret {
            Some(ty) => {
                let ty = self.resolve_type(ty);
                supported &= ty.is_some();
                ty
            }
            None => None,
        };
        let modifies = decl.modifies.iter().filter_map(|name| self.modified_global(name)).collect();
        let function = &mut self.functions[index];
        function.declared_ret = declared_ret;
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

    /// Checks the bodies that no call required earlier, so that their errors are
    /// reported. A generic function is checked for the types it is called with: one
    /// that is never called cannot be checked, which a warning says.
    pub(crate) fn check_remaining_functions(&mut self) {
        let mut index = 0;
        while index < self.instances.len() {
            self.check_instance(index);
            index += 1;
        }
        for function in &self.functions {
            if function.is_generic() && function.instances.is_empty() {
                let name = &function.decl.name.name;
                self.diagnostics.push(
                    Diagnostic::new(lion_diagnostics::Severity::Warning, format!("`{name}` is never called, so it is not checked"))
                        .with_primary(function.decl.name.span, "")
                        .with_note("a function with parameters without a type is checked for the types of each call (§15.3)"),
                );
            }
        }
    }

    fn check_instance(&mut self, instance: usize) {
        if self.instances[instance].state != BodyState::Unchecked {
            return;
        }
        let function = self.instances[instance].function;
        let params = self.functions[function].signature.clone().expect("an instance has a signature");
        self.instances[instance].state = BodyState::Checking;
        if let Ret::Unknown = self.instances[instance].ret {
            self.instances[instance].ret = Ret::Inferring;
        }
        let first_diagnostic = self.diagnostics.len();
        let interrupted = std::mem::replace(&mut self.ctx, Context::new(ContextKind::Function(instance)));
        let (body, defaults, param_types) = self.function_body(instance, &params);
        let ctx = std::mem::replace(&mut self.ctx, interrupted);
        // An error in a generic function shows which call created the instance.
        if let Some(origin) = self.instances[instance].origin {
            let name = &self.functions[function].decl.name.name;
            let types: Vec<String> = param_types.iter().map(Type::to_string).collect();
            let label = format!("`{name}` is checked for ({}) because of this call", types.join(", "));
            for diagnostic in &mut self.diagnostics[first_diagnostic..] {
                diagnostic.labels.push(lion_diagnostics::Label {
                    span: origin,
                    message: label.clone(),
                    primary: false,
                });
            }
        }
        let checked = &mut self.instances[instance];
        checked.reads = ctx.reads;
        checked.calls = ctx.calls;
        checked.checked = Some(CheckedBody { params: param_types, locals: ctx.locals, body, defaults });
        checked.state = BodyState::Checked;
    }

    /// The body of an instance, its default values, and the types of its parameters.
    fn function_body(
        &mut self,
        instance: usize,
        params: &[ParamInfo],
    ) -> (Vec<ir::Stmt>, Vec<(u32, ir::Expr)>, Vec<Type>) {
        let function = self.instances[instance].function;
        let decl = self.functions[function].decl;
        let arg_types = self.instances[instance].arg_types.clone();
        let generic = self.functions[function].is_generic();
        // The parameters are the first locals (C6: they belong to the outermost block).
        let ids: Vec<ir::LocalId> = params
            .iter()
            .enumerate()
            .map(|(position, param)| {
                let ty = param.ty.or_else(|| arg_types.get(position).copied());
                let id = self.push_local(LocalInfo {
                    name: param.name.clone(),
                    ty,
                    mutable: param.by_reference,
                    temporary: false,
                    by_reference: param.by_reference,
                    decl_span: param.span,
                    initialized: true,
                    first_assignment: None,
                    loop_variable: false,
                });
                self.ctx.flow.set(id, Assigned::Yes);
                id
            })
            .collect();
        // A default value sees the globals and the parameters before it (C10). In a
        // generic instance, only the omitted arguments need it, and a parameter without
        // a type takes the type of its default value.
        let mut defaults = Vec::new();
        let mut valid = true;
        for (position, (param, id)) in params.iter().zip(&ids).enumerate() {
            let needed = !generic || position >= arg_types.len();
            if let (true, Some(default)) = (needed, &decl.params[position].default) {
                match self.expr(default) {
                    Some(value) => {
                        let expected = self.ctx.locals[id.index()].ty.unwrap_or(value.ty);
                        self.ctx.locals[id.index()].ty = Some(expected);
                        let context = (param.span, "parameter declared here".to_string());
                        match self.coerce(value, expected, Some(context)) {
                            Some(value) => defaults.push((position as u32, value)),
                            None => valid = false,
                        }
                    }
                    None => valid = false,
                }
            }
            let scope = self.ctx.scopes.first_mut().expect("the outermost block");
            scope.names.insert(param.name.clone(), *id);
        }
        let param_types: Vec<Type> =
            ids.iter().map(|id| self.ctx.locals[id.index()].ty.unwrap_or(Type::None)).collect();
        if !valid {
            self.instances[instance].ret = Ret::Failed;
            return (Vec::new(), defaults, param_types);
        }
        let mut body = match &decl.body {
            ast::FunBody::Block(block) => self.stmts(&block.stmts),
            // `fun f(x) = expr` returns `expr` (§11.1).
            ast::FunBody::Expr(value) => {
                self.return_in_function(instance, Some(value), value.span).into_iter().collect()
            }
        };
        let falls_through = self.ctx.flow.is_reachable();
        self.close_scope();
        let ret = self.settle_return_type(instance);
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
        (body, defaults, param_types)
    }

    /// The declared return type, or the one inferred from the `return` statements (C12).
    fn settle_return_type(&mut self, instance: usize) -> Option<Type> {
        match self.instances[instance].ret {
            Ret::Declared(ty) => return Some(ty),
            Ret::Inferring => {}
            _ => return None,
        }
        if self.ctx.failed_return {
            self.instances[instance].ret = Ret::Failed;
            return None;
        }
        let decl = self.functions[self.instances[instance].function].decl;
        let returns = std::mem::take(&mut self.ctx.returns);
        let values: Vec<(Type, Span)> =
            returns.iter().filter_map(|&(ty, span)| ty.map(|ty| (ty, span))).collect();
        let ret = if values.is_empty() {
            Some(Type::None)
        } else if let Some(&(_, bare)) = returns.iter().find(|(ty, _)| ty.is_none()) {
            let name = &decl.name.name;
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
            self.not_implemented(
                decl.name.span,
                "functions that return values of different types (the result would be a union such as `Int or Text`)",
                "§7.3, §11.4",
            );
            None
        };
        self.instances[instance].ret = ret.map_or(Ret::Failed, Ret::Inferred);
        ret
    }

    /// The return type of an instance, checking its body first if it is inferred.
    fn return_type_of(&mut self, instance: usize, call: Span) -> Option<Type> {
        if let Ret::Unknown = self.instances[instance].ret {
            self.demands.push(call);
            self.check_instance(instance);
            self.demands.pop();
        }
        match self.instances[instance].ret {
            Ret::Declared(ty) | Ret::Inferred(ty) => Some(ty),
            Ret::Inferring => {
                let decl = self.functions[self.instances[instance].function].decl;
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
        instance: usize,
        value: Option<&ast::Expr>,
        span: Span,
    ) -> Option<ir::Stmt> {
        let declared = match self.instances[instance].ret {
            Ret::Declared(ty) => Some(ty),
            _ => None,
        };
        let decl = self.functions[self.instances[instance].function].decl;
        let stmt = match value {
            None => {
                if let Some(ty) = declared.filter(|ty| *ty != Type::None) {
                    let name = &decl.name.name;
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
                let checked = match declared {
                    Some(ty) => self.expr_expecting(expr, ty),
                    None => self.expr(expr),
                };
                let value = match (checked, declared) {
                    (Some(checked), Some(ty)) => {
                        let context =
                            decl.ret.as_ref().map(|ret| (ret.span, "return type declared here".to_string()));
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
        let pending: Vec<Pending> = pending.into_iter().collect::<Option<_>>()?;
        let instance = match self.functions[index].instance {
            Some(instance) => instance,
            None => self.instantiate(index, pending.iter().map(Pending::ty).collect(), callee)?,
        };
        let ret = self.return_type_of(instance, callee)?;
        self.ctx.calls.push(instance);
        if self.ctx.kind == ContextKind::Script {
            let unassigned = self.unassigned_globals();
            self.script_calls.push(ScriptCall { instance, span, unassigned });
        }
        Some(self.build_call(instance, pending, ret, span))
    }

    /// The instance of a generic function for the types of the given arguments (C1).
    fn instantiate(&mut self, function: usize, arg_types: Vec<Type>, call: Span) -> Option<usize> {
        if let Some(&instance) = self.functions[function].instances.get(&arg_types) {
            return Some(instance);
        }
        let ret = self.functions[function].declared_ret.map_or(Ret::Unknown, Ret::Declared);
        let instance = self.new_instance(function, arg_types.clone(), Some(call), ret);
        self.functions[function].instances.insert(arg_types, instance);
        Some(instance)
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
        let value = match param.ty {
            Some(ty) => self.expr_expecting(&arg.value, ty)?,
            None => self.expr(&arg.value)?,
        };
        match param.ty {
            Some(ty) => {
                let context = (param.span, "parameter declared here".to_string());
                Some(Pending::Value(self.coerce(value, ty, Some(context))?))
            }
            None => Some(Pending::Value(value)),
        }
    }

    /// The argument of a `var` parameter: a `var` variable, which the call may change,
    /// or any other value, whose change is lost (§11.2).
    fn reference_argument(&mut self, arg: &ast::Arg, param: &ParamInfo) -> Option<Pending> {
        let ast::ExprKind::Name(name) = &arg.value.kind else {
            let value = self.expr(&arg.value)?;
            let value = match param.ty {
                Some(ty) => {
                    self.coerce(value, ty, Some((param.span, "parameter declared here".to_string())))?
                }
                None => value,
            };
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
        if let Some(expected) = param.ty
            && ty != expected
        {
            self.diagnostics.push(
                Diagnostic::error("mismatched types")
                    .with_primary(span, format!("this is {}", article(ty)))
                    .with_secondary(param.span, format!("this `var` parameter is {}", article(expected)))
                    .with_note("a `var` parameter works on the caller's variable itself, so both have the same type (§11.2)"),
            );
            return None;
        }
        Some(Pending::Place(place, ty))
    }

    /// The call, keeping the left-to-right order of the arguments (§9.2) when some of
    /// them are stored in temporaries to be passed by reference.
    fn build_call(&mut self, instance: usize, pending: Vec<Pending>, ret: Type, span: Span) -> ir::Expr {
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
                Pending::Place(place, _) => args.push(ir::Arg::Reference(place)),
                Pending::Temporary(value) => {
                    let temp = self.temporary(value.ty, value.span);
                    args.push(ir::Arg::Reference(ir::Place::Local(temp)));
                    lets.push((temp, value));
                }
            }
        }
        let function = ir::FunctionId(instance as u32);
        let mut call = typed(ir::ExprKind::Call { function, args }, ret, span);
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
        let ContextKind::Function(instance) = self.ctx.kind else { return true };
        let function = &self.functions[self.instances[instance].function];
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
        } else if !function.modifies.contains(&local) {
            Diagnostic::error(format!("`{}` modifies `{name}` without saying so", function.decl.name.name))
                .with_primary(span, "")
                .with_secondary(function.decl.name.span, format!("add `modifies {name}` to this declaration"))
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
            self.instances.iter().map(|instance| instance.reads.iter().copied().collect()).collect();
        loop {
            let mut changed = false;
            for caller in 0..self.instances.len() {
                for &callee in &self.instances[caller].calls {
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
                .filter(|global| reads[call.instance].contains(global))
                .collect();
            unassigned.sort_by_key(|local| local.0);
            for global in unassigned {
                let decl = self.globals[&self.global_names[&global]].decl;
                let function = &self.functions[self.instances[call.instance].function].decl.name.name;
                self.diagnostics.push(
                    Diagnostic::error(format!(
                        "`{function}` may read `{}`, which may have no value at this call",
                        decl.name.name
                    ))
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
