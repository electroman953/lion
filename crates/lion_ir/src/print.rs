//! Writes a program for `lion debug ir` and type checker tests: each function with
//! its locals, then its statements, with typed operations as S-expressions.

use lion_runtime::format::format_float;

use crate::{Arg, Expr, ExprKind, Function, Place, Program, Stmt};

pub fn print_program(program: &Program) -> String {
    let mut out = String::new();
    let main = program.function(program.main);
    out.push_str("script\n");
    print_function(program, main, &mut out);
    for (index, function) in program.functions.iter().enumerate() {
        if index == program.main.index() {
            continue;
        }
        out.push_str(&format!("fun {} in {}\n", function.name, function.ret));
        print_function(program, function, &mut out);
    }
    out
}

fn print_function(program: &Program, function: &Function, out: &mut String) {
    let printer = Printer { program, function };
    out.push_str("  locals\n");
    for (index, local) in function.locals.iter().enumerate() {
        let kind = if (index as u32) < function.params {
            if local.by_reference { "var param" } else { "param" }
        } else if local.temporary {
            "temp"
        } else if local.mutable {
            "var"
        } else {
            "let"
        };
        out.push_str(&format!("    {}#{index} {kind} {}\n", local.name, local.ty));
    }
    if !function.defaults.is_empty() {
        out.push_str("  defaults\n");
        for (index, value) in &function.defaults {
            out.push_str(&format!("    {} = {}\n", printer.local(*index as usize), printer.expr(value)));
        }
    }
    out.push_str("  body\n");
    printer.block(&function.body, 2, out);
}

struct Printer<'a> {
    program: &'a Program,
    function: &'a Function,
}

impl Printer<'_> {
    fn block(&self, stmts: &[Stmt], depth: usize, out: &mut String) {
        let indent = "  ".repeat(depth);
        for stmt in stmts {
            match stmt {
                Stmt::Assign { place, value } => {
                    out.push_str(&format!("{indent}{} = {}\n", self.place(*place), self.expr(value)));
                }
                Stmt::Expr(expr) => out.push_str(&format!("{indent}{}\n", self.expr(expr))),
                Stmt::If { cond, then, otherwise } => {
                    out.push_str(&format!("{indent}if {}\n", self.expr(cond)));
                    self.block(then, depth + 1, out);
                    if !otherwise.is_empty() {
                        out.push_str(&format!("{indent}else\n"));
                        self.block(otherwise, depth + 1, out);
                    }
                    out.push_str(&format!("{indent}end\n"));
                }
                Stmt::While { cond, body } => {
                    out.push_str(&format!("{indent}while {}\n", self.expr(cond)));
                    self.block(body, depth + 1, out);
                    out.push_str(&format!("{indent}end\n"));
                }
                Stmt::For { var, iterable, body } => {
                    out.push_str(&format!(
                        "{indent}for {} in {}\n",
                        self.local(var.index()),
                        self.expr(iterable)
                    ));
                    self.block(body, depth + 1, out);
                    out.push_str(&format!("{indent}end\n"));
                }
                Stmt::Break => out.push_str(&format!("{indent}break\n")),
                Stmt::Continue => out.push_str(&format!("{indent}continue\n")),
                Stmt::Return(None) => out.push_str(&format!("{indent}return\n")),
                Stmt::Return(Some(value)) => out.push_str(&format!("{indent}return {}\n", self.expr(value))),
            }
        }
    }

    fn expr(&self, expr: &Expr) -> String {
        let print = |expr: &Expr| self.expr(expr);
        match &expr.kind {
            ExprKind::Int(value) => value.to_string(),
            ExprKind::Float(value) => format_float(*value),
            ExprKind::Bool(value) => value.to_string(),
            ExprKind::Text(text) => format!("{text:?}"),
            ExprKind::None => "none".to_string(),
            ExprKind::Local(local) => self.local(local.index()),
            ExprKind::Global(local) => self.global(local.index()),
            ExprKind::Call { function, args } => {
                let mut out = format!("(call {}", self.program.function(*function).name);
                for arg in args {
                    out.push(' ');
                    match arg {
                        Arg::Value(value) => out.push_str(&print(value)),
                        Arg::Reference(place) => out.push_str(&format!("&{}", self.place(*place))),
                    }
                }
                out + ")"
            }
            ExprKind::Let { local, value, body } => {
                format!("(let {} {} {})", self.local(local.index()), print(value), print(body))
            }
            ExprKind::Unary { op, operand } => format!("({} {})", op.name(), print(operand)),
            ExprKind::Binary { op, lhs, rhs } => format!("({} {} {})", op.name(), print(lhs), print(rhs)),
            ExprKind::And { lhs, rhs } => format!("(and {} {})", print(lhs), print(rhs)),
            ExprKind::Or { lhs, rhs } => format!("(or {} {})", print(lhs), print(rhs)),
            ExprKind::Convert { conversion, value } => format!("({} {})", conversion.name(), print(value)),
            ExprKind::If { cond, then, otherwise } => {
                format!("(if {} {} {})", print(cond), print(then), print(otherwise))
            }
            ExprKind::Range { start, end } => format!("(range {} {})", print(start), print(end)),
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

    fn place(&self, place: Place) -> String {
        match place {
            Place::Local(local) => self.local(local.index()),
            Place::Global(local) => self.global(local.index()),
        }
    }

    fn local(&self, index: usize) -> String {
        format!("{}#{index}", self.function.locals[index].name)
    }

    fn global(&self, index: usize) -> String {
        let main = self.program.function(self.program.main);
        format!("@{}#{index}", main.locals[index].name)
    }
}
