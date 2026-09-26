//! Name resolution and type checking: from the syntax tree to the typed IR.
//!
//! Every compile error that is not a syntax error comes from here, whatever the
//! execution mode (spec §22.1). The checker reports all the problems it finds and
//! builds an [`ir::Program`] only when there is none.

mod expr;
mod flow;
mod names;
mod stmt;
mod types;

use std::collections::HashMap;

use lion_diagnostics::{Diagnostic, Span};
use lion_ir::{self as ir, Type};
use lion_syntax::ast;

use crate::flow::{Assigned, Flow};

pub struct Checked {
    /// Present only when there are no errors.
    pub program: Option<ir::Program>,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn check(module: &ast::Module) -> Checked {
    let mut checker = Checker {
        diagnostics: Vec::new(),
        locals: Vec::new(),
        scopes: vec![Scope::default()],
        flow: Flow::start(),
        loops: Vec::new(),
    };
    let body = checker.stmts(&module.stmts);
    checker.close_scope();
    checker.finish(body)
}

/// What the checker knows about a local.
struct LocalInfo {
    name: String,
    /// `None` when its declaration had an error, already reported.
    ty: Option<Type>,
    mutable: bool,
    temporary: bool,
    decl_span: Span,
    /// Declared with a value (`let x = 1`) rather than without (`let x in Int`).
    initialized: bool,
    /// The first assignment met, for messages.
    first_assignment: Option<Span>,
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

struct Checker {
    diagnostics: Vec<Diagnostic>,
    locals: Vec<LocalInfo>,
    /// The blocks being checked, innermost last. Declaring a name again in the same
    /// block hides the old binding (§6.3); declaring a name of an enclosing block in an
    /// inner block is an error (§6.5).
    scopes: Vec<Scope>,
    /// What is known at the current point of the program.
    flow: Flow,
    /// The loops being checked, innermost last.
    loops: Vec<LoopExits>,
}

impl Checker {
    fn declare(
        &mut self,
        name: &ast::Ident,
        ty: Option<Type>,
        mutable: bool,
        initialized: bool,
    ) -> ir::LocalId {
        let (_, enclosing) = self.scopes.split_last().expect("a scope is open");
        if let Some(&outer) = enclosing.iter().rev().find_map(|scope| scope.names.get(&name.name)) {
            let outer = &self.locals[outer.index()];
            self.diagnostics.push(
                Diagnostic::error(format!("`{}` is already declared in an enclosing block", name.name))
                    .with_primary(name.span, "declared again here")
                    .with_secondary(outer.decl_span, "first declared here")
                    .with_note("a name of an enclosing block cannot be declared again in an inner block (§6.5)")
                    .with_help(format!(
                        "choose another name, or assign the existing variable without `let` or `var`: `{} = ...`",
                        name.name
                    )),
            );
        }
        let id = self.push_local(LocalInfo {
            name: name.name.clone(),
            ty,
            mutable,
            temporary: false,
            decl_span: name.span,
            initialized,
            first_assignment: None,
        });
        let scope = self.scopes.last_mut().expect("a scope is open");
        scope.names.insert(name.name.clone(), id);
        scope.declared.push(id);
        self.flow.set(id, if initialized { Assigned::Yes } else { Assigned::No });
        id
    }

    /// A local introduced by the checker, invisible to the program.
    fn temporary(&mut self, ty: Type, span: Span) -> ir::LocalId {
        let id = self.push_local(LocalInfo {
            name: "%t".to_string(),
            ty: Some(ty),
            mutable: false,
            temporary: true,
            decl_span: span,
            initialized: true,
            first_assignment: None,
        });
        self.flow.set(id, Assigned::Yes);
        id
    }

    fn push_local(&mut self, info: LocalInfo) -> ir::LocalId {
        self.locals.push(info);
        ir::LocalId(self.locals.len() as u32 - 1)
    }

    fn lookup(&self, name: &str) -> Option<ir::LocalId> {
        self.scopes.iter().rev().find_map(|scope| scope.names.get(name).copied())
    }

    fn not_implemented(&mut self, span: Span, what: &str, section: &str) {
        self.diagnostics.push(Diagnostic::not_implemented(span, what, section));
    }

    fn finish(self, body: Vec<ir::Stmt>) -> Checked {
        if self.diagnostics.iter().any(Diagnostic::is_fatal) {
            return Checked { program: None, diagnostics: self.diagnostics };
        }
        let locals = self
            .locals
            .into_iter()
            .map(|info| ir::Local {
                name: info.name,
                ty: info.ty.expect("every local of a valid program has a type"),
                mutable: info.mutable,
                temporary: info.temporary,
                span: info.decl_span,
            })
            .collect();
        Checked { program: Some(ir::Program { locals, body }), diagnostics: self.diagnostics }
    }
}

fn typed(kind: ir::ExprKind, ty: Type, span: Span) -> ir::Expr {
    ir::Expr { kind, ty, span }
}

/// "an Int", "a Text": how a value of the type is named in messages.
fn article(ty: Type) -> &'static str {
    match ty {
        Type::Int => "an Int",
        Type::Float => "a Float",
        Type::Bool => "a Bool",
        Type::Text => "a Text",
        Type::None => "`none`",
    }
}

#[cfg(test)]
mod tests;
