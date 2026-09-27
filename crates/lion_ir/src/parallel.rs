//! What the turns of a parallel loop may reach (§19.2), for the backends that run them
//! on several threads: the modules to initialize before the threads start, and whether
//! the turns must run one after the other.

use std::collections::HashSet;

use crate::visit::{exprs_in, exprs_in_stmts};
use crate::{Arg, Builtin, Expr, ExprKind, Function, ParallelLoop, Place, Program, Stmt};

/// What the turns of a parallel loop may reach, through the functions they call.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reach {
    /// The files whose globals the turns may use, in the order in which the code meets
    /// them. They get their values before the threads start, since a thread does not
    /// give values to the globals (D81, C84).
    pub modules: Vec<u32>,
    /// A turn may read the keyboard, record a failed `expect`, or use a `shared synced`
    /// object: the turns then run one after the other, in their order, which gives the
    /// same result as a lock on each access (§19.3, C84).
    pub in_order: bool,
}

/// What the turns of `parallel`, a loop of `function`, may reach.
pub fn reach(program: &Program, function: &Function, parallel: &ParallelLoop) -> Reach {
    let mut ctx =
        Ctx { program, seen: HashSet::new(), pending: Vec::new(), reach: Reach::default(), dynamic: false };
    // `==`, `in` and the Sets may call the `equals` of a structure (§12.5).
    for def in &program.structs {
        if let Some(equals) = def.equals {
            ctx.visit(equals.0);
        }
    }
    walk(&parallel.body, Some(function), &mut ctx);
    while let Some(next) = ctx.pending.pop() {
        let called = &program.functions[next as usize];
        walk(&called.body, None, &mut ctx);
        for (_, value) in &called.defaults {
            exprs_in(value, &mut |expr| expr_use(expr, None, &mut ctx));
        }
    }
    ctx.reach
}

struct Ctx<'p> {
    program: &'p Program,
    seen: HashSet<u32>,
    /// The functions met but not yet gone through.
    pending: Vec<u32>,
    reach: Reach,
    /// Whether the functions that may be called through a value are already met.
    dynamic: bool,
}

impl Ctx<'_> {
    fn visit(&mut self, function: u32) {
        if self.seen.insert(function) {
            self.pending.push(function);
        }
    }

    /// A turn uses `place`: `own` is the function whose locals the code names. The
    /// locals of a function called by a turn belong to that call.
    fn uses(&mut self, place: Place, own: Option<&Function>) {
        let synced = match place {
            Place::Local(local) => own.is_some_and(|function| function.local(local).synced),
            Place::Global(global) => self.program.function(self.program.main).local(global).synced,
        };
        if synced {
            self.reach.in_order = true;
        }
    }

    /// A function value may be any function that the program uses as a value.
    fn dynamic(&mut self) {
        if std::mem::replace(&mut self.dynamic, true) {
            return;
        }
        let mut functions = Vec::new();
        for function in &self.program.functions {
            let mut found = |expr: &Expr| {
                if let ExprKind::Closure { function, .. } = expr.kind {
                    functions.push(function.0);
                }
            };
            exprs_in_stmts(&function.body, &mut found);
            for (_, value) in &function.defaults {
                exprs_in(value, &mut found);
            }
        }
        for function in functions {
            self.visit(function);
        }
    }
}

fn walk(stmts: &[Stmt], own: Option<&Function>, ctx: &mut Ctx) {
    places(stmts, own, ctx);
    exprs_in_stmts(stmts, &mut |expr| expr_use(expr, own, ctx));
}

/// The places that the statements assign, and the modules they initialize; the
/// expressions are seen by [`expr_use`].
fn places(stmts: &[Stmt], own: Option<&Function>, ctx: &mut Ctx) {
    for stmt in stmts {
        match stmt {
            Stmt::Assign { place, .. } => ctx.uses(*place, own),
            Stmt::AssignElement { root, .. } | Stmt::Add { root, .. } | Stmt::Remove { root, .. } => {
                ctx.uses(*root, own)
            }
            Stmt::InitModule { module } => {
                if !ctx.reach.modules.contains(module) {
                    ctx.reach.modules.push(*module);
                }
            }
            Stmt::If { then, otherwise, .. } => {
                places(then, own, ctx);
                places(otherwise, own, ctx);
            }
            Stmt::While { body, .. } | Stmt::For { body, .. } | Stmt::Seq(body) => places(body, own, ctx),
            Stmt::Parallel(parallel) => places(&parallel.body, own, ctx),
            Stmt::Expr(_) | Stmt::Return(_) | Stmt::Break | Stmt::Continue | Stmt::Declare { .. } => {}
        }
    }
}

/// One expression, without the ones inside it, which the visitor also gives.
fn expr_use(expr: &Expr, own: Option<&Function>, ctx: &mut Ctx) {
    match &expr.kind {
        ExprKind::Call { function, args } => {
            ctx.visit(function.0);
            for arg in args {
                if let Arg::Reference(place) = arg {
                    ctx.uses(*place, own);
                }
            }
        }
        ExprKind::Closure { function, .. } => ctx.visit(function.0),
        ExprKind::CallValue { .. } | ExprKind::Partial { .. } => ctx.dynamic(),
        ExprKind::CallBuiltin { builtin: Builtin::Ask | Builtin::ExpectFailed, .. } => {
            ctx.reach.in_order = true
        }
        ExprKind::Local(local) | ExprKind::Cell(local) => ctx.uses(Place::Local(*local), own),
        ExprKind::Global(global) => ctx.uses(Place::Global(*global), own),
        // The visitor gives the expressions of a block, not its places.
        ExprKind::Block { stmts, .. } => places(stmts, own, ctx),
        _ => {}
    }
}
