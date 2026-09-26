//! Writes a program for `lion debug ir` and type checker tests: the resolved locals
//! with their types, then the statements with typed operations as S-expressions.

use lion_runtime::format::format_float;

use crate::{Expr, ExprKind, Program, Stmt};

pub fn print_program(program: &Program) -> String {
    let mut out = String::from("locals\n");
    for (index, local) in program.locals.iter().enumerate() {
        let kind = if local.temporary {
            "temp"
        } else if local.mutable {
            "var"
        } else {
            "let"
        };
        out.push_str(&format!("  {}#{index} {kind} {}\n", local.name, local.ty));
    }
    out.push_str("body\n");
    for stmt in &program.body {
        let line = match stmt {
            Stmt::Assign { local, value } => {
                format!("{} = {}", local_name(program, local.index()), print_expr(program, value))
            }
            Stmt::Expr(expr) => print_expr(program, expr),
        };
        out.push_str(&format!("  {line}\n"));
    }
    out
}

fn print_expr(program: &Program, expr: &Expr) -> String {
    let print = |expr: &Expr| print_expr(program, expr);
    match &expr.kind {
        ExprKind::Int(value) => value.to_string(),
        ExprKind::Float(value) => format_float(*value),
        ExprKind::Bool(value) => value.to_string(),
        ExprKind::Text(text) => format!("{text:?}"),
        ExprKind::None => "none".to_string(),
        ExprKind::Local(local) => local_name(program, local.index()),
        ExprKind::Let { local, value, body } => {
            format!("(let {} {} {})", local_name(program, local.index()), print(value), print(body))
        }
        ExprKind::Unary { op, operand } => format!("({} {})", op.name(), print(operand)),
        ExprKind::Binary { op, lhs, rhs } => format!("({} {} {})", op.name(), print(lhs), print(rhs)),
        ExprKind::And { lhs, rhs } => format!("(and {} {})", print(lhs), print(rhs)),
        ExprKind::Or { lhs, rhs } => format!("(or {} {})", print(lhs), print(rhs)),
        ExprKind::Convert { conversion, value } => format!("({} {})", conversion.name(), print(value)),
        ExprKind::Concat(parts) => {
            let parts: Vec<String> = parts.iter().map(print).collect();
            format!("(concat {})", parts.join(" "))
        }
        ExprKind::CallBuiltin { builtin, args } => {
            let args: Vec<String> = args.iter().map(print).collect();
            format!("({} {})", builtin.name(), args.join(" "))
        }
    }
}

fn local_name(program: &Program, index: usize) -> String {
    format!("{}#{index}", program.locals[index].name)
}
