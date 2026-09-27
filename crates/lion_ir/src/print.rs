//! Writes a program for `lion debug ir` and type checker tests: each function with
//! its locals, then its statements, with typed operations as S-expressions.

use lion_runtime::format::format_float;

use crate::{Arg, Expr, ExprKind, Function, Place, Program, Step, Stmt, Type};

pub fn print_program(program: &Program) -> String {
    let mut out = String::new();
    for def in &program.structs {
        let fields: Vec<String> = def.fields.iter().map(|(name, ty)| format!("{name} in {ty}")).collect();
        out.push_str(&format!("struct {}({})\n", def.name, fields.join(", ")));
    }
    for enumeration in &program.enums {
        let (open, close) = if enumeration.is_ordered() { ("[", "]") } else { ("{", "}") };
        out.push_str(&format!(
            "enum {} = {open}{}{close}\n",
            enumeration.name(),
            enumeration.values().join(", ")
        ));
    }
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
        } else if local.captured {
            if local.boxed { "var capture" } else { "capture" }
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
                Stmt::Seq(stmts) => self.block(stmts, depth, out),
                Stmt::AssignElement { root, path, value } => {
                    out.push_str(&format!(
                        "{indent}{}{} = {}\n",
                        self.place(*root),
                        self.path(*root, path),
                        self.expr(value)
                    ));
                }
                Stmt::Add { root, path, value } => {
                    out.push_str(&format!(
                        "{indent}{}{}.add({})\n",
                        self.place(*root),
                        self.path(*root, path),
                        self.expr(value)
                    ));
                }
                Stmt::Remove { root, path, key } => {
                    out.push_str(&format!(
                        "{indent}{}{}.remove({})\n",
                        self.place(*root),
                        self.path(*root, path),
                        self.expr(key)
                    ));
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
                Stmt::Parallel(parallel) => {
                    let gather = match parallel.gather {
                        Some(local) => format!(" gathering {}", self.local(local.index())),
                        None => String::new(),
                    };
                    out.push_str(&format!(
                        "{indent}parallel for {} in {}{gather}\n",
                        self.local(parallel.var.index()),
                        self.expr(&parallel.iterable)
                    ));
                    self.block(&parallel.body, depth + 1, out);
                    out.push_str(&format!("{indent}end\n"));
                }
                Stmt::Break => out.push_str(&format!("{indent}break\n")),
                Stmt::InitModule { module } => out.push_str(&format!("{indent}init_module {module}\n")),
                Stmt::Declare { local } if self.function.local(*local).boxed => {
                    out.push_str(&format!("{indent}new_cell {}\n", self.local(local.index())));
                }
                Stmt::Declare { .. } => {}
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
            ExprKind::List(elements) => {
                let elements: Vec<String> = elements.iter().map(print).collect();
                format!("(list{}{})", if elements.is_empty() { "" } else { " " }, elements.join(" "))
            }
            ExprKind::Set(elements) => {
                let elements: Vec<String> = elements.iter().map(print).collect();
                format!("(set{}{})", if elements.is_empty() { "" } else { " " }, elements.join(" "))
            }
            ExprKind::Map(entries) => {
                let entries: Vec<String> = entries
                    .iter()
                    .map(|(key, value)| format!(" ({} {})", print(key), print(value)))
                    .collect();
                format!("(map{})", entries.concat())
            }
            ExprKind::Tuple(elements) => {
                let elements: Vec<String> = elements.iter().map(print).collect();
                format!("(tuple{}{})", if elements.is_empty() { "" } else { " " }, elements.join(" "))
            }
            ExprKind::Index { object, index } => format!("(index {} {})", print(object), print(index)),
            ExprKind::TypeTest { value, ty } => format!("(in_type {} {ty})", print(value)),
            ExprKind::Try(value) => format!("(try {})", print(value)),
            ExprKind::Slice { object, range } => format!("(slice {} {})", print(object), print(range)),
            ExprKind::Property { object, property } => format!("({} {})", property.name(), print(object)),
            ExprKind::Block { stmts, value } => {
                let mut inner = String::new();
                self.block(stmts, 0, &mut inner);
                let stmts: Vec<&str> = inner.lines().map(str::trim).collect();
                format!("(block [{}] {})", stmts.join("; "), print(value))
            }
            ExprKind::Concat(parts) => {
                let parts: Vec<String> = parts.iter().map(print).collect();
                format!("(concat {})", parts.join(" "))
            }
            ExprKind::CallBuiltin { builtin: crate::Builtin::Native(native), args } => {
                let args: Vec<String> = args.iter().map(print).collect();
                format!("(native {} {})", native.name(), args.join(" "))
            }
            ExprKind::CallBuiltin { builtin, args } => {
                let args: Vec<String> = args.iter().map(print).collect();
                format!("({} {})", builtin.name(), args.join(" "))
            }
            ExprKind::Struct { structure, fields } => {
                let fields: Vec<String> = fields.iter().map(print).collect();
                format!("(struct {} {})", structure.name(), fields.join(" "))
            }
            ExprKind::Closure { function, captures } => {
                let mut out = format!("(fun {}", self.program.function(*function).name);
                for capture in captures {
                    out.push(' ');
                    out.push_str(&print(capture));
                }
                out + ")"
            }
            ExprKind::Cell(local) => format!("(cell {})", self.local(local.index())),
            ExprKind::Task(value) => format!("(task {})", print(value)),
            ExprKind::Wait(value) => format!("(wait {})", print(value)),
            ExprKind::Compile(value) => format!("(compile {})", print(value)),
            ExprKind::Partial { callee, args } => {
                let args: Vec<String> = args.iter().map(print).collect();
                format!("(partial {} {})", print(callee), args.join(" "))
            }
            ExprKind::CallValue { callee, args } => {
                let args: Vec<String> = args.iter().map(print).collect();
                format!(
                    "(call_value {}{}{})",
                    print(callee),
                    if args.is_empty() { "" } else { " " },
                    args.join(" ")
                )
            }
            ExprKind::Enum { enumeration, value } => {
                format!("{}.{}", enumeration.name(), enumeration.values()[*value as usize])
            }
            ExprKind::Field { object, field } => {
                format!("(field {} {})", self.field_name(object.ty, *field), print(object))
            }
        }
    }

    /// The steps from `root`, written as in Lion: `[i]` and `.name`.
    fn path(&self, root: Place, path: &[Step]) -> String {
        let mut ty = match root {
            Place::Local(local) => self.function.locals[local.index()].ty,
            Place::Global(local) => self.program.function(self.program.main).locals[local.index()].ty,
        };
        let mut out = String::new();
        for step in path {
            match step {
                Step::Index(index) => {
                    out.push_str(&format!("[{}]", self.expr(index)));
                    ty = ty.element().unwrap_or(ty);
                }
                Step::Field(field) => {
                    out.push_str(&format!(".{}", self.field_name(ty, *field)));
                    if let Type::Struct(structure) = ty {
                        ty = self.program.structure(structure).fields[*field as usize].1;
                    }
                }
            }
        }
        out
    }

    fn field_name(&self, ty: Type, field: u32) -> String {
        match ty {
            Type::Struct(structure) => self.program.structure(structure).fields[field as usize].0.clone(),
            _ => format!("#{field}"),
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
