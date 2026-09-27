//! Computation at compile time (spec §21.1, D21, D82): `compile expr`. The checker
//! makes sure that the expression depends on nothing known only while the program runs,
//! and has no effect outside its values, even through the functions it calls. Once the
//! program is checked, the value is computed, and it replaces the expression (C77).

use std::collections::HashSet;

use lion_diagnostics::{Diagnostic, Span};
use lion_ir::{self as ir, Type};
use lion_syntax::ast;

use crate::parallel::Variable;
use crate::{Checker, article, typed};

/// The expression computed at compile time around the code being checked.
#[derive(Clone, Copy)]
pub(crate) struct CompileRegion {
    /// The locals declared from this one on belong to the expression.
    first_local: usize,
    span: Span,
}

/// An expression computed at compile time, with the functions it calls, checked once
/// every function is.
pub(crate) struct CompileCheck {
    calls: Vec<usize>,
    /// The globals of modules that the expression reads itself: `math.pi`.
    reads: Vec<ir::LocalId>,
    value: ir::Expr,
    span: Span,
}

impl Checker<'_> {
    /// `compile value` (§21.1).
    pub(crate) fn compile_expr(&mut self, inner: &ast::Expr, span: Span) -> Option<ir::Expr> {
        let region = CompileRegion { first_local: self.ctx.locals.len(), span };
        let first_call = self.ctx.calls.len();
        let first_read = self.ctx.reads.len();
        let outer = self.ctx.compile.replace(region);
        let value = self.expr(inner);
        self.ctx.compile = outer;
        let value = value?;
        if !self.embeddable(value.ty, &mut HashSet::new()) {
            self.diagnostics.push(
                Diagnostic::error(format!("`compile` gives a value written with literals, not {}", article(value.ty)))
                    .with_primary(inner.span, "")
                    .with_note("the program contains the value that `compile` computes: a function, a Domain or a task cannot be written in it (§21.1, C77)"),
            );
            return None;
        }
        let calls = self.ctx.calls[first_call..].to_vec();
        let reads = self.ctx.reads.get(first_read..).unwrap_or_default().to_vec();
        self.compile_checks.push(CompileCheck { calls, reads, value: value.clone(), span });
        let ty = value.ty;
        Some(typed(ir::ExprKind::Compile(Box::new(value)), ty, span))
    }

    /// A variable read by `compile`, declared outside it: its value is known only while
    /// the program runs (C77).
    pub(crate) fn check_compile_read(&mut self, variable: Variable, name: &str, span: Span) {
        let Some(region) = self.ctx.compile else { return };
        if let Variable::Local(local) = variable
            && local.index() >= region.first_local
        {
            return;
        }
        self.diagnostics.push(
            Diagnostic::error(format!(
                "`compile` cannot read `{name}`: its value is known only while the program runs"
            ))
            .with_primary(span, "")
            .with_secondary(region.span, "computed while the program is compiled")
            .with_note("`compile` computes a value from literals and functions (§21.1, C77)")
            .with_help("write the value in the expression, or remove `compile`"),
        );
    }

    /// What `compile` runs has no effect: no output nor input, no change of a variable
    /// of the script, no reading of one (§21.1, D82).
    pub(crate) fn check_compile_regions(&mut self) {
        for check in std::mem::take(&mut self.compile_checks) {
            let mut direct = None;
            ir::visit::exprs_in(&check.value, &mut |expr| {
                if let ir::ExprKind::CallBuiltin { builtin, .. } = expr.kind
                    && let Some(effect) = builtin.outside_effect()
                {
                    direct.get_or_insert(effect);
                }
            });
            if let Some(effect) = direct {
                self.compile_error(check.span, format!("`compile` cannot run what {effect}"));
                continue;
            }
            if let Some(&global) = check.reads.iter().find(|&&global| !self.constant_of_module(global)) {
                let name = self.global_display_name(global);
                self.compile_error(
                    check.span,
                    format!("`compile` cannot read `{name}`, a variable of the program"),
                );
                continue;
            }
            if let Some(message) = check.reads.iter().find_map(|&global| self.module_effect(global)) {
                self.compile_error(check.span, message);
                continue;
            }
            for &instance in &check.calls {
                let name = self.instance_display_name(instance);
                let through = |checker: &Self, current: usize| {
                    if current == instance {
                        String::new()
                    } else {
                        format!(" (through `{}`)", checker.instance_display_name(current))
                    }
                };
                let used = self.globals_used_by(instance);
                if let Some(&(global, current)) =
                    used.iter().find(|(global, _)| !self.constant_of_module(*global))
                {
                    let via = through(self, current);
                    let global = self.global_display_name(global);
                    self.compile_error(
                        check.span,
                        format!("`compile` cannot run `{name}`, which uses `{global}`{via}, a variable of the program"),
                    );
                    break;
                }
                if let Some(message) = used.iter().find_map(|&(global, _)| self.module_effect(global)) {
                    self.compile_error(check.span, message);
                    break;
                }
                let effect = |builtin: ir::Builtin| builtin.outside_effect().is_some();
                if let Some((builtin, current)) = self.builtin_called_by(instance, &effect) {
                    let via = through(self, current);
                    let effect = builtin.outside_effect().expect("found above");
                    self.compile_error(
                        check.span,
                        format!("`compile` cannot run `{name}`, which {effect}{via}"),
                    );
                    break;
                }
            }
        }
    }

    /// A `let` of a module: known once the module gets its values, which `compile`
    /// does first (D81).
    fn constant_of_module(&self, global: ir::LocalId) -> bool {
        self.global_modules.contains_key(&global) && !self.global_info(global).decl.mutable
    }

    /// Why the module of `global` cannot get its values while the program is compiled.
    fn module_effect(&self, global: ir::LocalId) -> Option<String> {
        let &module = self.global_modules.get(&global)?;
        let init = self.modules[module].init?;
        let effect = |builtin: ir::Builtin| builtin.outside_effect().is_some();
        let (builtin, _) = self.builtin_called_by(init, &effect)?;
        let name = &self.modules[module].name;
        Some(format!(
            "`compile` cannot read `{}`: the module `{name}` {} when it gets its values",
            self.global_display_name(global),
            builtin.outside_effect().expect("found above")
        ))
    }

    /// `x`, or `geometry.x` for a global of a module.
    pub(crate) fn global_display_name(&self, global: ir::LocalId) -> String {
        let name = &self.global_names[&global];
        match self.global_modules.get(&global) {
            Some(&module) => {
                let path = &self.modules[module].name;
                format!("{}.{name}", path.rsplit('.').next().unwrap_or(path))
            }
            None => name.clone(),
        }
    }

    fn compile_error(&mut self, span: Span, message: String) {
        self.diagnostics.push(
            Diagnostic::error(message)
                .with_primary(span, "")
                .with_note("`compile` computes its value while the program is compiled: it reads nothing from outside and changes nothing (§21.1, D82)"),
        );
    }

    /// Whether a value of this type can be written in the program as literals.
    fn embeddable(&self, ty: Type, seen: &mut HashSet<Type>) -> bool {
        if !seen.insert(ty) {
            return true;
        }
        ty.members().into_iter().all(|member| match member {
            Type::Int
            | Type::Float
            | Type::Rational
            | Type::Bool
            | Type::Text
            | Type::None
            | Type::Error
            | Type::Range
            | Type::Enum(_) => true,
            Type::List(element) | Type::Set(element) => self.embeddable(element.get(), seen),
            Type::Map(parts) => parts.elements().into_iter().all(|part| self.embeddable(part, seen)),
            Type::Tuple(elements) => {
                elements.elements().into_iter().all(|element| self.embeddable(element, seen))
            }
            Type::Struct(structure) => {
                let fields: Vec<Option<Type>> =
                    self.structs[self.struct_index(structure)].fields.iter().map(|field| field.ty).collect();
                fields.into_iter().all(|field| field.is_some_and(|ty| self.embeddable(ty, seen)))
            }
            Type::Fun(_)
            | Type::Domain(_)
            | Type::Task(_)
            | Type::Var(_)
            | Type::Trait(_)
            | Type::Union(_) => false,
        })
    }
}
