//! Narrowing (spec §7.4): after a test such as `x in None` or `x != none`, the checker
//! knows a narrower type for `x` where the test holds, and where it fails.

use lion_ir::{self as ir, Type};

use crate::Checker;

/// What a condition teaches about locals, when it is true and when it is false.
#[derive(Default)]
pub(crate) struct Facts {
    pub(crate) when_true: Vec<(ir::LocalId, Type)>,
    pub(crate) when_false: Vec<(ir::LocalId, Type)>,
}

impl Facts {
    fn swapped(self) -> Facts {
        Facts { when_true: self.when_false, when_false: self.when_true }
    }
}

impl Checker<'_> {
    /// The type of a local here: narrowed by the flow, or declared.
    pub(crate) fn local_type(&self, local: ir::LocalId) -> Option<Type> {
        self.ctx.flow.narrowed(local).or(self.ctx.locals[local.index()].ty)
    }

    /// The facts of a checked condition.
    pub(crate) fn facts(&self, cond: &ir::Expr) -> Facts {
        match &cond.kind {
            ir::ExprKind::TypeTest { value, ty } => match value.kind {
                ir::ExprKind::Local(local) if !self.ctx.locals[local.index()].temporary => {
                    let current = value.ty;
                    Facts {
                        when_true: current.intersection(*ty).map(|t| (local, t)).into_iter().collect(),
                        when_false: current.without(*ty).map(|t| (local, t)).into_iter().collect(),
                    }
                }
                _ => Facts::default(),
            },
            ir::ExprKind::Binary { op: op @ (ir::BinaryOp::EqValue | ir::BinaryOp::NeValue), lhs, rhs } => {
                let local = match (&lhs.kind, &rhs.kind) {
                    (ir::ExprKind::Local(local), ir::ExprKind::None)
                    | (ir::ExprKind::None, ir::ExprKind::Local(local)) => *local,
                    _ => return Facts::default(),
                };
                let current = if matches!(lhs.kind, ir::ExprKind::Local(_)) { lhs.ty } else { rhs.ty };
                let facts = Facts {
                    when_true: vec![(local, Type::None)],
                    when_false: current.without(Type::None).map(|t| (local, t)).into_iter().collect(),
                };
                if *op == ir::BinaryOp::EqValue { facts } else { facts.swapped() }
            }
            ir::ExprKind::Unary { op: ir::UnaryOp::Not, operand } => self.facts(operand).swapped(),
            ir::ExprKind::And { lhs, rhs } => {
                let mut when_true = self.facts(lhs).when_true;
                when_true.extend(self.facts(rhs).when_true);
                Facts { when_true, when_false: Vec::new() }
            }
            ir::ExprKind::Or { lhs, rhs } => {
                let mut when_false = self.facts(lhs).when_false;
                when_false.extend(self.facts(rhs).when_false);
                Facts { when_true: Vec::new(), when_false }
            }
            _ => Facts::default(),
        }
    }

    pub(crate) fn apply(&mut self, facts: &[(ir::LocalId, Type)]) {
        if !self.ctx.flow.is_reachable() {
            return;
        }
        for &(local, ty) in facts {
            self.ctx.flow.narrow(local, Some(ty));
        }
    }

    /// After `local` receives a value of type `ty`, that is the type it is known to have.
    pub(crate) fn narrow_to_value(&mut self, local: ir::LocalId, ty: Type) {
        let declared = self.ctx.locals[local.index()].ty;
        let narrowed = declared.filter(|declared| *declared != ty && ty.is_subset_of(*declared)).map(|_| ty);
        self.ctx.flow.narrow(local, narrowed);
    }
}
