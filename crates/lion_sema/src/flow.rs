//! Definite assignment (spec §6.1): which locals have a value at each point.
//!
//! The checker walks statements in order and keeps one [`Flow`] for the current point.
//! Where paths meet (after an `if`, after a loop), their flows are joined: a local has
//! a value only if it has one on every path. Code that cannot be reached has a
//! vacuous flow, which never reports anything.

use lion_diagnostics::{Diagnostic, Span};
use lion_ir as ir;

use crate::Checker;

/// Whether a local has received a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Assigned {
    No,
    /// On some paths only.
    Maybe,
    Yes,
}

#[derive(Clone, Debug)]
pub(crate) struct Flow {
    /// Indexed by local. Locals beyond the end have no value yet.
    assigned: Vec<Assigned>,
    reachable: bool,
}

impl Flow {
    pub(crate) fn start() -> Flow {
        Flow { assigned: Vec::new(), reachable: true }
    }

    /// The flow after `return`, `break` or `continue`.
    pub(crate) fn unreachable() -> Flow {
        Flow { assigned: Vec::new(), reachable: false }
    }

    pub(crate) fn is_reachable(&self) -> bool {
        self.reachable
    }

    /// Unreachable code vacuously has every value.
    pub(crate) fn get(&self, local: ir::LocalId) -> Assigned {
        if !self.reachable {
            return Assigned::Yes;
        }
        self.assigned.get(local.index()).copied().unwrap_or(Assigned::No)
    }

    pub(crate) fn set(&mut self, local: ir::LocalId, value: Assigned) {
        if self.assigned.len() <= local.index() {
            self.assigned.resize(local.index() + 1, Assigned::No);
        }
        self.assigned[local.index()] = value;
    }

    /// The flow where two paths meet.
    pub(crate) fn join(self, other: Flow) -> Flow {
        match (self.reachable, other.reachable) {
            (false, _) => other,
            (_, false) => self,
            _ => {
                let length = self.assigned.len().max(other.assigned.len());
                let at = |flow: &Flow, index| flow.assigned.get(index).copied().unwrap_or(Assigned::No);
                let assigned = (0..length)
                    .map(|index| match (at(&self, index), at(&other, index)) {
                        (a, b) if a == b => a,
                        _ => Assigned::Maybe,
                    })
                    .collect();
                Flow { assigned, reachable: true }
            }
        }
    }
}

impl Checker {
    /// Reports a read of `local` where it may have no value.
    pub(crate) fn check_has_value(&mut self, local: ir::LocalId, span: Span) -> bool {
        let state = self.flow.get(local);
        if state == Assigned::Yes {
            return true;
        }
        let info = &self.locals[local.index()];
        let name = &info.name;
        let error = if state == Assigned::No {
            Diagnostic::error(format!("`{name}` is used before it has a value"))
                .with_primary(span, "used here")
                .with_secondary(info.decl_span, "declared here without a value")
                .with_help(format!("give `{name}` a value before this line (§6.1)"))
        } else {
            Diagnostic::error(format!("`{name}` may not have a value here"))
                .with_primary(span, "used here")
                .with_secondary(info.decl_span, "declared here without a value")
                .with_note(
                    "it receives a value on some of the paths that lead here, not on all of them (§6.1)",
                )
        };
        self.diagnostics.push(error);
        false
    }

    /// A constant changes at most once: a `let` with a value never, a `let` without a
    /// value exactly once on every path (§6.1).
    pub(crate) fn check_assignable(&mut self, local: ir::LocalId, span: Span) -> bool {
        let info = &self.locals[local.index()];
        if info.mutable {
            return true;
        }
        let name = &info.name;
        if info.initialized {
            self.diagnostics.push(
                Diagnostic::error(format!("cannot assign to the constant `{name}`"))
                    .with_primary(span, "")
                    .with_secondary(info.decl_span, "declared with `let`")
                    .with_help(format!("to change `{name}`, declare it with `var` (§6)")),
            );
            return false;
        }
        let mut error = match self.flow.get(local) {
            Assigned::No => return true,
            // Also the vacuous state of unreachable code.
            Assigned::Yes if !self.flow.is_reachable() => return true,
            Assigned::Yes => Diagnostic::error(format!("the constant `{name}` already has a value"))
                .with_primary(span, "second assignment"),
            Assigned::Maybe => Diagnostic::error(format!("the constant `{name}` may already have a value"))
                .with_primary(span, "this assignment may run after another one, for instance in a loop"),
        };
        if let Some(first) = info.first_assignment {
            error = error.with_secondary(first, "earlier assignment");
        }
        let error = error
            .with_note("a `let` declared without a value receives exactly one assignment (§6.1)")
            .with_help(format!("to change `{name}`, declare it with `var`"));
        self.diagnostics.push(error);
        false
    }

    /// At the end of a block: each `let` declared in it without a value must have
    /// received one on every path (§6.1).
    pub(crate) fn check_constants_assigned(&mut self, declared: &[ir::LocalId]) {
        if !self.flow.is_reachable() {
            return;
        }
        for &local in declared {
            let info = &self.locals[local.index()];
            if info.mutable || info.initialized || info.temporary || info.ty.is_none() {
                continue;
            }
            let message = match (self.flow.get(local), info.first_assignment) {
                (Assigned::Yes, _) => continue,
                (Assigned::No, None) => format!("the constant `{}` never receives a value", info.name),
                _ => format!("the constant `{}` does not receive a value on every path", info.name),
            };
            self.diagnostics.push(
                Diagnostic::error(message)
                    .with_primary(info.decl_span, "declared here without a value")
                    .with_note("a `let` declared without a value receives exactly one assignment (§6.1)"),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(index: u32) -> ir::LocalId {
        ir::LocalId(index)
    }

    #[test]
    fn join_keeps_what_every_path_agrees_on() {
        let mut a = Flow::start();
        a.set(local(0), Assigned::Yes);
        a.set(local(1), Assigned::Yes);
        let mut b = Flow::start();
        b.set(local(0), Assigned::Yes);
        let joined = a.join(b);
        assert_eq!(joined.get(local(0)), Assigned::Yes);
        assert_eq!(joined.get(local(1)), Assigned::Maybe);
        assert_eq!(joined.get(local(2)), Assigned::No);
    }

    #[test]
    fn unreachable_paths_do_not_count() {
        let mut a = Flow::start();
        a.set(local(0), Assigned::Yes);
        let joined = Flow::unreachable().join(a.clone());
        assert_eq!(joined.get(local(0)), Assigned::Yes);
        assert!(joined.is_reachable());
        assert_eq!(Flow::unreachable().get(local(5)), Assigned::Yes);
    }
}
