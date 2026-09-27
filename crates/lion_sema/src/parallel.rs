//! Data parallelism (spec §19.2, §19.3): `parallel [...]`, `parallel {...}` and
//! `parallel for`.
//!
//! What runs in parallel must not change anything outside it, even through the
//! functions it calls: a variable declared outside is not assigned, and a called
//! function modifies no global, directly or through its own calls (§11.5). This version
//! computes the parallel parts one after the other, which gives the same results
//! (C60); the use of several cores comes with step 6 of the roadmap (§28).

use std::collections::HashSet;

use lion_diagnostics::{Diagnostic, Span};
use lion_ir as ir;
use lion_syntax::ast;

use crate::Checker;

/// The part of the function being checked that runs in parallel.
#[derive(Clone, Copy)]
pub(crate) struct Parallel {
    /// The locals declared from this one on belong to the parallel part.
    first_local: usize,
    /// Where `ctx.calls` stood when the part started.
    first_call: usize,
    pub(crate) span: Span,
}

impl Parallel {
    /// Whether the local was declared before the part.
    pub(crate) fn is_outside(&self, local: ir::LocalId) -> bool {
        local.index() < self.first_local
    }
}

/// A variable, as a name resolves to it.
#[derive(Clone, Copy)]
pub(crate) enum Variable {
    /// Of the function being checked, or of the script.
    Local(ir::LocalId),
    /// A global seen from a function.
    Global(ir::LocalId),
}

/// A checked parallel part: the functions it calls, checked once all are known.
pub(crate) struct ParallelRegion {
    calls: Vec<usize>,
    span: Span,
    /// The property of a Domain, which may be tested in parallel (C74).
    domain: bool,
}

impl Checker<'_> {
    /// Checks `check` as a part that runs in parallel.
    pub(crate) fn in_parallel<T>(&mut self, span: Span, check: impl FnOnce(&mut Self) -> T) -> T {
        let region = Parallel { first_local: self.ctx.locals.len(), first_call: self.ctx.calls.len(), span };
        let outer = self.ctx.parallel.replace(region);
        let result = check(self);
        let calls = self.ctx.calls[region.first_call..].to_vec();
        self.parallel_regions.push(ParallelRegion { calls, span, domain: false });
        self.ctx.parallel = outer;
        result
    }

    /// The function of a Domain: it follows the rules of a parallel part, since a Domain
    /// may be tested in one (C74).
    pub(crate) fn check_like_parallel(&mut self, instance: usize, span: Span) {
        self.parallel_regions.push(ParallelRegion { calls: vec![instance], span, domain: true });
    }

    /// `parallel [...]` or `parallel {...}`: a comprehension (§19.2).
    pub(crate) fn parallel_expr(&mut self, inner: &ast::Expr, span: Span) -> Option<ir::Expr> {
        let comprehension = match &inner.kind {
            ast::ExprKind::List(elements) | ast::ExprKind::Set(elements) => elements
                .iter()
                .any(|element| matches!(&element.kind, ast::ExprKind::Binary { op: ast::BinaryOp::In, .. })),
            _ => false,
        };
        if !comprehension {
            self.diagnostics.push(
                Diagnostic::error("`parallel` applies to a comprehension or to a `for` loop")
                    .with_primary(inner.span, "")
                    .with_help("for instance `parallel [f(x), x in values]` (§19.2)"),
            );
            return None;
        }
        let value = self.in_parallel(span, |checker| checker.expr(inner))?;
        Some(ir::Expr { span, ..value })
    }

    /// Whether assigning `local` from here changes a variable outside the parallel part;
    /// reports it.
    pub(crate) fn changes_outside_parallel(&mut self, variable: Variable, name: &str, span: Span) -> bool {
        let Some(region) = self.ctx.parallel else { return false };
        // A global seen from a function is always outside.
        if let Variable::Local(local) = variable
            && !region.is_outside(local)
        {
            return false;
        }
        // A lock protects each access to a `shared synced` object (§19.3).
        if let Some(sharing) = self.sharing_of(variable) {
            return !sharing.synced;
        }
        self.diagnostics.push(
            Diagnostic::error(format!("a parallel part cannot change `{name}`, which is declared outside it"))
                .with_primary(span, "")
                .with_secondary(region.span, "runs in parallel")
                .with_note("what runs in parallel changes nothing outside it (§19.3)")
                .with_help("compute a value per element and gather the values with the comprehension; or declare the variable inside the loop"),
        );
        true
    }

    /// The functions called in parallel must not modify globals, directly or through the
    /// functions they call, nor use a shared object that is not `synced` (§19.3).
    pub(crate) fn check_parallel_regions(&mut self) {
        let regions = std::mem::take(&mut self.parallel_regions);
        if regions.is_empty() {
            return;
        }
        for region in regions {
            let mut reported = HashSet::new();
            for &instance in &region.calls {
                self.check_shared_uses(instance, &region, &mut reported);
                let Some((global, through)) = self.modified_global_of(instance) else { continue };
                let name = self.instance_display_name(instance);
                if !reported.insert((name.clone(), global)) {
                    continue;
                }
                let global_name = self.global_names[&global].clone();
                let via = match through {
                    Some(through) if through != instance => {
                        format!(" (through `{}`)", self.instance_display_name(through))
                    }
                    _ => String::new(),
                };
                if region.domain {
                    self.diagnostics.push(
                        Diagnostic::error(format!(
                            "the property of a Domain cannot modify `{global_name}`{via}"
                        ))
                        .with_primary(region.span, "")
                        .with_note("a Domain may be tested anywhere, even in a parallel part: its property changes nothing (C74)"),
                    );
                    continue;
                }
                self.diagnostics.push(
                    Diagnostic::error(format!(
                        "`{name}` modifies `{global_name}`{via}, so it cannot run in parallel"
                    ))
                    .with_primary(region.span, "")
                    .with_note("what runs in parallel changes nothing outside it (§19.3)")
                    .with_help(format!(
                        "use a local variable, or declare `{global_name}` as `shared synced` (§17.2)"
                    )),
                );
            }
        }
    }

    /// The shared objects that `instance` uses, directly or through its calls: refused
    /// unless `synced` (§19.3).
    fn check_shared_uses(
        &mut self,
        instance: usize,
        region: &ParallelRegion,
        reported: &mut HashSet<(String, ir::LocalId)>,
    ) {
        let span = region.span;
        for (global, current) in self.globals_used_by(instance) {
            let Some(sharing) = self.global_sharing.get(&global).copied() else { continue };
            if sharing.synced {
                self.synced_used.insert(sharing.origin);
                continue;
            }
            let name = self.instance_display_name(instance);
            if !reported.insert((name.clone(), global)) {
                continue;
            }
            let global_name = self.global_names[&global].clone();
            let via = if current == instance {
                String::new()
            } else {
                format!(" (through `{}`)", self.instance_display_name(current))
            };
            let message = if region.domain {
                format!("the property of a Domain cannot use `{global_name}`{via}, which is shared")
            } else {
                format!("`{name}` uses `{global_name}`{via}, which is shared, so it cannot run in parallel")
            };
            self.diagnostics.push(
                Diagnostic::error(message)
                    .with_primary(span, "")
                    .with_secondary(sharing.origin, "shared here")
                    .with_note("two tasks could use the object at the same time (§19.3)")
                    .with_help(format!(
                        "declare `{global_name}` as `shared synced`: a lock then protects each access"
                    )),
            );
        }
    }
}
