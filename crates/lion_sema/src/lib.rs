//! Name resolution and type checking: from the syntax tree to the typed IR.
//!
//! Every compile error that is not a syntax error comes from here, whatever the
//! execution mode (spec §22.1). The checker reports all the problems it finds and
//! builds an [`ir::Program`] only when there is none.
//!
//! The file is the script (§20.1). Its top-level functions are registered first, so
//! they can be called before their declaration. The script is then checked in order;
//! a function body is checked when a call needs its inferred return type, and every
//! other body afterwards.

mod collections;
mod expr;
mod flow;
mod functions;
mod names;
mod stmt;
mod types;

use std::collections::HashMap;

use lion_diagnostics::{Diagnostic, Span};
use lion_ir::{self as ir, Type};
use lion_syntax::ast;

use crate::flow::{Assigned, Flow};
use crate::functions::{FunctionInfo, Instance, ScriptCall};

pub struct Checked {
    /// Present only when there are no errors.
    pub program: Option<ir::Program>,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn check(module: &ast::Module) -> Checked {
    let mut checker = Checker::new(module);
    let body = checker.stmts(&module.stmts);
    checker.close_scope();
    checker.ctx.body = body;
    checker.check_remaining_functions();
    checker.check_script_calls();
    checker.finish()
}

/// What the checker knows about a local.
struct LocalInfo {
    name: String,
    /// `None` when its declaration had an error, already reported.
    ty: Option<Type>,
    mutable: bool,
    temporary: bool,
    /// A `var` parameter (§11.2).
    by_reference: bool,
    decl_span: Span,
    /// Declared with a value (`let x = 1`) rather than without (`let x in Int`).
    initialized: bool,
    /// The first assignment met, for messages.
    first_assignment: Option<Span>,
    /// The variable of a `for` loop.
    loop_variable: bool,
}

impl LocalInfo {
    fn variable(name: &ast::Ident, ty: Option<Type>, mutable: bool, initialized: bool) -> LocalInfo {
        LocalInfo {
            name: name.name.clone(),
            ty,
            mutable,
            temporary: false,
            by_reference: false,
            decl_span: name.span,
            initialized,
            first_assignment: None,
            loop_variable: false,
        }
    }
}

/// The names declared in one block.
#[derive(Default)]
struct Scope {
    names: HashMap<String, ir::LocalId>,
    /// In order of declaration, including those hidden by a later declaration.
    declared: Vec<ir::LocalId>,
}

/// The flows that leave a loop through `break`.
#[derive(Default)]
struct LoopExits {
    breaks: Vec<Flow>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ContextKind {
    /// The top-level statements of the file.
    Script,
    /// The body of an instance of a top-level function, by index.
    Function(usize),
}

/// The state of the function being checked.
struct Context {
    kind: ContextKind,
    locals: Vec<LocalInfo>,
    /// The blocks being checked, innermost last. Declaring a name again in the same
    /// block hides the old binding (§6.3); declaring a name of an enclosing block in an
    /// inner block is an error (§6.5).
    scopes: Vec<Scope>,
    /// What is known at the current point of the function.
    flow: Flow,
    /// The loops being checked, innermost last.
    loops: Vec<LoopExits>,
    /// The type of each `return` value (`None` for a bare `return`), to infer the
    /// return type of a function that does not write it.
    returns: Vec<(Option<Type>, Span)>,
    /// Whether the value of a `return` had an error, so the return type is unknown.
    failed_return: bool,
    /// The globals read and the functions called, for the check of the calls made by
    /// the script (C3).
    reads: Vec<ir::LocalId>,
    calls: Vec<usize>,
    body: Vec<ir::Stmt>,
}

impl Context {
    fn new(kind: ContextKind) -> Context {
        Context {
            kind,
            locals: Vec::new(),
            scopes: vec![Scope::default()],
            flow: Flow::start(),
            loops: Vec::new(),
            returns: Vec::new(),
            failed_return: false,
            reads: Vec::new(),
            calls: Vec::new(),
            body: Vec::new(),
        }
    }
}

/// A variable declared at the top level of the script.
struct GlobalInfo<'a> {
    /// The first declaration.
    decl: &'a ast::LetStmt,
    /// Allocated among the script's locals when the name is declared once; functions
    /// cannot refer to a name declared several times (C5).
    local: Option<ir::LocalId>,
    ty: GlobalType,
}

#[derive(Clone, Copy)]
enum GlobalType {
    /// The script has not reached the declaration yet.
    Unknown,
    /// `None` when the declaration has an error.
    Known(Option<Type>),
}

struct Checker<'a> {
    diagnostics: Vec<Diagnostic>,
    /// The function being checked. The contexts it interrupted are on the Rust stack.
    ctx: Context,
    globals: HashMap<String, GlobalInfo<'a>>,
    /// The name of each global, by its local in the script.
    global_names: HashMap<ir::LocalId, String>,
    functions: Vec<FunctionInfo<'a>>,
    function_names: HashMap<String, usize>,
    /// The checked versions of the functions: one per function, or one per set of
    /// argument types for a generic function (C1).
    instances: Vec<Instance>,
    /// Calls made by the script, checked once every function is known (C3).
    script_calls: Vec<ScriptCall>,
    /// The calls that required checking a function body early, innermost last.
    demands: Vec<Span>,
}

impl<'a> Checker<'a> {
    fn new(module: &'a ast::Module) -> Checker<'a> {
        let mut checker = Checker {
            diagnostics: Vec::new(),
            ctx: Context::new(ContextKind::Script),
            globals: HashMap::new(),
            global_names: HashMap::new(),
            functions: Vec::new(),
            function_names: HashMap::new(),
            instances: Vec::new(),
            script_calls: Vec::new(),
            demands: Vec::new(),
        };
        checker.register_top_level(module);
        checker
    }

    /// Declares a local in the innermost block.
    fn declare(
        &mut self,
        name: &ast::Ident,
        ty: Option<Type>,
        mutable: bool,
        initialized: bool,
    ) -> ir::LocalId {
        self.check_hiding(name);
        let info = LocalInfo::variable(name, ty, mutable, initialized);
        // A global of the script uses the local allocated for it in advance.
        let id = match self.preallocated_global(name) {
            Some(id) => {
                self.ctx.locals[id.index()] = info;
                if let Some(global) = self.globals.get_mut(&name.name) {
                    global.ty = GlobalType::Known(ty);
                }
                id
            }
            None => self.push_local(info),
        };
        let scope = self.ctx.scopes.last_mut().expect("a scope is open");
        scope.names.insert(name.name.clone(), id);
        scope.declared.push(id);
        self.ctx.flow.set(id, if initialized { Assigned::Yes } else { Assigned::No });
        id
    }

    /// A name of an enclosing block cannot be declared again in an inner block (§6.5).
    /// In the script, the functions of the file are names of its top-level block.
    fn check_hiding(&mut self, name: &ast::Ident) {
        let (_, enclosing) = self.ctx.scopes.split_last().expect("a scope is open");
        let outer = enclosing.iter().rev().find_map(|scope| scope.names.get(&name.name));
        let (outer_span, help) = match outer {
            Some(&outer) => (
                self.ctx.locals[outer.index()].decl_span,
                format!(
                    "choose another name, or assign the existing variable without `let` or `var`: `{} = ...`",
                    name.name
                ),
            ),
            // At the top level, `register_top_level` reports it already.
            None if self.ctx.kind == ContextKind::Script && !enclosing.is_empty() => {
                let Some(&function) = self.function_names.get(&name.name) else { return };
                (self.functions[function].decl.name.span, "choose another name".to_string())
            }
            None => return,
        };
        let place = if enclosing.is_empty() { "the script" } else { "an enclosing block" };
        self.diagnostics.push(
            Diagnostic::error(format!("`{}` is already declared in {place}", name.name))
                .with_primary(name.span, "declared again here")
                .with_secondary(outer_span, "first declared here")
                .with_note("a name of an enclosing block cannot be declared again in an inner block (§6.5)")
                .with_help(help),
        );
    }

    /// The local allocated for a global, when `name` is its top-level declaration.
    fn preallocated_global(&self, name: &ast::Ident) -> Option<ir::LocalId> {
        if self.ctx.kind != ContextKind::Script || self.ctx.scopes.len() != 1 {
            return None;
        }
        let global = self.globals.get(&name.name)?;
        if global.decl.name.span == name.span { global.local } else { None }
    }

    /// A local introduced by the checker, invisible to the program.
    fn temporary(&mut self, ty: Type, span: Span) -> ir::LocalId {
        let id = self.push_local(LocalInfo {
            name: "%t".to_string(),
            ty: Some(ty),
            mutable: false,
            temporary: true,
            by_reference: false,
            decl_span: span,
            initialized: true,
            first_assignment: None,
            loop_variable: false,
        });
        self.ctx.flow.set(id, Assigned::Yes);
        id
    }

    fn push_local(&mut self, info: LocalInfo) -> ir::LocalId {
        self.ctx.locals.push(info);
        ir::LocalId(self.ctx.locals.len() as u32 - 1)
    }

    /// A local of the current function visible from here.
    fn lookup(&self, name: &str) -> Option<ir::LocalId> {
        self.ctx.scopes.iter().rev().find_map(|scope| scope.names.get(name).copied())
    }

    fn not_implemented(&mut self, span: Span, what: &str, section: &str) {
        self.diagnostics.push(Diagnostic::not_implemented(span, what, section));
    }

    fn finish(mut self) -> Checked {
        let mut diagnostics = std::mem::take(&mut self.diagnostics);
        // Function bodies are checked out of order: report in the order of the file,
        // once, even when several instances of a generic function find the same problem.
        let position = |diagnostic: &Diagnostic| {
            let primary = diagnostic.labels.iter().find(|label| label.primary).or(diagnostic.labels.first());
            primary.map(|label| (label.span.source, label.span.start, label.span.end))
        };
        diagnostics.sort_by_key(position);
        diagnostics.dedup_by(|a, b| a.message == b.message && position(a) == position(b));
        if diagnostics.iter().any(Diagnostic::is_fatal) {
            return Checked { program: None, diagnostics };
        }
        let script = std::mem::replace(&mut self.ctx, Context::new(ContextKind::Script));
        let main = ir::Function {
            name: "script".to_string(),
            params: 0,
            defaults: Vec::new(),
            ret: Type::None,
            locals: ir_locals(script.locals),
            body: script.body,
            span: None,
        };
        let functions = self.functions;
        let mut functions: Vec<ir::Function> =
            self.instances.into_iter().map(|instance| instance.into_ir(&functions)).collect();
        functions.push(main);
        let main = ir::FunctionId(functions.len() as u32 - 1);
        Checked { program: Some(ir::Program { functions, main }), diagnostics }
    }
}

fn ir_locals(locals: Vec<LocalInfo>) -> Vec<ir::Local> {
    locals
        .into_iter()
        .map(|info| ir::Local {
            name: info.name,
            ty: info.ty.expect("every local of a valid program has a type"),
            mutable: info.mutable,
            temporary: info.temporary,
            by_reference: info.by_reference,
            span: info.decl_span,
        })
        .collect()
}

fn typed(kind: ir::ExprKind, ty: Type, span: Span) -> ir::Expr {
    ir::Expr { kind, ty, span }
}

/// "an Int", "a List of Text": how a value of the type is named in messages.
fn article(ty: Type) -> String {
    match ty {
        Type::Int => "an Int".to_string(),
        Type::None => "`none`".to_string(),
        other => format!("a {other}"),
    }
}

#[cfg(test)]
mod tests;
