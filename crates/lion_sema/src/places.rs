//! Places: the variables and the parts of variables that a statement changes (§6.3,
//! §6.4), and the checks of invariants that follow a change inside a structure
//! (§12.3, D40).

use lion_diagnostics::{Diagnostic, Span};
use lion_ir::{self as ir, Type};
use lion_syntax::ast;

use crate::flow::Assigned;
use crate::names::Resolved;
use crate::{Checker, GlobalType, article, capitalize, typed};

/// A variable, then the steps to a part of it: `class[i].grade`.
pub(crate) struct Path {
    pub(crate) root: ir::Place,
    pub(crate) steps: Vec<ir::Step>,
    /// The type of the root, then the type after each step.
    pub(crate) types: Vec<Type>,
    pub(crate) span: Span,
    /// The root is `self` in a `var self` method: its invariants are checked when the
    /// outermost `var self` call on it ends, not at each change (§12.3, D9).
    pub(crate) in_method: bool,
}

impl Path {
    pub(crate) fn ty(&self) -> Type {
        *self.types.last().expect("a path has the type of its root")
    }
}

impl Checker<'_> {
    /// The place that `target` designates, which must be a `var` variable or a part of
    /// one (§6.4).
    pub(crate) fn place_path(&mut self, target: &ast::Expr) -> Option<Path> {
        match &target.kind {
            ast::ExprKind::Name(name) => {
                let (root, ty, in_method) = self.changeable_variable(name, target.span)?;
                Some(Path { root, steps: Vec::new(), types: vec![ty], span: target.span, in_method })
            }
            ast::ExprKind::Paren(inner) => self.place_path(inner),
            ast::ExprKind::Index { object, index } => {
                let mut path = self.place_path(object)?;
                let index = self.expr(index)?;
                let element = match (path.ty(), index.ty) {
                    (Type::List(element), Type::Int) => element.get(),
                    (Type::Text, _) => {
                        self.diagnostics.push(
                            Diagnostic::error("a Text cannot be changed character by character")
                                .with_primary(target.span, "")
                                .with_help("build a new text with interpolation, and assign it"),
                        );
                        return None;
                    }
                    (Type::List(_), other) => {
                        self.diagnostics.push(
                            Diagnostic::error("the index of an element to change is an Int")
                                .with_primary(index.span, format!("this is {}", article(other))),
                        );
                        return None;
                    }
                    (other, _) => {
                        self.diagnostics.push(
                            Diagnostic::error(format!(
                                "{} has no elements to change",
                                capitalize(&article(other))
                            ))
                            .with_primary(target.span, ""),
                        );
                        return None;
                    }
                };
                path.steps.push(ir::Step::Index(index));
                path.types.push(element);
                path.span = target.span;
                Some(path)
            }
            ast::ExprKind::Field { object, name } => {
                let mut path = self.place_path(object)?;
                let ty = path.ty();
                let (field, field_ty) = self.field_of(ty, name, object.span)?;
                path.steps.push(ir::Step::Field(field));
                path.types.push(field_ty);
                path.span = target.span;
                Some(path)
            }
            _ => {
                self.diagnostics.push(
                    Diagnostic::error("only a variable, or a part of one, can be changed")
                        .with_primary(target.span, "")
                        .with_note("a temporary value would lose the change"),
                );
                None
            }
        }
    }

    /// A variable whose content changes: a `var` with a value (§6.4), and whether it is
    /// the `self` of a `var self` method.
    fn changeable_variable(&mut self, name: &str, span: Span) -> Option<(ir::Place, Type, bool)> {
        match self.resolve(name, span) {
            Resolved::Local(local) => {
                if !self.check_has_value(local, span) {
                    return None;
                }
                let info = &self.ctx.locals[local.index()];
                if !info.mutable {
                    let decl_span = info.decl_span;
                    let error = if info.captured {
                        crate::flow::captured_change(name, span)
                    } else if name == "self" {
                        Diagnostic::error("this method cannot change `self`")
                            .with_primary(span, "")
                            .with_note("a method changes its object only when it is declared with `var self` (§12.4)")
                            .with_help("add `var self` as the first parameter: `fun Type.name(var self, ...)`")
                    } else {
                        Diagnostic::error(format!("cannot change the content of the constant `{name}`"))
                            .with_primary(span, "")
                            .with_secondary(decl_span, "declared with `let`")
                            .with_note("a value held by `let` never changes, nor does its content (§6.4)")
                            .with_help(format!("to change it, declare it with `var`: `var {name} = ...`"))
                    };
                    self.diagnostics.push(error);
                    return None;
                }
                let in_method = name == "self" && info.by_reference;
                if self.changes_outside_parallel(crate::parallel::Variable::Local(local), name, span) {
                    return None;
                }
                let ty = self.local_type(local)?;
                self.ctx.flow.set(local, Assigned::Yes);
                Some((ir::Place::Local(local), ty, in_method))
            }
            Resolved::Global(local) => {
                if !self.check_global_assignment(local, span)
                    || self.changes_outside_parallel(crate::parallel::Variable::Global(local), name, span)
                {
                    return None;
                }
                self.ctx.reads.push(local);
                let GlobalType::Known(ty) = self.global_info(local).ty else { return None };
                Some((ir::Place::Global(local), ty?, false))
            }
            Resolved::Function(_) | Resolved::Standard(_) => {
                self.diagnostics.push(
                    Diagnostic::error(format!("`{name}` is a function, not a variable"))
                        .with_primary(span, ""),
                );
                None
            }
            Resolved::Nothing => None,
        }
    }

    /// Stores the indices of the path in temporaries, so that the path can be read and
    /// changed several times while each index is evaluated once, left to right (§9.2).
    pub(crate) fn stabilize(&mut self, mut path: Path) -> (Vec<ir::Stmt>, Path) {
        let mut stmts = Vec::new();
        for step in &mut path.steps {
            let ir::Step::Index(index) = step else { continue };
            // A variable is copied too: the value assigned may change it (§9.2).
            if matches!(index.kind, ir::ExprKind::Int(_)) {
                continue;
            }
            let span = index.span;
            let temp = self.temporary(Type::Int, span);
            let value = std::mem::replace(index, typed(ir::ExprKind::Local(temp), Type::Int, span));
            stmts.push(ir::Stmt::Assign { place: ir::Place::Local(temp), value });
        }
        (stmts, path)
    }

    /// The value found after the first `steps` steps of a stabilized path.
    pub(crate) fn read_path(&self, path: &Path, steps: usize) -> ir::Expr {
        let root = match path.root {
            ir::Place::Local(local) => ir::ExprKind::Local(local),
            ir::Place::Global(local) => ir::ExprKind::Global(local),
        };
        let mut value = typed(root, path.types[0], path.span);
        for (step, ty) in path.steps[..steps].iter().zip(&path.types[1..]) {
            let object = Box::new(value);
            let kind = match step {
                ir::Step::Index(index) => ir::ExprKind::Index { object, index: Box::new(index.clone()) },
                ir::Step::Field(field) => ir::ExprKind::Field { object, field: *field },
            };
            value = typed(kind, *ty, path.span);
        }
        value
    }

    /// After a change at the end of a stabilized path: the structures that contain the
    /// changed part check their invariants, innermost first (§12.3, D40). The part
    /// itself is checked too when `whole` is set, after a `var self` method changed it.
    pub(crate) fn check_invariants(&mut self, path: &Path, whole: bool) -> Vec<ir::Stmt> {
        let last = if whole { path.steps.len() } else { path.steps.len().saturating_sub(1) };
        if !whole && path.steps.is_empty() {
            return Vec::new();
        }
        let mut stmts = Vec::new();
        for steps in (0..=last).rev() {
            if steps == 0 && path.in_method {
                continue;
            }
            if let Type::Struct(structure) = path.types[steps] {
                let value = self.read_path(path, steps);
                stmts.extend(self.validation(structure, value));
            }
        }
        stmts
    }
}
