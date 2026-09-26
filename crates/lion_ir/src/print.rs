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
    print_block(program, &program.body, 1, &mut out);
    out
}

fn print_block(program: &Program, stmts: &[Stmt], depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth);
    for stmt in stmts {
        match stmt {
            Stmt::Assign { local, value } => {
                let local = local_name(program, local.index());
                out.push_str(&format!("{indent}{local} = {}\n", print_expr(program, value)));
            }
            Stmt::Expr(expr) => out.push_str(&format!("{indent}{}\n", print_expr(program, expr))),
            Stmt::If { cond, then, otherwise } => {
                out.push_str(&format!("{indent}if {}\n", print_expr(program, cond)));
                print_block(program, then, depth + 1, out);
                if !otherwise.is_empty() {
                    out.push_str(&format!("{indent}else\n"));
                    print_block(program, otherwise, depth + 1, out);
                }
                out.push_str(&format!("{indent}end\n"));
            }
            Stmt::While { cond, body } => {
                out.push_str(&format!("{indent}while {}\n", print_expr(program, cond)));
                print_block(program, body, depth + 1, out);
                out.push_str(&format!("{indent}end\n"));
            }
            Stmt::Break => out.push_str(&format!("{indent}break\n")),
            Stmt::Continue => out.push_str(&format!("{indent}continue\n")),
            Stmt::Return => out.push_str(&format!("{indent}return\n")),
        }
    }
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
        ExprKind::If { cond, then, otherwise } => {
            format!("(if {} {} {})", print(cond), print(then), print(otherwise))
        }
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
