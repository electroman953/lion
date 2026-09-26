//! Writes the syntax tree as S-expressions, for `lion debug ast` and parser tests.
//! The structure, and so the precedence of operators, is explicit: `1 + 2 * 3` is
//! printed `(+ 1 (* 2 3))`.

use crate::ast::*;
use crate::token::escape_text;

pub fn print_module(module: &Module) -> String {
    let mut out = String::new();
    for stmt in &module.stmts {
        out.push_str(&print_stmt(stmt));
        out.push('\n');
    }
    out
}

pub fn print_stmt(stmt: &Stmt) -> String {
    match &stmt.kind {
        StmtKind::Let(decl) => {
            let mut out = format!("({} {}", if decl.mutable { "var" } else { "let" }, decl.name.name);
            if let Some(value) = &decl.value {
                out.push(' ');
                out.push_str(&print_expr(value));
            }
            if let Some(annotation) = &decl.annotation {
                out.push_str(" : ");
                out.push_str(&print_type(annotation));
            }
            out + ")"
        }
        StmtKind::Assign { target, op, value, .. } => {
            format!("({} {} {})", op.as_str(), print_expr(target), print_expr(value))
        }
        StmtKind::Expr(expr) => print_expr(expr),
    }
}

pub fn print_expr(expr: &Expr) -> String {
    match &expr.kind {
        ExprKind::Int(value) => value.to_string(),
        ExprKind::Float(value) => format!("{value:?}"),
        ExprKind::Bool(value) => value.to_string(),
        ExprKind::None => "none".to_string(),
        ExprKind::Text(parts) => match parts.as_slice() {
            [] => "\"\"".to_string(),
            [TextPart::Literal(text)] => format!("\"{}\"", escape_text(text)),
            parts => {
                let parts: Vec<String> = parts
                    .iter()
                    .map(|part| match part {
                        TextPart::Literal(text) => format!("\"{}\"", escape_text(text)),
                        TextPart::Interpolation(value) => format!("{{{}}}", print_expr(value)),
                    })
                    .collect();
                format!("(text {})", parts.join(" "))
            }
        },
        ExprKind::Name(name) | ExprKind::TypeName(name) => name.clone(),
        ExprKind::Paren(inner) => format!("(paren {})", print_expr(inner)),
        ExprKind::Unary { op, operand } => {
            let op = match op {
                UnaryOp::Neg => "neg",
                UnaryOp::Not => "not",
            };
            format!("({op} {})", print_expr(operand))
        }
        ExprKind::Binary { op, lhs, rhs, .. } => {
            format!("({} {} {})", op.as_str(), print_expr(lhs), print_expr(rhs))
        }
        ExprKind::Compare { first, rest } if rest.len() == 1 => {
            format!("({} {} {})", rest[0].op.as_str(), print_expr(first), print_expr(&rest[0].rhs))
        }
        ExprKind::Compare { first, rest } => {
            let mut out = format!("(chain {}", print_expr(first));
            for comparison in rest {
                out.push_str(&format!(" {} {}", comparison.op.as_str(), print_expr(&comparison.rhs)));
            }
            out + ")"
        }
        ExprKind::As { value, ty } => format!("(as {} {})", print_expr(value), print_type(ty)),
        ExprKind::Call { callee, args } => {
            let mut out = format!("(call {}", print_expr(callee));
            for arg in args {
                out.push(' ');
                if arg.var_marker.is_some() {
                    out.push_str("var ");
                }
                if let Some(name) = &arg.name {
                    out.push_str(&name.name);
                    out.push_str(": ");
                }
                out.push_str(&print_expr(&arg.value));
            }
            out + ")"
        }
        ExprKind::Field { object, name } => format!("(. {} {})", print_expr(object), name.name),
        ExprKind::Index { object, index } => {
            format!("(index {} {})", print_expr(object), print_expr(index))
        }
    }
}

pub fn print_type(ty: &TypeExpr) -> String {
    let list = |types: &[TypeExpr]| types.iter().map(print_type).collect::<Vec<_>>().join(" ");
    match &ty.kind {
        TypeExprKind::Named { module, name, args } => {
            let mut path: String = module.iter().map(|part| format!("{}.", part.name)).collect();
            path.push_str(&name.name);
            if args.is_empty() { path } else { format!("({path} of {})", list(args)) }
        }
        TypeExprKind::Maybe(inner) => format!("(maybe {})", print_type(inner)),
        TypeExprKind::Union(members) => format!("(or {})", list(members)),
        TypeExprKind::Tuple(members) => format!("(tuple {})", list(members)),
        TypeExprKind::Fun { params, ret: Some(ret) } => {
            format!("(fun ({}) in {})", list(params), print_type(ret))
        }
        TypeExprKind::Fun { params, ret: None } => format!("(fun ({}))", list(params)),
    }
}
