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
            let private = if decl.private.is_some() { "private " } else { "" };
            let mut out =
                format!("({private}{} {}", if decl.mutable { "var" } else { "let" }, decl.name.name);
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
        StmtKind::If { branches, otherwise } => {
            let mut out = String::from("(if");
            for (index, branch) in branches.iter().enumerate() {
                if index > 0 {
                    out.push_str(" elif");
                }
                out.push_str(&format!(" {} {}", print_expr(&branch.cond), print_block(&branch.body)));
            }
            if let Some(otherwise) = otherwise {
                out.push_str(&format!(" else {}", print_block(otherwise)));
            }
            out + ")"
        }
        StmtKind::While { cond, body } => format!("(while {} {})", print_expr(cond), print_block(body)),
        StmtKind::For { parallel, var, iterable, body } => {
            let keyword = if parallel.is_some() { "parallel-for" } else { "for" };
            format!("({keyword} {} {} {})", var.name, print_expr(iterable), print_block(body))
        }
        StmtKind::Break => "break".to_string(),
        StmtKind::Continue => "continue".to_string(),
        StmtKind::Return(None) => "(return)".to_string(),
        StmtKind::Return(Some(value)) => format!("(return {})", print_expr(value)),
        StmtKind::Fun(decl) => print_fun(decl),
        StmtKind::Match { scrutinee, cases } => {
            let cases: Vec<String> = cases
                .iter()
                .map(|(case, body)| format!("({} {})", print_case(case), print_block(body)))
                .collect();
            format!("(match {} {})", print_expr(scrutinee), cases.join(" "))
        }
        StmtKind::Struct(decl) => print_struct(decl),
        StmtKind::Trait(decl) => {
            let mut out = format!("(trait {}", decl.name.name);
            for method in &decl.methods {
                out.push(' ');
                out.push_str(&print_fun(method));
            }
            for (name, ty) in &decl.fields {
                out.push_str(&format!(" ({} : {})", name.name, print_type(ty)));
            }
            out + ")"
        }
        StmtKind::Test { name, decl } => format!("(test {name:?} {})", print_fun(decl)),
        StmtKind::Expect(condition) => format!("(expect {})", print_expr(&condition.expr)),
        StmtKind::Use(path) => {
            let names: Vec<&str> = path.iter().map(|name| name.name.as_str()).collect();
            format!("(use {})", names.join("."))
        }
        StmtKind::TypeDef(def) => match &def.kind {
            TypeDefKind::Enum { ordered, values } => {
                let values: Vec<&str> = values.iter().map(|value| value.name.as_str()).collect();
                let kind = if *ordered { "ordered-enum" } else { "enum" };
                format!("({kind} {} {})", def.name.name, values.join(" "))
            }
            TypeDefKind::Union(ty) => format!("(type {} {})", def.name.name, print_type(ty)),
        },
    }
}

fn print_struct(decl: &StructDecl) -> String {
    let mut out = format!("(struct {}", decl.name.name);
    for line in &decl.lines {
        match line {
            StructLine::Field(field) => {
                let private = if field.private.is_some() { "private " } else { "" };
                out.push_str(&format!(" ({private}{} : {}", field.name.name, print_type(&field.ty)));
                if let Some(default) = &field.default {
                    out.push_str(&format!(" = {}", print_expr(default)));
                }
                for condition in &field.conditions {
                    out.push_str(&format!(", {}", print_expr(&condition.expr)));
                }
                out.push(')');
            }
            StructLine::Invariant(condition) => {
                out.push_str(&format!(" (invariant {})", print_expr(&condition.expr)));
            }
        }
    }
    out + ")"
}

fn print_fun(decl: &FunDecl) -> String {
    let mut out = String::from("(");
    if decl.private.is_some() {
        out.push_str("private ");
    }
    if let Some((abi, _)) = &decl.foreign {
        out.push_str(&format!("foreign {abi:?} "));
    }
    out.push_str(if decl.infix { "infix-fun " } else { "fun " });
    if let Some(receiver) = &decl.receiver {
        out.push_str(&format!("{}.", receiver.name));
    }
    out.push_str(&decl.name.name);
    let params: Vec<String> = decl
        .params
        .iter()
        .map(|param| {
            let mut out = format!("({}{}", if param.var.is_some() { "var " } else { "" }, param.name.name);
            if let Some(ty) = &param.ty {
                out.push_str(&format!(" : {}", print_type(ty)));
            }
            if let Some(default) = &param.default {
                out.push_str(&format!(" = {}", print_expr(default)));
            }
            out + ")"
        })
        .collect();
    out.push_str(&format!(" ({})", params.join(" ")));
    if let Some(ret) = &decl.ret {
        out.push_str(&format!(" in {}", print_type(ret)));
    }
    for (name, set) in &decl.type_params {
        out.push_str(&format!(" (where {} {})", name.name, print_type(set)));
    }
    if !decl.modifies.is_empty() {
        let names: Vec<&str> = decl.modifies.iter().map(|name| name.name.as_str()).collect();
        out.push_str(&format!(" (modifies {})", names.join(" ")));
    }
    match &decl.body {
        FunBody::Block(block) => out.push_str(&format!(" {}", print_block(block))),
        FunBody::Expr(expr) => out.push_str(&format!(" = {}", print_expr(expr))),
        FunBody::Foreign => {}
        FunBody::Required => out.push_str(" required"),
    }
    out + ")"
}

fn print_case(case: &Case) -> String {
    let binding =
        |binding: &Option<Ident>| binding.as_ref().map_or(String::new(), |name| format!(" {}", name.name));
    let mut out = match &case.pattern {
        Pattern::Otherwise => "otherwise".to_string(),
        Pattern::Value(value) => print_expr(value),
        Pattern::Type { ty, binding: name } => format!("in-type {}{}", print_type(ty), binding(name)),
        Pattern::In { set, binding: name } => format!("in {}{}", print_expr(set), binding(name)),
    };
    for condition in &case.conditions {
        out.push_str(&format!(", {}", print_expr(condition)));
    }
    out
}

fn print_block(block: &Block) -> String {
    let stmts: Vec<String> = block.stmts.iter().map(print_stmt).collect();
    format!("[{}]", stmts.join(" "))
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
        ExprKind::List(elements) => {
            let elements: Vec<String> = elements.iter().map(print_expr).collect();
            format!("(list{}{})", if elements.is_empty() { "" } else { " " }, elements.join(" "))
        }
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
        ExprKind::TypeTest { value, ty } => format!("(in-type {} {})", print_expr(value), print_type(ty)),
        ExprKind::Try(value) => format!("(try {})", print_expr(value)),
        ExprKind::Match { scrutinee, cases } => {
            let cases: Vec<String> = cases
                .iter()
                .map(|(case, value)| format!("({} then {})", print_case(case), print_expr(value)))
                .collect();
            format!("(match-expr {} {})", print_expr(scrutinee), cases.join(" "))
        }
        ExprKind::If { branches, otherwise } => {
            let mut out = String::from("(if-expr");
            for (index, (cond, value)) in branches.iter().enumerate() {
                if index > 0 {
                    out.push_str(" elif");
                }
                out.push_str(&format!(" {} {}", print_expr(cond), print_expr(value)));
            }
            if let Some(otherwise) = otherwise {
                out.push_str(&format!(" else {}", print_expr(otherwise)));
            }
            out + ")"
        }
        ExprKind::Parallel(inner) => format!("(parallel {})", print_expr(inner)),
        ExprKind::Shared { value, synced: false } => format!("(shared {})", print_expr(value)),
        ExprKind::Shared { value, synced: true } => format!("(shared synced {})", print_expr(value)),
        ExprKind::Fun(decl) => print_fun(decl),
        ExprKind::Task(value) => format!("(task {})", print_expr(value)),
        ExprKind::Wait(value) => format!("(wait {})", print_expr(value)),
        ExprKind::Set(elements) => {
            let elements: Vec<String> = elements.iter().map(print_expr).collect();
            if elements.is_empty() { "(set)".to_string() } else { format!("(set {})", elements.join(" ")) }
        }
        ExprKind::Tuple(elements) => {
            let elements: Vec<String> = elements
                .iter()
                .map(|element| match &element.name {
                    Some(name) => format!("{}: {}", name.name, print_expr(&element.value)),
                    None => print_expr(&element.value),
                })
                .collect();
            if elements.is_empty() {
                "(tuple)".to_string()
            } else {
                format!("(tuple {})", elements.join(" "))
            }
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
