//! Going through every expression of a body, for the passes that look for something
//! in it or replace it.

use crate::{Arg, Expr, ExprKind, Step, Stmt};

/// Calls `f` on every expression of the statements, each before the ones inside it.
pub fn exprs_in_stmts(stmts: &[Stmt], f: &mut dyn FnMut(&Expr)) {
    for stmt in stmts {
        match stmt {
            Stmt::Assign { value, .. } | Stmt::Expr(value) | Stmt::Return(Some(value)) => exprs_in(value, f),
            Stmt::If { cond, then, otherwise } => {
                exprs_in(cond, f);
                exprs_in_stmts(then, f);
                exprs_in_stmts(otherwise, f);
            }
            Stmt::While { cond, body } => {
                exprs_in(cond, f);
                exprs_in_stmts(body, f);
            }
            Stmt::Seq(stmts) => exprs_in_stmts(stmts, f),
            Stmt::AssignElement { path, value, .. }
            | Stmt::Add { path, value, .. }
            | Stmt::Remove { path, key: value, .. } => {
                for step in path {
                    if let Step::Index(index) = step {
                        exprs_in(index, f);
                    }
                }
                exprs_in(value, f);
            }
            Stmt::For { iterable, body, .. } => {
                exprs_in(iterable, f);
                exprs_in_stmts(body, f);
            }
            Stmt::Return(None)
            | Stmt::Break
            | Stmt::Continue
            | Stmt::InitModule { .. }
            | Stmt::Declare { .. } => {}
        }
    }
}

/// Calls `f` on `expr`, then on every expression inside it.
pub fn exprs_in(expr: &Expr, f: &mut dyn FnMut(&Expr)) {
    f(expr);
    match &expr.kind {
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Text(_)
        | ExprKind::None
        | ExprKind::Local(_)
        | ExprKind::Global(_)
        | ExprKind::Enum { .. }
        | ExprKind::Cell(_) => {}
        ExprKind::Call { args, .. } => {
            for arg in args {
                if let Arg::Value(value) = arg {
                    exprs_in(value, f);
                }
            }
        }
        ExprKind::Let { value, body, .. } => {
            exprs_in(value, f);
            exprs_in(body, f);
        }
        ExprKind::Unary { operand, .. } => exprs_in(operand, f),
        ExprKind::Binary { lhs, rhs, .. } | ExprKind::And { lhs, rhs } | ExprKind::Or { lhs, rhs } => {
            exprs_in(lhs, f);
            exprs_in(rhs, f);
        }
        ExprKind::Convert { value, .. }
        | ExprKind::TypeTest { value, .. }
        | ExprKind::Try(value)
        | ExprKind::Task(value)
        | ExprKind::Wait(value)
        | ExprKind::Compile(value) => exprs_in(value, f),
        ExprKind::List(values)
        | ExprKind::Set(values)
        | ExprKind::Tuple(values)
        | ExprKind::Concat(values)
        | ExprKind::CallBuiltin { args: values, .. }
        | ExprKind::Struct { fields: values, .. }
        | ExprKind::Closure { captures: values, .. } => {
            for value in values {
                exprs_in(value, f);
            }
        }
        ExprKind::Index { object, index: other }
        | ExprKind::Slice { object, range: other }
        | ExprKind::Range { start: object, end: other } => {
            exprs_in(object, f);
            exprs_in(other, f);
        }
        ExprKind::Property { object, .. } | ExprKind::Field { object, .. } => exprs_in(object, f),
        ExprKind::Block { stmts, value } => {
            exprs_in_stmts(stmts, f);
            exprs_in(value, f);
        }
        ExprKind::If { cond, then, otherwise } => {
            exprs_in(cond, f);
            exprs_in(then, f);
            exprs_in(otherwise, f);
        }
        ExprKind::CallValue { callee, args } | ExprKind::Partial { callee, args } => {
            exprs_in(callee, f);
            for arg in args {
                exprs_in(arg, f);
            }
        }
        ExprKind::Map(entries) => {
            for (key, value) in entries {
                exprs_in(key, f);
                exprs_in(value, f);
            }
        }
    }
}

/// Calls `f` on every expression of the statements, each after the ones inside it, so
/// that `f` may replace it.
pub fn exprs_in_stmts_mut(stmts: &mut [Stmt], f: &mut dyn FnMut(&mut Expr)) {
    for stmt in stmts {
        match stmt {
            Stmt::Assign { value, .. } | Stmt::Expr(value) | Stmt::Return(Some(value)) => {
                exprs_in_mut(value, f)
            }
            Stmt::If { cond, then, otherwise } => {
                exprs_in_mut(cond, f);
                exprs_in_stmts_mut(then, f);
                exprs_in_stmts_mut(otherwise, f);
            }
            Stmt::While { cond, body } => {
                exprs_in_mut(cond, f);
                exprs_in_stmts_mut(body, f);
            }
            Stmt::Seq(stmts) => exprs_in_stmts_mut(stmts, f),
            Stmt::AssignElement { path, value, .. }
            | Stmt::Add { path, value, .. }
            | Stmt::Remove { path, key: value, .. } => {
                for step in path {
                    if let Step::Index(index) = step {
                        exprs_in_mut(index, f);
                    }
                }
                exprs_in_mut(value, f);
            }
            Stmt::For { iterable, body, .. } => {
                exprs_in_mut(iterable, f);
                exprs_in_stmts_mut(body, f);
            }
            Stmt::Return(None)
            | Stmt::Break
            | Stmt::Continue
            | Stmt::InitModule { .. }
            | Stmt::Declare { .. } => {}
        }
    }
}

/// Calls `f` on every expression inside `expr`, then on `expr`.
pub fn exprs_in_mut(expr: &mut Expr, f: &mut dyn FnMut(&mut Expr)) {
    match &mut expr.kind {
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Text(_)
        | ExprKind::None
        | ExprKind::Local(_)
        | ExprKind::Global(_)
        | ExprKind::Enum { .. }
        | ExprKind::Cell(_) => {}
        ExprKind::Call { args, .. } => {
            for arg in args {
                if let Arg::Value(value) = arg {
                    exprs_in_mut(value, f);
                }
            }
        }
        ExprKind::Let { value, body, .. } => {
            exprs_in_mut(value, f);
            exprs_in_mut(body, f);
        }
        ExprKind::Unary { operand, .. } => exprs_in_mut(operand, f),
        ExprKind::Binary { lhs, rhs, .. } | ExprKind::And { lhs, rhs } | ExprKind::Or { lhs, rhs } => {
            exprs_in_mut(lhs, f);
            exprs_in_mut(rhs, f);
        }
        ExprKind::Convert { value, .. }
        | ExprKind::TypeTest { value, .. }
        | ExprKind::Try(value)
        | ExprKind::Task(value)
        | ExprKind::Wait(value)
        | ExprKind::Compile(value) => exprs_in_mut(value, f),
        ExprKind::List(values)
        | ExprKind::Set(values)
        | ExprKind::Tuple(values)
        | ExprKind::Concat(values)
        | ExprKind::CallBuiltin { args: values, .. }
        | ExprKind::Struct { fields: values, .. }
        | ExprKind::Closure { captures: values, .. } => {
            for value in values {
                exprs_in_mut(value, f);
            }
        }
        ExprKind::Index { object, index: other }
        | ExprKind::Slice { object, range: other }
        | ExprKind::Range { start: object, end: other } => {
            exprs_in_mut(object, f);
            exprs_in_mut(other, f);
        }
        ExprKind::Property { object, .. } | ExprKind::Field { object, .. } => exprs_in_mut(object, f),
        ExprKind::Block { stmts, value } => {
            exprs_in_stmts_mut(stmts, f);
            exprs_in_mut(value, f);
        }
        ExprKind::If { cond, then, otherwise } => {
            exprs_in_mut(cond, f);
            exprs_in_mut(then, f);
            exprs_in_mut(otherwise, f);
        }
        ExprKind::CallValue { callee, args } | ExprKind::Partial { callee, args } => {
            exprs_in_mut(callee, f);
            for arg in args {
                exprs_in_mut(arg, f);
            }
        }
        ExprKind::Map(entries) => {
            for (key, value) in entries {
                exprs_in_mut(key, f);
                exprs_in_mut(value, f);
            }
        }
    }
    f(expr);
}
