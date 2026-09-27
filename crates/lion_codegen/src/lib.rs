//! Translation of the typed IR into Rust code (spec §22.1), which `lion build` compiles
//! to native code with rustc, and so with LLVM.
//!
//! The code calls `lion_native`, the runtime of compiled programs, which shares its
//! values and its operations with the virtual machine: the two modes give the same
//! results and the same bugs (§22.2). The translation follows the one into bytecode
//! (`lion_vm::compile`) step by step, in the same order of evaluation, with the same
//! places in the source for the bugs.
//!
//! Representation:
//! - Int, Float and Bool are `i64`, `f64` and `bool`; every other value is a `Value`.
//!   A value changes representation where the type of its storage and the type of the
//!   expression differ: a variable narrowed to `Int` keeps its storage (§7.4).
//! - Each local is a Rust variable. A variable that a nested function shares holds a
//!   cell (§11.5); a `var` parameter is a pointer to the variable of the caller (§11.2).
//! - The variables of the script that functions reach are `static`; the others are
//!   locals of the function of the script.
//! - Each function becomes `fn fN(rt, params…, captures…) -> R<T>`. A parameter with a
//!   default value is an `Option`, the default being computed by the function (§11.2).
//!   A function called through a value also gets `dN`, which takes `Value`s.
//! - Expressions become `let` statements, one per operation, in the order of evaluation
//!   (§9.2); a call checks the depth of the calls first (C13), and a bug records the
//!   places of the calls in progress on its way out (§18.4).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;

use lion_diagnostics::Span;
use lion_ir::{self as ir, Arg, BinaryOp, Builtin, Conversion, ExprKind, Place, Stmt, Type, UnaryOp};
use lion_vm::kinds;

/// The Rust code of a program: a module whose `main` runs it. `sources` are the files of
/// the program, in the order of their `SourceId`s, to show where a bug happens.
pub fn generate(program: &ir::Program, sources: &[(&str, &str)]) -> String {
    Generator::new(program).program(sources)
}

/// How a value is held in Rust.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Repr {
    Int,
    Float,
    Bool,
    Value,
}

impl Repr {
    fn of(ty: Type) -> Repr {
        match ty {
            Type::Int => Repr::Int,
            Type::Float => Repr::Float,
            Type::Bool => Repr::Bool,
            _ => Repr::Value,
        }
    }

    fn rust(self) -> &'static str {
        match self {
            Repr::Int => "i64",
            Repr::Float => "f64",
            Repr::Bool => "bool",
            Repr::Value => "Value",
        }
    }

    fn zero(self) -> &'static str {
        match self {
            Repr::Int => "0",
            Repr::Float => "0.0",
            Repr::Bool => "false",
            Repr::Value => "Value::None",
        }
    }
}

/// How a variable is held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Storage {
    Plain(Repr),
    /// A `Value::Cell`, shared with nested functions (§11.5).
    Cell,
    /// A `var` parameter: a pointer to the variable of the caller (§11.2).
    Pointer(Repr),
}

impl Storage {
    fn of(local: &ir::Local) -> Storage {
        assert!(!(local.by_reference && local.boxed), "a `var` parameter is not held in a cell");
        if local.by_reference {
            Storage::Pointer(Repr::of(local.ty))
        } else if local.boxed {
            Storage::Cell
        } else {
            Storage::Plain(Repr::of(local.ty))
        }
    }

    fn rust(self) -> String {
        match self {
            Storage::Plain(repr) => repr.rust().to_string(),
            Storage::Cell => "Value".to_string(),
            Storage::Pointer(repr) => format!("*mut {}", repr.rust()),
        }
    }
}

/// A value computed by the code so far.
#[derive(Clone, Debug)]
struct Op {
    code: String,
    repr: Repr,
    /// A variable, read where the value is used; otherwise an owned value, a temporary
    /// or a constant.
    place: bool,
}

impl Op {
    fn owned(code: impl Into<String>, repr: Repr) -> Op {
        Op { code: code.into(), repr, place: false }
    }

    fn place(code: impl Into<String>, repr: Repr) -> Op {
        Op { code: code.into(), repr, place: true }
    }

    fn none() -> Op {
        Op::owned("Value::None", Repr::Value)
    }

    /// The value itself: a copy of a number, a clone of a variable.
    fn take(&self) -> String {
        if self.place && self.repr == Repr::Value {
            format!("{}.clone()", self.code)
        } else {
            self.code.clone()
        }
    }

    /// A reference to the value.
    fn by_ref(&self) -> String {
        format!("&{}", self.code)
    }
}

struct Generator<'p> {
    program: &'p ir::Program,
    spans: Vec<Span>,
    span_numbers: HashMap<Span, usize>,
    texts: Vec<String>,
    text_numbers: HashMap<String, usize>,
    /// The locals of the script that other functions reach: they are `static`.
    statics: HashSet<u32>,
    /// The functions called through a value: closures and `equals` (§11, §12.5).
    dynamic: BTreeSet<u32>,
    layouts: HashMap<ir::StructRef, usize>,
    enums: HashMap<ir::EnumRef, usize>,
}

impl<'p> Generator<'p> {
    fn new(program: &'p ir::Program) -> Generator<'p> {
        let mut statics = HashSet::new();
        for (index, function) in program.functions.iter().enumerate() {
            if index == program.main.index() {
                continue;
            }
            let mut exprs: Vec<&ir::Expr> = function.defaults.iter().map(|(_, value)| value).collect();
            let mut found = |expr: &ir::Expr| match &expr.kind {
                ExprKind::Global(global) => {
                    statics.insert(global.0);
                }
                ExprKind::Call { args, .. } => {
                    for arg in args {
                        if let Arg::Reference(Place::Global(global)) = arg {
                            statics.insert(global.0);
                        }
                    }
                }
                ExprKind::Block { stmts, .. } => globals_assigned(stmts, &mut statics),
                _ => {}
            };
            ir::visit::exprs_in_stmts(&function.body, &mut found);
            for expr in exprs.drain(..) {
                ir::visit::exprs_in(expr, &mut found);
            }
            globals_assigned(&function.body, &mut statics);
        }
        let dynamic =
            program.structs.iter().filter_map(|def| def.equals.map(|function| function.0)).collect();
        Generator {
            program,
            spans: Vec::new(),
            span_numbers: HashMap::new(),
            texts: Vec::new(),
            text_numbers: HashMap::new(),
            statics,
            dynamic,
            layouts: program.structs.iter().enumerate().map(|(index, def)| (def.id, index)).collect(),
            enums: program
                .enums
                .iter()
                .enumerate()
                .map(|(index, enumeration)| (*enumeration, index))
                .collect(),
        }
    }

    fn program(mut self, sources: &[(&str, &str)]) -> String {
        let program = self.program;
        let mut functions = String::new();
        for (index, function) in program.functions.iter().enumerate() {
            let code = FunctionGen::new(&mut self, index, function).generate();
            functions.push_str(&code);
        }
        let dynamic: Vec<u32> = self.dynamic.iter().copied().collect();
        for index in &dynamic {
            functions.push_str(&self.wrapper(*index));
        }

        let mut out = String::new();
        out.push_str(HEADER);
        let _ = writeln!(out, "pub fn main() -> std::process::ExitCode {{\n    start(&PROGRAM)\n}}\n");
        out.push_str("static PROGRAM: Program = Program {\n    sources: &[\n");
        for (name, text) in sources {
            let _ = writeln!(out, "        ({name:?}, {text:?}),");
        }
        out.push_str("    ],\n    structs: &[\n");
        for def in &program.structs {
            let fields: Vec<String> = def.fields.iter().map(|(name, _)| format!("{name:?}")).collect();
            let equals = match def.equals {
                Some(function) => format!("Some({})", function.0),
                None => "None".to_string(),
            };
            let _ = writeln!(
                out,
                "        Structure {{ name: {:?}, fields: &[{}], equals: {equals} }},",
                def.name,
                fields.join(", ")
            );
        }
        out.push_str("    ],\n    enums: &[\n");
        for enumeration in &program.enums {
            let values: Vec<String> = enumeration.values().iter().map(|value| format!("{value:?}")).collect();
            let _ = writeln!(
                out,
                "        Enumeration {{ name: {:?}, values: &[{}] }},",
                enumeration.name(),
                values.join(", ")
            );
        }
        out.push_str("    ],\n    texts: &[\n");
        for text in &self.texts {
            let _ = writeln!(out, "        {text:?},");
        }
        out.push_str("    ],\n    names: &[\n");
        for function in &program.functions {
            let _ = writeln!(out, "        {:?},", function.name);
        }
        out.push_str("    ],\n    dynamic: &[\n");
        for index in 0..program.functions.len() as u32 {
            if self.dynamic.contains(&index) {
                let _ = writeln!(out, "        Some(d{index}),");
            } else {
                out.push_str("        None,\n");
            }
        }
        out.push_str("    ],\n    foreign: &[\n");
        for function in &program.foreign {
            let params: Vec<&str> = function.params.iter().map(|param| c_type(*param)).collect();
            let _ = writeln!(
                out,
                "        Foreign {{ library: {:?}, name: {:?}, params: &[{}], ret: {} }},",
                function.library,
                function.name,
                params.join(", "),
                c_type(function.ret)
            );
        }
        let _ = writeln!(
            out,
            "    ],\n    modules: {},\n    script: f{},\n}};\n",
            program.module_inits.len(),
            program.main.0
        );
        for (number, span) in self.spans.iter().enumerate() {
            let _ = writeln!(
                out,
                "const S{number}: Option<Span> = span({}, {}, {});",
                span.source.index(),
                span.start,
                span.end
            );
        }
        if !self.spans.is_empty() {
            out.push('\n');
        }
        let main = program.function(program.main);
        let mut statics: Vec<u32> = self.statics.iter().copied().collect();
        statics.sort_unstable();
        for global in &statics {
            let storage = Storage::of(&main.locals[*global as usize]);
            let zero = match storage {
                Storage::Plain(repr) => repr.zero(),
                _ => "Value::None",
            };
            let _ = writeln!(out, "static mut G{global}: {} = {zero};", storage.rust());
        }
        if !statics.is_empty() {
            out.push('\n');
        }
        out.push_str(&functions);
        out
    }

    /// The function `dN`, which calls `fN` with values: through a function value, or as
    /// the `equals` of a structure.
    fn wrapper(&self, index: u32) -> String {
        let function = &self.program.functions[index as usize];
        let defaults: HashSet<u32> = function.defaults.iter().map(|(index, _)| *index).collect();
        let mut args = Vec::new();
        for position in 0..function.params {
            let repr = match Storage::of(&function.locals[position as usize]) {
                Storage::Plain(repr) => repr,
                other => panic!("a function called through a value has a parameter held as {other:?}"),
            };
            if defaults.contains(&position) {
                args.push(format!("args.next().map(|value| {})", from_value("value", repr)));
            } else {
                args.push(format!(
                    "{{ let value = args.next().expect(\"the arguments are checked\"); {} }}",
                    from_value("value", repr)
                ));
            }
        }
        for capture in 0..function.captures {
            let local = &function.locals[(function.params + capture) as usize];
            let value = format!("captures[{capture}].clone()");
            args.push(match Storage::of(local) {
                Storage::Plain(repr) => from_value(&value, repr),
                Storage::Cell => value,
                Storage::Pointer(_) => panic!("a capture is not a `var` parameter"),
            });
        }
        let args: String = args.iter().map(|arg| format!(", {arg}")).collect();
        format!(
            "fn d{index}(rt: &mut Rt, args: Vec<Value>, captures: &[Value]) -> R<Value> {{\n    \
             let mut args = args.into_iter();\n    \
             let result = f{index}(rt{args})?;\n    \
             Ok({})\n}}\n\n",
            to_value("result", Repr::of(function.ret))
        )
    }

    fn span(&mut self, span: Span) -> String {
        let number = match self.span_numbers.get(&span) {
            Some(&number) => number,
            None => {
                self.spans.push(span);
                self.span_numbers.insert(span, self.spans.len() - 1);
                self.spans.len() - 1
            }
        };
        format!("S{number}")
    }

    fn text(&mut self, text: &str) -> usize {
        if let Some(&number) = self.text_numbers.get(text) {
            return number;
        }
        self.texts.push(text.to_string());
        self.text_numbers.insert(text.to_string(), self.texts.len() - 1);
        self.texts.len() - 1
    }

    /// The type number of each structure and enumeration among the members of `ty`,
    /// with its kind, as the virtual machine numbers them.
    fn named_types(&self, ty: Type) -> Vec<(u16, u32)> {
        ty.members()
            .into_iter()
            .filter_map(|member| match member {
                Type::Struct(structure) => Some((kinds::STRUCT, self.layouts[&structure] as u32)),
                Type::Enum(enumeration) => {
                    Some((kinds::ENUM, (self.program.structs.len() + self.enums[&enumeration]) as u32))
                }
                _ => None,
            })
            .collect()
    }
}

const HEADER: &str = "\
// The Rust code of a Lion program, made by `lion build` from its typed IR (spec §22).
#![allow(
    unused,
    unused_mut,
    unused_unsafe,
    unused_labels,
    unreachable_code,
    unreachable_patterns,
    non_snake_case,
    non_upper_case_globals,
    static_mut_refs,
    clippy::all
)]

use lion_native::*;

";

/// The label that leaves the turn of a parallel loop, as `break` does.
const TURN: &str = "'turn";

/// The locals that the statements write: assign, change in place, go through with
/// `for`, or give to a `var` parameter.
fn written_locals(stmts: &[Stmt], written: &mut BTreeSet<u32>) {
    for stmt in stmts {
        match stmt {
            Stmt::Assign { place: Place::Local(local), .. }
            | Stmt::AssignElement { root: Place::Local(local), .. }
            | Stmt::Add { root: Place::Local(local), .. }
            | Stmt::Remove { root: Place::Local(local), .. }
            | Stmt::Declare { local } => {
                written.insert(local.0);
            }
            Stmt::For { var, body, .. } => {
                written.insert(var.0);
                written_locals(body, written);
            }
            Stmt::Parallel(parallel) => {
                written.insert(parallel.var.0);
                written.extend(parallel.gather.map(|gather| gather.0));
                written_locals(&parallel.body, written);
            }
            Stmt::If { then, otherwise, .. } => {
                written_locals(then, written);
                written_locals(otherwise, written);
            }
            Stmt::While { body, .. } | Stmt::Seq(body) => written_locals(body, written),
            _ => {}
        }
    }
    ir::visit::exprs_in_stmts(stmts, &mut |expr| match &expr.kind {
        ExprKind::Let { local, .. } => {
            written.insert(local.0);
        }
        ExprKind::Call { args, .. } => {
            for arg in args {
                if let Arg::Reference(Place::Local(local)) = arg {
                    written.insert(local.0);
                }
            }
        }
        ExprKind::Block { stmts, .. } => written_locals(stmts, written),
        _ => {}
    });
}

/// The locals that the expressions of the statements read.
fn read_locals(stmts: &[Stmt], read: &mut BTreeSet<u32>) {
    ir::visit::exprs_in_stmts(stmts, &mut |expr| {
        if let ExprKind::Local(local) | ExprKind::Cell(local) = expr.kind {
            read.insert(local.0);
        }
    });
}

/// The globals that the statements assign, or change in place.
fn globals_assigned(stmts: &[Stmt], statics: &mut HashSet<u32>) {
    for stmt in stmts {
        match stmt {
            Stmt::Assign { place: Place::Global(global), .. }
            | Stmt::AssignElement { root: Place::Global(global), .. }
            | Stmt::Add { root: Place::Global(global), .. }
            | Stmt::Remove { root: Place::Global(global), .. } => {
                statics.insert(global.0);
            }
            Stmt::If { then, otherwise, .. } => {
                globals_assigned(then, statics);
                globals_assigned(otherwise, statics);
            }
            Stmt::While { body, .. } | Stmt::For { body, .. } | Stmt::Seq(body) => {
                globals_assigned(body, statics);
            }
            Stmt::Parallel(parallel) => globals_assigned(&parallel.body, statics),
            _ => {}
        }
    }
}

fn c_type(ty: Type) -> &'static str {
    match ty {
        Type::Int => "CType::Int",
        Type::Float => "CType::Float",
        Type::Bool => "CType::Bool",
        Type::Text => "CType::Text",
        Type::None => "CType::None",
        _ => "CType::MaybeText",
    }
}

/// The code that turns the `Value` `value` into `repr`.
fn from_value(value: &str, repr: Repr) -> String {
    match repr {
        Repr::Int => format!("int(&{value})"),
        Repr::Float => format!("float(&{value})"),
        Repr::Bool => format!("boolean(&{value})"),
        Repr::Value => value.to_string(),
    }
}

/// The code that turns `value`, held as `repr`, into a `Value`.
fn to_value(value: &str, repr: Repr) -> String {
    match repr {
        Repr::Int => format!("Value::Int({value})"),
        Repr::Float => format!("Value::Float({value})"),
        Repr::Bool => format!("Value::Bool({value})"),
        Repr::Value => value.to_string(),
    }
}

fn int_literal(value: i64) -> String {
    if value == i64::MIN { "i64::MIN".to_string() } else { format!("({value}i64)") }
}

fn float_literal(value: f64) -> String {
    if value.is_finite() {
        format!("({value:?}f64)")
    } else {
        format!("f64::from_bits({:#x})", value.to_bits())
    }
}

/// The translation of one function.
struct FunctionGen<'g, 'p> {
    g: &'g mut Generator<'p>,
    function: &'p ir::Function,
    index: usize,
    is_script: bool,
    out: String,
    indent: usize,
    next: usize,
    /// The loops being translated, innermost last: the labels to leave the loop and to
    /// leave the turn. The turns of a parallel loop are left with `TURN`.
    loops: Vec<(String, String)>,
    /// How many turns of parallel loops the code being translated is in.
    turns: usize,
}

impl<'g, 'p> FunctionGen<'g, 'p> {
    fn new(g: &'g mut Generator<'p>, index: usize, function: &'p ir::Function) -> FunctionGen<'g, 'p> {
        let is_script = index == g.program.main.index();
        FunctionGen {
            g,
            function,
            index,
            is_script,
            out: String::new(),
            indent: 2,
            next: 0,
            loops: Vec::new(),
            turns: 0,
        }
    }

    fn generate(mut self) -> String {
        let function = self.function;
        let defaults: HashSet<u32> = function.defaults.iter().map(|(index, _)| *index).collect();
        let mut params = String::new();
        for position in 0..function.params + function.captures {
            let local = &function.locals[position as usize];
            let storage = Storage::of(local);
            if defaults.contains(&position) {
                let _ = write!(params, ", p{position}: Option<{}>", storage.rust());
            } else {
                let _ = write!(params, ", mut l{position}: {}", storage.rust());
            }
        }
        let ret = if self.is_script { Repr::Value } else { Repr::of(function.ret) };
        let mut head = format!("/// {}\n", function.name.replace('\n', " "));
        let _ = writeln!(head, "fn f{}(rt: &mut Rt{params}) -> R<{}> {{", self.index, ret.rust());
        head.push_str("    unsafe {\n");
        for (position, local) in function.locals.iter().enumerate() {
            let position = position as u32;
            let is_param = position < function.params + function.captures && !defaults.contains(&position);
            if is_param || (self.is_script && self.g.statics.contains(&position)) {
                continue;
            }
            let storage = Storage::of(local);
            let zero = match storage {
                Storage::Plain(repr) => repr.zero(),
                Storage::Cell => "Value::None",
                Storage::Pointer(_) => panic!("only a parameter is a `var` parameter"),
            };
            let _ = writeln!(head, "        let mut l{position}: {} = {zero};", storage.rust());
        }
        // Omitted arguments take their default value, in the order of the parameters (§11.2).
        for (position, value) in &function.defaults {
            self.line(format!("if let Some(value) = p{position} {{"));
            self.line(format!("    l{position} = value;"));
            self.line("} else {");
            self.indent += 1;
            let op = self.expr(value);
            self.store_local(ir::LocalId(*position), op);
            self.indent -= 1;
            self.line("}");
        }
        for stmt in &function.body {
            self.stmt(stmt);
        }
        match ret {
            Repr::Value => self.line("Ok(Value::None)"),
            _ => self.line("unreachable!(\"a function that gives a value returns on every path\")"),
        }
        head.push_str(&self.out);
        head.push_str("    }\n}\n\n");
        head
    }

    fn line(&mut self, line: impl AsRef<str>) {
        for _ in 0..self.indent {
            self.out.push_str("    ");
        }
        self.out.push_str(line.as_ref());
        self.out.push('\n');
    }

    fn fresh(&mut self, prefix: &str) -> String {
        self.next += 1;
        format!("{prefix}{}", self.next)
    }

    /// A new temporary holding `code`.
    fn temp(&mut self, repr: Repr, code: impl AsRef<str>) -> Op {
        let name = self.fresh("t");
        self.line(format!("let {name}: {} = {};", repr.rust(), code.as_ref()));
        Op::owned(name, repr)
    }

    fn span(&mut self, span: Span) -> String {
        self.g.span(span)
    }

    /// The value, held as `want`.
    fn coerce(&mut self, op: Op, want: Repr) -> Op {
        if op.repr == want {
            return op;
        }
        let code = match (op.repr, want) {
            (_, Repr::Value) => to_value(&op.take(), op.repr),
            (Repr::Value, _) => from_value(&op.code, want),
            (from, to) => panic!("a value held as {from:?} cannot be held as {to:?}"),
        };
        self.temp(want, code)
    }

    /// A number or a Bool.
    fn scalar(&mut self, expr: &ir::Expr, repr: Repr) -> String {
        let op = self.expr(expr);
        self.coerce(op, repr).code
    }

    /// The Rust place of a local of this function, which holds its storage.
    fn local_var(&self, local: ir::LocalId) -> String {
        if self.is_script && self.g.statics.contains(&local.0) {
            format!("(*&raw mut G{})", local.0)
        } else {
            format!("l{}", local.0)
        }
    }

    fn global_storage(&self, global: ir::LocalId) -> Storage {
        Storage::of(self.g.program.function(self.g.program.main).local(global))
    }

    fn read_local(&mut self, local: ir::LocalId) -> Op {
        let var = self.local_var(local);
        let storage = Storage::of(self.function.local(local));
        self.read(var, storage)
    }

    fn read(&mut self, var: String, storage: Storage) -> Op {
        match storage {
            Storage::Plain(repr) => Op::place(var, repr),
            Storage::Cell => self.temp(Repr::Value, format!("cell_get(&{var})")),
            Storage::Pointer(repr) => Op::place(format!("(*{var})"), repr),
        }
    }

    fn store_local(&mut self, local: ir::LocalId, op: Op) {
        let var = self.local_var(local);
        let storage = Storage::of(self.function.local(local));
        self.store(var, storage, op);
    }

    fn store(&mut self, var: String, storage: Storage, op: Op) {
        match storage {
            Storage::Plain(repr) => {
                let op = self.coerce(op, repr);
                self.line(format!("{var} = {};", op.take()));
            }
            Storage::Cell => {
                let op = self.coerce(op, Repr::Value);
                self.line(format!("cell_set(&{var}, {});", op.take()));
            }
            Storage::Pointer(repr) => {
                let op = self.coerce(op, repr);
                self.line(format!("*{var} = {};", op.take()));
            }
        }
    }

    // Statements.

    fn stmts(&mut self, stmts: &[Stmt]) {
        for stmt in stmts {
            self.stmt(stmt);
        }
    }

    fn stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Assign { place, value } => {
                let op = self.expr(value);
                match place {
                    Place::Local(local) => self.store_local(*local, op),
                    Place::Global(global) => {
                        let storage = self.global_storage(*global);
                        self.store(format!("(*&raw mut G{})", global.0), storage, op);
                    }
                }
            }
            Stmt::Declare { local } => {
                if self.function.local(*local).boxed {
                    let var = self.local_var(*local);
                    self.line(format!("{var} = new_cell();"));
                }
            }
            Stmt::Expr(expr) => {
                self.expr(expr);
            }
            Stmt::If { cond, then, otherwise } => {
                let cond = self.scalar(cond, Repr::Bool);
                self.line(format!("if {cond} {{"));
                self.block(then);
                if !otherwise.is_empty() {
                    self.line("} else {");
                    self.block(otherwise);
                }
                self.line("}");
            }
            Stmt::While { cond, body } => {
                let (exit, turn) = (self.fresh("'b"), self.fresh("'c"));
                self.line(format!("{exit}: loop {{"));
                self.indent += 1;
                let cond = self.scalar(cond, Repr::Bool);
                self.line(format!("if !{cond} {{ break {exit}; }}"));
                self.turn(&exit, &turn, body);
                self.indent -= 1;
                self.line("}");
            }
            Stmt::For { var, iterable, body } if iterable.ty == Type::Range => {
                let (start, end) = match &iterable.kind {
                    ExprKind::Range { start, end } => {
                        let a = self.operand_before(start, &[end]);
                        let b = self.expr(end);
                        let (a, b) = (self.coerce(a, Repr::Int), self.coerce(b, Repr::Int));
                        (a.code, b.code)
                    }
                    _ => {
                        let range = self.expr(iterable);
                        let bounds = self.fresh("t");
                        self.line(format!("let {bounds} = range({});", range.by_ref()));
                        (format!("{bounds}[0]"), format!("{bounds}[1]"))
                    }
                };
                let (first, last, counter) = (self.fresh("t"), self.fresh("t"), self.fresh("t"));
                self.line(format!("let ({first}, {last}): (i64, i64) = ({start}, {end});"));
                self.line(format!("if {first} <= {last} {{"));
                self.indent += 1;
                self.line(format!("let mut {counter}: i64 = {first};"));
                let (exit, turn) = (self.fresh("'b"), self.fresh("'c"));
                self.line(format!("{exit}: loop {{"));
                self.indent += 1;
                self.store_local(*var, Op::owned(counter.clone(), Repr::Int));
                self.turn(&exit, &turn, body);
                self.line(format!("if {counter} >= {last} {{ break {exit}; }}"));
                self.line(format!("{counter} += 1;"));
                self.indent -= 1;
                self.line("}");
                self.indent -= 1;
                self.line("}");
            }
            Stmt::For { var, iterable, body } => {
                let sequence = self.expr(iterable);
                let sequence = self.coerce(sequence, Repr::Value);
                let (values, size, position) = (self.fresh("t"), self.fresh("t"), self.fresh("t"));
                self.line(format!("let {values}: Value = {};", sequence.take()));
                self.line(format!("let {size}: usize = shared::sequence_len(&{values});"));
                self.line(format!("let mut {position}: usize = 0;"));
                let (exit, turn) = (self.fresh("'b"), self.fresh("'c"));
                self.line(format!("{exit}: while {position} < {size} {{"));
                self.indent += 1;
                let element = Op::owned(format!("shared::sequence_at(&{values}, {position})"), Repr::Value);
                let element = self.temp(Repr::Value, element.code);
                self.store_local(*var, element);
                self.turn(&exit, &turn, body);
                self.line(format!("{position} += 1;"));
                self.indent -= 1;
                self.line("}");
            }
            Stmt::Parallel(parallel) => self.parallel(parallel),
            Stmt::Seq(stmts) => self.stmts(stmts),
            Stmt::InitModule { module } => self.init_module(*module),
            Stmt::AssignElement { root, path, value } => {
                self.change(*root, path, value, |steps, value, root| {
                    format!("shared::store_element(&mut rt.at(SPAN), {root}, &{steps}, {})", value.take())
                });
            }
            Stmt::Add { root, path, value } => {
                self.change(*root, path, value, |steps, value, root| {
                    format!("shared::add_element(&mut rt.at(SPAN), {root}, &{steps}, {})", value.take())
                });
            }
            Stmt::Remove { root, path, key } => {
                self.change(*root, path, key, |steps, key, root| {
                    format!("shared::remove_element(&mut rt.at(SPAN), {root}, &{steps}, {})", key.by_ref())
                });
            }
            Stmt::Break => {
                let (exit, _) = self.loops.last().expect("`break` is inside a loop").clone();
                if exit == TURN {
                    self.line("return Ok(TurnExit::Break);");
                } else {
                    self.line(format!("break {exit};"));
                }
            }
            Stmt::Continue => {
                let (_, turn) = self.loops.last().expect("`continue` is inside a loop").clone();
                self.line(format!("break {turn};"));
            }
            Stmt::Return(value) => self.ret(value.as_ref()),
        }
    }

    fn block(&mut self, stmts: &[Stmt]) {
        self.indent += 1;
        self.stmts(stmts);
        self.indent -= 1;
    }

    /// The body of a loop, in a block that `continue` leaves.
    fn turn(&mut self, exit: &str, turn: &str, body: &[Stmt]) {
        self.line(format!("{turn}: {{"));
        self.loops.push((exit.to_string(), turn.to_string()));
        self.block(body);
        self.loops.pop();
        self.line("}");
    }

    fn ret(&mut self, value: Option<&ir::Expr>) {
        match value {
            Some(value) => {
                let op = self.expr(value);
                self.leave_function(op);
            }
            // `return` in the script ends it (§20.1).
            None if self.is_script && self.turns > 0 => self.line("return Ok(TurnExit::Halt);"),
            None => self.leave_function(Op::none()),
        }
    }

    /// Leaves the function with the value; from a turn of a parallel loop, the turn
    /// gives the value to the loop, which leaves the function (§19.2).
    fn leave_function(&mut self, op: Op) {
        if self.turns > 0 {
            let op = self.coerce(op, Repr::Value);
            self.line(format!("return Ok(TurnExit::Return({}));", op.take()));
        } else {
            let op = self.coerce(op, Repr::of(self.function.ret));
            self.line(format!("return Ok({});", op.take()));
        }
    }

    fn init_module(&mut self, module: u32) {
        self.line(format!("if rt.begin_module({module}) {{"));
        if let Some(init) = self.g.program.module_inits[module as usize] {
            self.line("    rt.enter(None)?;");
            let result = self.fresh("t");
            self.line(format!("    let {result} = f{}(rt);", init.0));
            self.line(format!("    rt.leave({result}, None)?;"));
        }
        self.line("}");
    }

    /// A parallel loop (§19.2): `rt.parallel` runs its turns, in chunks, with a closure
    /// that runs the turns of a chunk. A closure only reads the variables around it: those
    /// that the turns write are its own, and the pointers of `var` parameters are wrapped
    /// to be read by several threads. Turns that must run in their order are an ordinary
    /// loop (C84).
    fn parallel(&mut self, parallel: &ir::ParallelLoop) {
        let program = self.g.program;
        let reach = ir::parallel::reach(program, self.function, parallel);
        if reach.in_order {
            let as_loop = Stmt::For {
                var: parallel.var,
                iterable: parallel.iterable.clone(),
                body: parallel.body.clone(),
            };
            self.stmt(&as_loop);
            return;
        }
        let span = self.span(parallel.iterable.span);
        let sequence = self.expr(&parallel.iterable);
        let sequence = self.coerce(sequence, Repr::Value);
        let sequence = if sequence.place { self.temp(Repr::Value, sequence.take()) } else { sequence };
        for module in &reach.modules {
            self.init_module(*module);
        }
        let turns = self.fresh("t");
        self.line(format!("let {turns}: usize = turn_count(&{});", sequence.code));
        let mut written = BTreeSet::new();
        written.insert(parallel.var.0);
        written.extend(parallel.gather.map(|gather| gather.0));
        written_locals(&parallel.body, &mut written);
        let mut pointers = BTreeSet::new();
        read_locals(&parallel.body, &mut pointers);
        pointers.retain(|local| {
            !written.contains(local)
                && matches!(Storage::of(self.function.local(ir::LocalId(*local))), Storage::Pointer(_))
        });
        for local in &pointers {
            self.line(format!("let w{local} = Shared(l{local});"));
        }
        let gather = match parallel.gather {
            Some(gather) => format!("Some(&mut {})", self.local_var(gather)),
            None => "None".to_string(),
        };
        let exit = self.fresh("t");
        self.line(format!(
            "let {exit} = rt.parallel({turns}, {gather}, {span}, &|rt: &mut Rt, first: usize, last: usize, gathered: &mut Value| -> R<TurnExit> {{"
        ));
        self.indent += 1;
        self.line("unsafe {");
        self.indent += 1;
        for local in &pointers {
            self.line(format!("let l{local} = w{local}.0;"));
        }
        for local in &written {
            if self.is_script && self.g.statics.contains(local) {
                continue;
            }
            let storage = Storage::of(self.function.local(ir::LocalId(*local)));
            let zero = match storage {
                Storage::Plain(repr) => repr.zero(),
                _ => "Value::None",
            };
            self.line(format!("let mut l{local}: {} = {zero};", storage.rust()));
        }
        if let Some(gather) = parallel.gather {
            let var = self.local_var(gather);
            self.line(format!("{var} = std::mem::take(gathered);"));
        }
        let counter = self.fresh("t");
        self.line(format!("let mut {counter}: usize = first;"));
        let (exit_label, turn) = (self.fresh("'b"), self.fresh("'c"));
        self.line(format!("{exit_label}: while {counter} < last {{"));
        self.indent += 1;
        let value = self.temp(Repr::Value, format!("turn_value(&{}, {counter})", sequence.code));
        self.store_local(parallel.var, value);
        self.turns += 1;
        self.turn(TURN, &turn, &parallel.body);
        self.turns -= 1;
        self.line(format!("{counter} += 1;"));
        self.indent -= 1;
        self.line("}");
        if let Some(gather) = parallel.gather {
            let var = self.local_var(gather);
            self.line(format!("*gathered = std::mem::take(&mut {var});"));
        }
        self.line("Ok(TurnExit::End)");
        self.indent -= 1;
        self.line("}");
        self.indent -= 1;
        self.line("})?;");
        // The first turn that left the loop decides how it ends.
        let value = self.fresh("t");
        self.line(format!("match {exit} {{"));
        self.indent += 1;
        self.line("TurnExit::End | TurnExit::Break => {}");
        self.line(format!("TurnExit::Return({value}) => {{"));
        self.indent += 1;
        self.leave_function(Op::owned(value, Repr::Value));
        self.indent -= 1;
        self.line("}");
        if self.turns > 0 {
            self.line("TurnExit::Halt => return Ok(TurnExit::Halt),");
        } else if self.is_script {
            self.line("TurnExit::Halt => return Ok(Value::None),");
        } else {
            self.line("TurnExit::Halt => unreachable!(\"only the script ends the program with `return`\"),");
        }
        self.indent -= 1;
        self.line("}");
    }

    /// A change in place of a part of the variable `root` (§6.3): the steps, then the
    /// value, as the virtual machine evaluates them. `call` gives the shared operation,
    /// from the steps, the value and the root.
    fn change(
        &mut self,
        root: Place,
        path: &[ir::Step],
        value: &ir::Expr,
        call: impl FnOnce(&str, &Op, &str) -> String,
    ) {
        let mut steps = Vec::new();
        for step in path {
            steps.push(match step {
                ir::Step::Index(index) => {
                    let op = self.expr(index);
                    let op = self.coerce(op, Repr::Value);
                    let op = if op.place { self.temp(Repr::Value, op.take()) } else { op };
                    op.code
                }
                ir::Step::Field(field) => format!("Value::Int({field})"),
            });
        }
        let op = self.expr(value);
        let op = self.coerce(op, Repr::Value);
        let steps_name = self.fresh("t");
        self.line(format!("let {steps_name}: [Value; {}] = [{}];", steps.len(), steps.join(", ")));
        let span = self.span(value.span);
        let result = self.fresh("t");
        let (var, storage) = match root {
            Place::Local(local) => (self.local_var(local), Storage::of(self.function.local(local))),
            Place::Global(global) => (format!("(*&raw mut G{})", global.0), self.global_storage(global)),
        };
        match storage {
            Storage::Plain(_) => {
                let code = call(&steps_name, &op, &format!("&mut {var}")).replace("SPAN", &span);
                self.line(format!("let {result} = {code};"));
            }
            Storage::Pointer(_) => {
                let code = call(&steps_name, &op, &format!("&mut *{var}")).replace("SPAN", &span);
                self.line(format!("let {result} = {code};"));
            }
            // The value leaves the cell while it changes, so that it is not copied.
            Storage::Cell => {
                let whole = self.fresh("t");
                self.line(format!("let mut {whole}: Value = cell_take(&{var});"));
                let code = call(&steps_name, &op, &format!("&mut {whole}")).replace("SPAN", &span);
                self.line(format!("let {result} = {code};"));
                self.line(format!("cell_set(&{var}, {whole});"));
            }
        }
        self.line(format!("rt.stop({result}, {span})?;"));
    }

    // Expressions.

    /// Like [`FunctionGen::expr`], for an operand evaluated before `later`: if one of
    /// them calls a function, which may change the variable, its current value is taken
    /// now, so that operands keep the values they had from left to right (§9.2).
    fn operand_before(&mut self, expr: &ir::Expr, later: &[&ir::Expr]) -> Op {
        let op = self.expr(expr);
        if op.place && later.iter().any(|later| calls_function(later)) {
            let code = op.take();
            return self.temp(op.repr, code);
        }
        op
    }

    /// The values of several expressions, evaluated in order.
    fn operands(&mut self, exprs: &[&ir::Expr]) -> Vec<Op> {
        (0..exprs.len()).map(|index| self.operand_before(exprs[index], &exprs[index + 1..])).collect()
    }

    /// The values of several expressions, evaluated in order, as `Value`s.
    fn values(&mut self, exprs: &[&ir::Expr]) -> Vec<String> {
        let ops = self.operands(exprs);
        ops.into_iter().map(|op| self.coerce(op, Repr::Value).take()).collect()
    }

    /// The value of the expression, held as its type says.
    fn expr(&mut self, expr: &ir::Expr) -> Op {
        let op = self.expr_raw(expr);
        // The cell of a variable is a `Value`, whatever the type of the variable.
        if matches!(expr.kind, ExprKind::Cell(_)) {
            return op;
        }
        self.coerce(op, Repr::of(expr.ty))
    }

    fn expr_raw(&mut self, expr: &ir::Expr) -> Op {
        let ty = expr.ty;
        match &expr.kind {
            ExprKind::Int(value) => Op::owned(int_literal(*value), Repr::Int),
            ExprKind::Float(value) => Op::owned(float_literal(*value), Repr::Float),
            ExprKind::Bool(value) => Op::owned(value.to_string(), Repr::Bool),
            ExprKind::None => Op::none(),
            ExprKind::Text(text) => {
                let number = self.g.text(text);
                self.temp(Repr::Value, format!("rt.text({number})"))
            }
            ExprKind::Local(local) => self.read_local(*local),
            ExprKind::Global(global) => {
                let storage = self.global_storage(*global);
                self.read(format!("(*&raw mut G{})", global.0), storage)
            }
            ExprKind::Cell(local) => Op::place(self.local_var(*local), Repr::Value),
            ExprKind::Call { function, args } => self.call(*function, args, expr.span),
            ExprKind::Let { local, value, body } => {
                let op = self.expr(value);
                self.store_local(*local, op);
                self.expr(body)
            }
            ExprKind::Block { stmts, value } => {
                self.stmts(stmts);
                self.expr(value)
            }
            ExprKind::Compile(value) => self.expr(value),
            ExprKind::Unary { op, operand } => {
                let span = self.span(expr.span);
                let a = self.expr(operand);
                match op {
                    UnaryOp::NegInt => self.temp(Repr::Int, format!("rt.check(ops::int_neg({}), {span})?", a.code)),
                    UnaryOp::NegFloat => self.temp(Repr::Float, format!("-{}", a.code)),
                    UnaryOp::NegRational => self.temp(
                        Repr::Value,
                        format!(
                            "new_rational(rt.check(ops::rational_op(RationalOp::Subtract, [0, 1], rational({})), {span})?)",
                            a.by_ref()
                        ),
                    ),
                    UnaryOp::Not => self.temp(Repr::Bool, format!("!{}", a.code)),
                }
            }
            ExprKind::Binary { op: op @ (BinaryOp::EqNone | BinaryOp::NeNone), lhs, rhs } => {
                // `none` equals `none`; the operands are still evaluated for their effects.
                self.operand_before(lhs, &[rhs]);
                self.expr(rhs);
                Op::owned((*op == BinaryOp::EqNone).to_string(), Repr::Bool)
            }
            ExprKind::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs, expr.span),
            ExprKind::And { lhs, rhs } | ExprKind::Or { lhs, rhs } => {
                let first = self.scalar(lhs, Repr::Bool);
                let result = self.fresh("t");
                self.line(format!("let mut {result}: bool = {first};"));
                let test = if matches!(expr.kind, ExprKind::And { .. }) { "" } else { "!" };
                self.line(format!("if {test}{result} {{"));
                self.indent += 1;
                let second = self.scalar(rhs, Repr::Bool);
                self.line(format!("{result} = {second};"));
                self.indent -= 1;
                self.line("}");
                Op::owned(result, Repr::Bool)
            }
            ExprKind::Convert { conversion, value } => self.convert(*conversion, value, expr.span),
            ExprKind::If { cond, then, otherwise } => {
                let cond = self.scalar(cond, Repr::Bool);
                let repr = Repr::of(ty);
                let result = self.fresh("t");
                self.line(format!("let {result}: {};", repr.rust()));
                self.line(format!("if {cond} {{"));
                for (branch, last) in [(then, false), (otherwise, true)] {
                    self.indent += 1;
                    let op = self.expr(branch);
                    let op = self.coerce(op, repr);
                    self.line(format!("{result} = {};", op.take()));
                    self.indent -= 1;
                    self.line(if last { "}" } else { "} else {" });
                }
                Op::owned(result, repr)
            }
            ExprKind::Range { start, end } => {
                let a = self.operand_before(start, &[end]);
                let b = self.expr(end);
                self.temp(Repr::Value, format!("new_range({}, {})", a.code, b.code))
            }
            ExprKind::List(elements) => {
                let values = self.values(&elements.iter().collect::<Vec<_>>());
                self.temp(Repr::Value, format!("new_list(vec![{}])", values.join(", ")))
            }
            ExprKind::Tuple(elements) => {
                let values = self.values(&elements.iter().collect::<Vec<_>>());
                self.temp(Repr::Value, format!("new_tuple(vec![{}])", values.join(", ")))
            }
            ExprKind::Set(elements) => {
                let values = self.values(&elements.iter().collect::<Vec<_>>());
                let span = self.span(expr.span);
                let result = self.fresh("t");
                self.line(format!(
                    "let {result} = shared::make_set(&mut rt.at({span}), vec![{}]);",
                    values.join(", ")
                ));
                self.temp(Repr::Value, format!("new_set(rt.stop({result}, {span})?)"))
            }
            ExprKind::Map(entries) => {
                let exprs: Vec<&ir::Expr> = entries.iter().flat_map(|(key, value)| [key, value]).collect();
                let values = self.values(&exprs);
                let pairs: Vec<String> =
                    values.chunks(2).map(|pair| format!("({}, {})", pair[0], pair[1])).collect();
                let span = self.span(expr.span);
                let result = self.fresh("t");
                self.line(format!(
                    "let {result} = shared::make_map(&mut rt.at({span}), vec![{}]);",
                    pairs.join(", ")
                ));
                self.temp(Repr::Value, format!("new_map(rt.stop({result}, {span})?)"))
            }
            ExprKind::Index { object, index } => {
                let span = self.span(expr.span);
                let o = self.operand_before(object, &[index]);
                let i = self.expr(index);
                if matches!(object.ty, Type::Map(_)) {
                    let i = self.coerce(i, Repr::Value);
                    let result = self.fresh("t");
                    self.line(format!(
                        "let {result} = shared::map_index(&mut rt.at({span}), map({}), {});",
                        o.by_ref(),
                        i.by_ref()
                    ));
                    self.temp(Repr::Value, format!("rt.stop({result}, {span})?"))
                } else {
                    let i = self.coerce(i, Repr::Int);
                    self.temp(
                        Repr::Value,
                        format!("rt.check(shared::get_index({}, {}), {span})?", o.by_ref(), i.code),
                    )
                }
            }
            ExprKind::Slice { object, range } => {
                let span = self.span(expr.span);
                let o = self.operand_before(object, &[range]);
                let r = self.expr(range);
                self.temp(
                    Repr::Value,
                    format!("rt.check(shared::get_slice({}, range({})), {span})?", o.by_ref(), r.by_ref()),
                )
            }
            ExprKind::Property { object, property } => {
                let span = self.span(expr.span);
                let o = self.expr(object);
                let o = self.coerce(o, Repr::Value);
                match property {
                    ir::Property::Size => {
                        self.temp(Repr::Int, format!("rt.check(shared::size({}), {span})?", o.by_ref()))
                    }
                    ir::Property::First => {
                        self.temp(Repr::Value, format!("rt.check(shared::first({}), {span})?", o.by_ref()))
                    }
                    ir::Property::Last => {
                        self.temp(Repr::Value, format!("rt.check(shared::last({}), {span})?", o.by_ref()))
                    }
                }
            }
            ExprKind::TypeTest { value, ty: part } => {
                let op = self.expr(value);
                let op = self.coerce(op, Repr::Value);
                let code = self.type_test(value.ty, *part, &op.by_ref());
                self.temp(Repr::Bool, code)
            }
            ExprKind::Try(value) => {
                let op = self.expr(value);
                let op = self.coerce(op, Repr::Value);
                let op = if op.place { self.temp(Repr::Value, op.take()) } else { op };
                // The errors of the program are structures or enumerations (§18.2).
                let result = ty.members();
                let members = value.ty.members();
                let errors: Vec<String> = self
                    .g
                    .named_types(value.ty)
                    .into_iter()
                    .zip(
                        members
                            .into_iter()
                            .filter(|member| matches!(member, Type::Struct(_) | Type::Enum(_))),
                    )
                    .filter(|(_, member)| !result.contains(member))
                    .map(|((_, number), _)| number.to_string())
                    .collect();
                let span = self.span(expr.span);
                let leave = if self.is_script {
                    format!("return Err(failure(&{}, {span}));", op.code)
                } else if self.turns > 0 {
                    format!("return Ok(TurnExit::Return({}));", op.code)
                } else {
                    format!("return Ok({});", op.code)
                };
                self.line(format!(
                    "if shared::is_failure(&{}, &[{}]) {{ {leave} }}",
                    op.code,
                    errors.join(", ")
                ));
                op
            }
            ExprKind::Concat(parts) => {
                let ops = self.operands(&parts.iter().collect::<Vec<_>>());
                let mut refs = Vec::new();
                for op in ops {
                    let op = self.coerce(op, Repr::Value);
                    refs.push(op.by_ref());
                }
                self.temp(Repr::Value, format!("concat(&[{}])", refs.join(", ")))
            }
            ExprKind::CallBuiltin { builtin, args } => self.builtin(*builtin, args, expr),
            ExprKind::Struct { structure, fields } => {
                let values = self.values(&fields.iter().collect::<Vec<_>>());
                let layout = self.g.layouts[structure];
                self.temp(Repr::Value, format!("rt.record({layout}, vec![{}])", values.join(", ")))
            }
            ExprKind::Field { object, field } => {
                let o = self.expr(object);
                let o = self.coerce(o, Repr::Value);
                self.temp(Repr::Value, format!("field({}, {field})", o.by_ref()))
            }
            ExprKind::Enum { enumeration, value } => {
                let number = self.g.enums[enumeration];
                self.temp(Repr::Value, format!("rt.enumeration({number}, {value})"))
            }
            ExprKind::Closure { function, captures } => {
                self.g.dynamic.insert(function.0);
                let values = self.values(&captures.iter().collect::<Vec<_>>());
                self.temp(Repr::Value, format!("rt.closure({}, vec![{}])", function.0, values.join(", ")))
            }
            ExprKind::CallValue { callee, args } | ExprKind::Partial { callee, args } => {
                let mut exprs: Vec<&ir::Expr> = vec![callee];
                exprs.extend(args.iter());
                let ops = self.operands(&exprs);
                let mut ops = ops.into_iter();
                let callee = ops.next().expect("the callee");
                let callee = self.coerce(callee, Repr::Value);
                let values: Vec<String> = ops.map(|op| self.coerce(op, Repr::Value).take()).collect();
                if matches!(expr.kind, ExprKind::CallValue { .. }) {
                    let span = self.span(expr.span);
                    self.temp(
                        Repr::Value,
                        format!("rt.call_value({}, vec![{}], {span})?", callee.by_ref(), values.join(", ")),
                    )
                } else {
                    self.temp(Repr::Value, format!("bind({}, vec![{}])", callee.by_ref(), values.join(", ")))
                }
            }
            ExprKind::Task(value) => {
                let op = self.expr(value);
                let op = self.coerce(op, Repr::Value);
                self.temp(Repr::Value, format!("new_task({})", op.take()))
            }
            ExprKind::Wait(value) => {
                let op = self.expr(value);
                self.temp(Repr::Value, format!("wait({})", op.by_ref()))
            }
        }
    }

    /// A call of a function of the program (§11): the arguments, then the call.
    fn call(&mut self, function: ir::FunctionId, args: &[Arg], span: Span) -> Op {
        let program = self.g.program;
        let callee = program.function(function);
        let defaults: HashSet<u32> = callee.defaults.iter().map(|(index, _)| *index).collect();
        let values: Vec<&ir::Expr> = args
            .iter()
            .filter_map(|arg| match arg {
                Arg::Value(value) => Some(value),
                Arg::Reference(_) => None,
            })
            .collect();
        let mut codes = Vec::new();
        let mut write_backs = Vec::new();
        let mut value_number = 0;
        for (position, arg) in args.iter().enumerate() {
            let param = &callee.locals[position];
            match arg {
                Arg::Value(value) => {
                    value_number += 1;
                    let op = self.operand_before(value, &values[value_number..]);
                    let repr = match Storage::of(param) {
                        Storage::Plain(repr) => repr,
                        Storage::Cell => Repr::Value,
                        Storage::Pointer(_) => panic!("a value given to a `var` parameter"),
                    };
                    let op = self.coerce(op, repr);
                    let code = op.take();
                    if defaults.contains(&(position as u32)) {
                        codes.push(format!("Some({code})"));
                    } else {
                        codes.push(code);
                    }
                }
                Arg::Reference(place) => {
                    let Storage::Pointer(repr) = Storage::of(param) else {
                        panic!("a variable given to a parameter that is not `var`")
                    };
                    let code = match place {
                        Place::Local(local) => match Storage::of(self.function.local(*local)) {
                            Storage::Pointer(_) => format!("l{}", local.0),
                            // A variable in a cell goes through a copy, stored back after
                            // the call (C50).
                            Storage::Cell => {
                                let var = self.local_var(*local);
                                let copy = self.fresh("t");
                                self.line(format!(
                                    "let mut {copy}: {} = {};",
                                    repr.rust(),
                                    from_value(&format!("cell_get(&{var})"), repr)
                                ));
                                write_backs.push((var, copy.clone(), repr));
                                format!("&raw mut {copy}")
                            }
                            Storage::Plain(_) if self.is_script && self.g.statics.contains(&local.0) => {
                                format!("&raw mut G{}", local.0)
                            }
                            Storage::Plain(_) => format!("&raw mut l{}", local.0),
                        },
                        Place::Global(global) => format!("&raw mut G{}", global.0),
                    };
                    codes.push(code);
                }
            }
        }
        for _ in args.len()..callee.params as usize {
            codes.push("None".to_string());
        }
        let span = self.span(span);
        let args: String = codes.iter().map(|code| format!(", {code}")).collect();
        self.line(format!("rt.enter({span})?;"));
        let result = self.fresh("t");
        self.line(format!("let {result} = f{}(rt{args});", function.0));
        let op = self.temp(Repr::of(callee.ret), format!("rt.leave({result}, {span})?"));
        for (var, copy, repr) in write_backs {
            self.line(format!("cell_set(&{var}, {});", to_value(&copy, repr)));
        }
        op
    }

    fn binary(&mut self, op: BinaryOp, lhs: &ir::Expr, rhs: &ir::Expr, span: Span) -> Op {
        use BinaryOp::*;
        let span = self.span(span);
        let a = self.operand_before(lhs, &[rhs]);
        let b = self.expr(rhs);
        let int = |name: &str| format!("rt.check(ops::{name}({}, {}), {span})?", a.code, b.code);
        let rational = |name: &str| {
            format!(
                "new_rational(rt.check(ops::rational_op(RationalOp::{name}, rational({}), rational({})), {span})?)",
                a.by_ref(),
                b.by_ref()
            )
        };
        let compare = |symbol: &str| format!("{} {symbol} {}", a.code, b.code);
        let rational_compare = |symbol: &str| {
            format!(
                "(ops::rational_compare(rational({}), rational({})) as i64) {symbol} 0",
                a.by_ref(),
                b.by_ref()
            )
        };
        let (repr, code) = match op {
            AddInt => (Repr::Int, int("int_add")),
            SubInt => (Repr::Int, int("int_sub")),
            MulInt => (Repr::Int, int("int_mul")),
            DivInt => (Repr::Int, int("int_div")),
            ModInt => (Repr::Int, int("int_mod")),
            PowInt => (Repr::Int, int("int_pow")),
            AddFloat => (Repr::Float, compare("+")),
            SubFloat => (Repr::Float, compare("-")),
            MulFloat => (Repr::Float, compare("*")),
            DivFloat => (Repr::Float, compare("/")),
            PowFloat => (Repr::Float, format!("ops::float_pow({}, {})", a.code, b.code)),
            Over => (
                Repr::Value,
                format!("new_rational(rt.check(ops::rational({}, {}), {span})?)", a.code, b.code),
            ),
            AddRational => (Repr::Value, rational("Add")),
            SubRational => (Repr::Value, rational("Subtract")),
            MulRational => (Repr::Value, rational("Multiply")),
            DivRational => (Repr::Value, rational("Divide")),
            PowRational => (
                Repr::Value,
                format!(
                    "new_rational(rt.check(ops::rational_pow(rational({}), {}), {span})?)",
                    a.by_ref(),
                    b.code
                ),
            ),
            EqRational => (Repr::Bool, rational_compare("==")),
            NeRational => (Repr::Bool, rational_compare("!=")),
            LtRational => (Repr::Bool, rational_compare("<")),
            LeRational => (Repr::Bool, rational_compare("<=")),
            GtRational => (Repr::Bool, rational_compare(">")),
            GeRational => (Repr::Bool, rational_compare(">=")),
            EqInt | EqFloat | EqBool => (Repr::Bool, compare("==")),
            NeInt | NeFloat | NeBool => (Repr::Bool, compare("!=")),
            LtInt | LtFloat => (Repr::Bool, compare("<")),
            LeInt | LeFloat => (Repr::Bool, compare("<=")),
            GtInt | GtFloat => (Repr::Bool, compare(">")),
            GeInt | GeFloat => (Repr::Bool, compare(">=")),
            EqText => (Repr::Bool, format!("text({}) == text({})", a.by_ref(), b.by_ref())),
            NeText => (Repr::Bool, format!("text({}) != text({})", a.by_ref(), b.by_ref())),
            InRange => {
                let bounds = self.fresh("t");
                self.line(format!("let {bounds} = range({});", b.by_ref()));
                (Repr::Bool, format!("{bounds}[0] <= {} && {} <= {bounds}[1]", a.code, a.code))
            }
            InList => {
                let a = self.coerce(a, Repr::Value);
                (
                    Repr::Bool,
                    format!(
                        "shared::in_list(&mut rt.at({span}), {}, shared::elements({}))?",
                        a.by_ref(),
                        b.by_ref()
                    ),
                )
            }
            EqValue | NeValue => {
                let not = if op == NeValue { "!" } else { "" };
                (
                    Repr::Bool,
                    format!("{not}shared::equal(&mut rt.at({span}), {}, {})?", a.by_ref(), b.by_ref()),
                )
            }
            InSet => {
                let a = self.coerce(a, Repr::Value);
                (
                    Repr::Bool,
                    format!("shared::set_contains(&mut rt.at({span}), set({}), {})?", b.by_ref(), a.by_ref()),
                )
            }
            InMap => {
                let a = self.coerce(a, Repr::Value);
                (
                    Repr::Bool,
                    format!(
                        "shared::map_position(&mut rt.at({span}), map({}), {})?.is_some()",
                        b.by_ref(),
                        a.by_ref()
                    ),
                )
            }
            SetUnion => (
                Repr::Value,
                format!(
                    "new_set(shared::set_union(&mut rt.at({span}), set({}), set({}))?)",
                    a.by_ref(),
                    b.by_ref()
                ),
            ),
            SetInter | SetMinus => (
                Repr::Value,
                format!(
                    "new_set(shared::set_filter(&mut rt.at({span}), set({}), set({}), {})?)",
                    a.by_ref(),
                    b.by_ref(),
                    op == SetInter
                ),
            ),
            Subset => (
                Repr::Bool,
                format!("shared::set_subset(&mut rt.at({span}), set({}), set({}))?", a.by_ref(), b.by_ref()),
            ),
            EqNone | NeNone => unreachable!("translated to a constant"),
        };
        self.temp(repr, code)
    }

    fn convert(&mut self, conversion: Conversion, value: &ir::Expr, span: Span) -> Op {
        let span = self.span(span);
        let a = self.expr(value);
        match conversion {
            Conversion::IntToFloat => self.temp(Repr::Float, format!("ops::int_to_float({}).0", a.code)),
            Conversion::FloatToInt => {
                self.temp(Repr::Int, format!("rt.check(ops::float_to_int({}), {span})?", a.code))
            }
            Conversion::IntToRational => self.temp(Repr::Value, format!("new_rational([{}, 1])", a.code)),
            Conversion::RationalToFloat => {
                self.temp(Repr::Float, format!("ops::rational_to_float(rational({}))", a.by_ref()))
            }
            Conversion::ToText | Conversion::Literal => {
                let code = match a.repr {
                    Repr::Int | Repr::Bool => format!("new_text({}.to_string())", a.code),
                    Repr::Float => format!("new_text(format_float({}))", a.code),
                    Repr::Value if conversion == Conversion::Literal => {
                        format!("new_text({}.literal())", a.code)
                    }
                    Repr::Value => format!("new_text({}.to_text())", a.code),
                };
                self.temp(Repr::Value, code)
            }
            Conversion::TextToInt => {
                self.temp(Repr::Value, format!("shared::text_to_int(text({}))", a.by_ref()))
            }
            Conversion::TextToFloat => {
                self.temp(Repr::Value, format!("shared::text_to_float(text({}))", a.by_ref()))
            }
            Conversion::EnumPosition => self.temp(Repr::Int, format!("enum_position({})", a.by_ref())),
        }
    }

    fn builtin(&mut self, builtin: Builtin, args: &[ir::Expr], expr: &ir::Expr) -> Op {
        let span = self.span(expr.span);
        match builtin {
            Builtin::Show => {
                let a = self.expr(&args[0]);
                let a = self.coerce(a, Repr::Value);
                self.line(format!("rt.show({})?;", a.by_ref()));
                Op::none()
            }
            Builtin::Sum => {
                let a = self.expr(&args[0]);
                match expr.ty {
                    Type::Float => self.temp(Repr::Float, format!("shared::sum_float({}).0", a.by_ref())),
                    Type::Rational => self.temp(
                        Repr::Value,
                        format!("new_rational(rt.check(shared::sum_rational({}), {span})?)", a.by_ref()),
                    ),
                    _ => self.temp(Repr::Int, format!("rt.check(shared::sum_int({}), {span})?", a.by_ref())),
                }
            }
            Builtin::Error => {
                let a = self.expr(&args[0]);
                self.temp(Repr::Value, format!("new_error({})", a.by_ref()))
            }
            Builtin::Message => {
                let a = self.expr(&args[0]);
                self.temp(Repr::Value, format!("shared::error_message({})", a.by_ref()))
            }
            Builtin::Broken => {
                let name = self.operand_before(&args[0], &[&args[1]]);
                let detail = self.expr(&args[1]);
                self.line(format!(
                    "return Err(rt.bug(broken({}, {}), {span}));",
                    name.by_ref(),
                    detail.by_ref()
                ));
                Op::none()
            }
            Builtin::Ask => {
                let a = self.expr(&args[0]);
                self.temp(Repr::Value, format!("rt.ask({})?", a.by_ref()))
            }
            Builtin::ExpectFailed => {
                let a = self.expr(&args[0]);
                self.line(format!("rt.expect_failed({}, {span});", a.by_ref()));
                Op::none()
            }
            Builtin::Fail => {
                let a = self.expr(&args[0]);
                self.line(format!("return Err(fail({}, {span}));", a.by_ref()));
                Op::none()
            }
            Builtin::Exit => {
                let a = self.scalar(&args[0], Repr::Int);
                self.line(format!("return Err(exit(rt.check(shared::exit_code({a}), {span})?));"));
                Op::none()
            }
            Builtin::Reverse => {
                let a = self.expr(&args[0]);
                self.temp(Repr::Value, format!("shared::reverse({})", a.by_ref()))
            }
            Builtin::Floor | Builtin::Ceil | Builtin::Round => {
                let a = self.scalar(&args[0], Repr::Float);
                let name = match builtin {
                    Builtin::Floor => "float_floor",
                    Builtin::Ceil => "float_ceil",
                    _ => "float_round",
                };
                self.temp(Repr::Int, format!("rt.check(ops::{name}({a}), {span})?"))
            }
            Builtin::Isqrt => {
                let a = self.scalar(&args[0], Repr::Int);
                self.temp(Repr::Int, format!("rt.check(ops::isqrt({a}), {span})?"))
            }
            Builtin::MapGet => {
                let m = self.operand_before(&args[0], &[&args[1]]);
                let k = self.expr(&args[1]);
                let k = self.coerce(k, Repr::Value);
                self.temp(
                    Repr::Value,
                    format!("shared::map_get(&mut rt.at({span}), map({}), {})?", m.by_ref(), k.by_ref()),
                )
            }
            Builtin::Foreign { index, .. } => {
                let values = self.values(&args.iter().collect::<Vec<_>>());
                let result = self.fresh("t");
                self.line(format!("let {result} = rt.call_foreign({index}, &[{}]);", values.join(", ")));
                self.temp(Repr::Value, format!("rt.check({result}, {span})?"))
            }
            Builtin::Native(native) => {
                let values = self.values(&args.iter().collect::<Vec<_>>());
                self.temp(
                    Repr::Value,
                    format!("rt.check(natives::call(Native::{native:?}, &[{}]), {span})?", values.join(", ")),
                )
            }
        }
    }

    /// The test that a value of type `whole` is of type `part` (§7.1), as the virtual
    /// machine makes it.
    fn type_test(&mut self, whole: Type, part: Type, value: &str) -> String {
        let kinds = kinds_of(part);
        let (tested, all) = (self.g.named_types(part), self.g.named_types(whole));
        let needed = |kind: u16| {
            kinds & kind != 0
                && tested.iter().filter(|(k, _)| *k == kind).count()
                    < all.iter().filter(|(k, _)| *k == kind).count()
        };
        if !needed(kinds::STRUCT) && !needed(kinds::ENUM) {
            return format!("(Value::kind({value}) & {kinds}) != 0");
        }
        let mut numbers: Vec<u32> = tested.iter().map(|(_, number)| *number).collect();
        numbers.sort_unstable();
        let numbers: Vec<String> = numbers.iter().map(u32::to_string).collect();
        format!(
            "shared::type_test_named({value}, {}, &[{}])",
            kinds & !(kinds::STRUCT | kinds::ENUM),
            numbers.join(", ")
        )
    }
}

/// The kinds of values of a type, as the bits of `lion_vm::kinds`.
fn kinds_of(ty: Type) -> u16 {
    ty.members()
        .into_iter()
        .map(|member| match member {
            Type::Int => kinds::INT,
            Type::Float => kinds::FLOAT,
            Type::Rational => kinds::RATIONAL,
            Type::Bool => kinds::BOOL,
            Type::Text => kinds::TEXT,
            Type::None => kinds::NONE,
            Type::Range => kinds::RANGE,
            Type::List(_) => kinds::LIST,
            Type::Set(_) => kinds::SET,
            Type::Map(_) => kinds::MAP,
            Type::Task(_) => kinds::TASK,
            Type::Tuple(_) => kinds::TUPLE,
            Type::Error => kinds::ERROR,
            Type::Struct(_) => kinds::STRUCT,
            Type::Enum(_) => kinds::ENUM,
            // A Domain is the function that tells whether a value belongs to it (C74).
            Type::Fun(_) | Type::Domain(_) => kinds::FUN,
            // A trait without members has no values.
            Type::Trait(_) => 0,
            Type::Var(_) => unreachable!("an instance has no type variables"),
            Type::Union(_) => unreachable!("the members of a union are not unions"),
        })
        .fold(0, |all, kind| all | kind)
}

/// Whether evaluating `expr` may call a function of the program, which may change a
/// variable.
fn calls_function(expr: &ir::Expr) -> bool {
    match &expr.kind {
        ExprKind::Call { .. } | ExprKind::CallValue { .. } | ExprKind::Block { .. } => true,
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Text(_)
        | ExprKind::None
        | ExprKind::Local(_)
        | ExprKind::Global(_)
        | ExprKind::Enum { .. }
        | ExprKind::Cell(_) => false,
        ExprKind::Let { value, body, .. } => calls_function(value) || calls_function(body),
        ExprKind::Unary { operand, .. } => calls_function(operand),
        ExprKind::Convert { value, .. }
        | ExprKind::TypeTest { value, .. }
        | ExprKind::Try(value)
        | ExprKind::Task(value)
        | ExprKind::Wait(value)
        | ExprKind::Compile(value) => calls_function(value),
        ExprKind::Binary { lhs, rhs, .. } | ExprKind::And { lhs, rhs } | ExprKind::Or { lhs, rhs } => {
            calls_function(lhs) || calls_function(rhs)
        }
        ExprKind::If { cond, then, otherwise } => {
            calls_function(cond) || calls_function(then) || calls_function(otherwise)
        }
        ExprKind::Range { start, end } => calls_function(start) || calls_function(end),
        ExprKind::Map(entries) => {
            entries.iter().any(|(key, value)| calls_function(key) || calls_function(value))
        }
        ExprKind::List(elements)
        | ExprKind::Set(elements)
        | ExprKind::Tuple(elements)
        | ExprKind::Concat(elements)
        | ExprKind::CallBuiltin { args: elements, .. }
        | ExprKind::Struct { fields: elements, .. }
        | ExprKind::Closure { captures: elements, .. } => elements.iter().any(calls_function),
        ExprKind::Index { object, index } => calls_function(object) || calls_function(index),
        ExprKind::Slice { object, range } => calls_function(object) || calls_function(range),
        ExprKind::Property { object, .. } | ExprKind::Field { object, .. } => calls_function(object),
        ExprKind::Partial { callee, args } => calls_function(callee) || args.iter().any(calls_function),
    }
}

#[cfg(test)]
mod tests;
