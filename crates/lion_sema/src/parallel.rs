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
    span: Span,
}

/// A checked parallel part: the functions it calls, checked once all are known.
pub(crate) struct ParallelRegion {
    calls: Vec<usize>,
    span: Span,
}

impl Checker<'_> {
    /// Checks `check` as a part that runs in parallel.
    pub(crate) fn in_parallel<T>(&mut self, span: Span, check: impl FnOnce(&mut Self) -> T) -> T {
        let region = Parallel { first_local: self.ctx.locals.len(), first_call: self.ctx.calls.len(), span };
        let outer = self.ctx.parallel.replace(region);
        let result = check(self);
        let calls = self.ctx.calls[region.first_call..].to_vec();
        self.parallel_regions.push(ParallelRegion { calls, span });
        self.ctx.parallel = outer;
        result
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
    pub(crate) fn changes_outside_parallel(
        &mut self,
        local: Option<ir::LocalId>,
        name: &str,
        span: Span,
    ) -> bool {
        let Some(region) = self.ctx.parallel else { return false };
        // A global seen from a function is always outside.
        if local.is_some_and(|local| local.index() >= region.first_local) {
            return false;
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
    /// functions they call (§19.3).
    pub(crate) fn check_parallel_regions(&mut self) {
        let regions = std::mem::take(&mut self.parallel_regions);
        if regions.is_empty() {
            return;
        }
        for region in regions {
            let mut reported = HashSet::new();
            for &instance in &region.calls {
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
}
