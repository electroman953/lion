//! Modules (spec §20): each file has its own names. `use m` makes the names of the
//! module `m` reachable as `m.name`: imported names stay qualified (D84).
//!
//! The checker keeps the tables of names of the module being checked at hand, in
//! `Checker::tables`; those of the other modules wait in their `ModuleInfo`. Checking
//! a function, a structure or a type of another module first enters that module.

use std::collections::HashMap;

use lion_diagnostics::{Diagnostic, Span};
use lion_ir::{self as ir, Type};
use lion_syntax::ast;

use crate::enums::NamedType;
use crate::{Checker, ContextKind, GlobalInfo, GlobalType, typed};

/// A file given to the checker: the script, or a module that a file uses.
pub struct Source<'a> {
    /// The path written after `use` (`shapes.circle`), empty for the script.
    pub name: String,
    pub module: &'a ast::Module,
    /// A module of the standard library, which may declare `foreign "lion"` functions.
    pub standard: bool,
}

/// The names declared at the top level of a file.
#[derive(Default)]
pub(crate) struct Tables<'a> {
    pub(crate) globals: HashMap<String, GlobalInfo<'a>>,
    pub(crate) function_names: HashMap<String, usize>,
    pub(crate) struct_names: HashMap<String, usize>,
    pub(crate) named_types: HashMap<String, NamedType<'a>>,
    /// Where each type of the file is declared.
    pub(crate) type_spans: HashMap<String, Span>,
}

pub(crate) struct ModuleInfo<'a> {
    /// The path of the module, empty for the script.
    pub(crate) name: String,
    pub(crate) ast: &'a ast::Module,
    pub(crate) standard: bool,
    /// The modules named by `use`, by the name that reaches them: the last part of
    /// their path (C61).
    pub(crate) imports: HashMap<String, usize>,
    /// Its names, while another module is being checked.
    pub(crate) tables: Tables<'a>,
    /// The function that gives the globals of the module their values (D81).
    pub(crate) init: Option<usize>,
}

impl<'a> Checker<'a> {
    /// Makes `module` the module being checked; returns the one it was.
    pub(crate) fn enter_module(&mut self, module: usize) -> usize {
        let previous = self.module;
        if module != previous {
            std::mem::swap(&mut self.tables, &mut self.modules[previous].tables);
            std::mem::swap(&mut self.tables, &mut self.modules[module].tables);
            self.module = module;
        }
        previous
    }

    /// The names of a module.
    pub(crate) fn table_of(&self, module: usize) -> &Tables<'a> {
        if module == self.module { &self.tables } else { &self.modules[module].tables }
    }

    /// The information of a global, from its local in the script.
    pub(crate) fn global_info(&self, local: ir::LocalId) -> &GlobalInfo<'a> {
        let module = self.global_modules.get(&local).copied().unwrap_or(0);
        &self.table_of(module).globals[&self.global_names[&local]]
    }

    /// The modules that `use` statements name, by the name that reaches them.
    pub(crate) fn resolve_imports(&mut self, module: usize) {
        let ast = self.modules[module].ast;
        let mut imports: HashMap<String, usize> = HashMap::new();
        let mut spans: HashMap<String, Span> = HashMap::new();
        for stmt in &ast.stmts {
            let ast::StmtKind::Use(path) = &stmt.kind else { continue };
            let full: Vec<&str> = path.iter().map(|name| name.name.as_str()).collect();
            let full = full.join(".");
            let Some(target) = self.modules.iter().position(|info| info.name == full) else {
                // The driver reports the modules it cannot find.
                continue;
            };
            let last = path.last().expect("a path has a name");
            if let Some(&first) = spans.get(&last.name) {
                self.diagnostics.push(
                    Diagnostic::error(format!("two modules are reached as `{}`", last.name))
                        .with_primary(last.span, "")
                        .with_secondary(first, "first used here")
                        .with_note("a module is reached by the last part of its path (C61)"),
                );
                continue;
            }
            spans.insert(last.name.clone(), last.span);
            imports.insert(last.name.clone(), target);
        }
        self.modules[module].imports = imports;
    }

    /// The module that `name` reaches from the module being checked, unless a variable
    /// or a function of that name hides it.
    pub(crate) fn imported_module(&self, name: &str) -> Option<usize> {
        let module = *self.modules[self.module].imports.get(name)?;
        (!self.is_known(name)).then_some(module)
    }

    /// Whether a module may see the functions, the globals and the methods of `owner`.
    pub(crate) fn sees(&self, owner: usize) -> bool {
        owner == self.module || self.modules[self.module].imports.values().any(|&module| module == owner)
    }

    /// `m.f(...)`: a function of a module (§20.2).
    pub(crate) fn module_call(
        &mut self,
        module: usize,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> Option<ir::Expr> {
        let Some(&function) = self.table_of(module).function_names.get(&name.name) else {
            self.no_member(module, name);
            return None;
        };
        if let Some(private) = self.functions[function].decl.private {
            self.private_error(module, name, private);
            return None;
        }
        self.call_function(function, name.span, args, span)
    }

    /// `m.x`: a global of a module, read after the module is initialized (D81).
    pub(crate) fn module_global(&mut self, module: usize, name: &ast::Ident, span: Span) -> Option<ir::Expr> {
        let Some(global) = self.table_of(module).globals.get(&name.name) else {
            self.no_member(module, name);
            return None;
        };
        let (decl, local, ty) = (global.decl, global.local, global.ty);
        if let Some(private) = decl.private {
            self.private_error(module, name, private);
            return None;
        }
        let local = local?;
        let GlobalType::Known(Some(ty)) = ty else { return None };
        self.ctx.reads.push(local);
        let read = typed(ir::ExprKind::Global(local), ty, span);
        let init = ir::Stmt::InitModule { module: module as u32 };
        Some(typed(ir::ExprKind::Block { stmts: vec![init], value: Box::new(read) }, ty, span))
    }

    /// The struct or the type `name` of a module, for `m.Type`.
    pub(crate) fn module_type(&mut self, module: usize, name: &ast::Ident) -> Option<Type> {
        let tables = self.table_of(module);
        if let Some(&index) = tables.struct_names.get(&name.name) {
            return Some(Type::Struct(self.structs[index].id));
        }
        if tables.named_types.contains_key(&name.name) {
            let previous = self.enter_module(module);
            let ty = self.defined_type(&name.name, name.span);
            self.enter_module(previous);
            return ty.flatten();
        }
        self.no_member(module, name);
        None
    }

    fn no_member(&mut self, module: usize, name: &ast::Ident) {
        let module_name = self.modules[module].name.clone();
        self.diagnostics.push(
            Diagnostic::error(format!("the module `{module_name}` has no `{}`", name.name))
                .with_primary(name.span, ""),
        );
    }

    fn private_error(&mut self, module: usize, name: &ast::Ident, private: Span) {
        let module_name = self.modules[module].name.clone();
        let mut error = Diagnostic::error(format!("`{module_name}.{}` is private", name.name))
            .with_primary(name.span, "")
            .with_note("`private` limits a declaration to its file (§20.3, D10)");
        if !self.modules[module].standard {
            error = error.with_secondary(private, "declared `private` here");
        }
        self.diagnostics.push(error);
    }

    /// A module contains only declarations (§20.2, D31).
    pub(crate) fn check_module_statements(&mut self, module: usize) {
        let ast = self.modules[module].ast;
        for stmt in &ast.stmts {
            let problem = match &stmt.kind {
                ast::StmtKind::Fun(_)
                | ast::StmtKind::Struct(_)
                | ast::StmtKind::TypeDef(_)
                | ast::StmtKind::Trait(_)
                | ast::StmtKind::Use(_) => {
                    continue;
                }
                ast::StmtKind::Let(decl) if decl.value.is_some() => continue,
                ast::StmtKind::Let(decl) => Diagnostic::error(format!(
                    "the variable `{}` of a module needs a value where it is declared",
                    decl.name.name
                ))
                .with_primary(decl.name.span, "")
                .with_note("a module runs no statements, so nothing else can give it one (§20.2, C62)"),
                _ => Diagnostic::error("a module contains only declarations")
                    .with_primary(stmt.span, "this runs")
                    .with_note("only the file that is run executes statements (§20.2, D31)"),
            };
            self.diagnostics.push(problem);
        }
    }

    /// The globals of a module get their values in a function of their own, run just
    /// before the first use of the module (D81).
    pub(crate) fn check_module_init(&mut self, module: usize) {
        let ast = self.modules[module].ast;
        let decls: Vec<&'a ast::LetStmt> = ast
            .stmts
            .iter()
            .filter_map(|stmt| match &stmt.kind {
                ast::StmtKind::Let(decl) => Some(decl),
                _ => None,
            })
            .collect();
        // Registered with the functions, so that their bodies call it (D81).
        let Some(instance) = self.modules[module].init else { return };
        let previous = self.enter_module(module);
        let interrupted = std::mem::replace(&mut self.ctx, crate::Context::new(ContextKind::Init(module)));
        let mut body = Vec::new();
        let mut types = Vec::new();
        for decl in decls {
            let Some(value) = &decl.value else { continue };
            let annotation = decl.annotation.as_ref().and_then(|ty| self.resolve_type(ty));
            let checked = match annotation {
                Some(expected) => self.expr_expecting(value, expected).and_then(|checked| {
                    self.coerce(checked, expected, Some((decl.name.span, "declared here".to_string())))
                }),
                None if decl.annotation.is_some() => None,
                None => self.expr(value),
            };
            let ty = checked.as_ref().map(|value| value.ty);
            let Some(global) = self.tables.globals.get_mut(&decl.name.name) else { continue };
            if global.decl.name.span != decl.name.span {
                continue;
            }
            global.ty = GlobalType::Known(ty);
            let local = global.local;
            if let Some(local) = local {
                types.push((local, ty));
            }
            if let (Some(local), Some(value)) = (local, checked) {
                body.push(ir::Stmt::Assign { place: ir::Place::Global(local), value });
            }
        }
        let ctx = std::mem::replace(&mut self.ctx, interrupted);
        self.finish_synthetic(instance, Vec::new(), ctx.locals, body, ctx.calls);
        self.enter_module(previous);
        // The globals of modules are registers of the script, which the check of the
        // script (§20.1) is running.
        for (local, ty) in types {
            self.ctx.locals[local.index()].ty = ty;
            self.ctx.flow.set(local, crate::flow::Assigned::Yes);
        }
    }
}
