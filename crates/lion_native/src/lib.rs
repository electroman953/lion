//! The runtime of the programs that `lion build` compiles to native code (spec §22).
//!
//! `lion_codegen` translates the typed IR into Rust code, which calls this crate. The
//! values are those of the virtual machine, and the operations on them come from
//! `lion_vm::shared` and `lion_runtime`, as in the interpreted mode: both modes give the
//! same results and the same bugs (§22.2). Only the alerts are missing: the interpreted
//! mode alone reports them (§22.3).
//!
//! A compiled program runs on a thread with a large stack, since each call of the
//! program is a call of the machine; it stops with the same exit codes as `lion run`.

use std::cell::RefCell;
use std::ffi::c_void;
use std::io::{self, BufWriter, Write};
use std::panic;
use std::process::ExitCode;
use std::rc::Rc;

use lion_diagnostics::{SourceMap, render};

pub use lion_diagnostics::{SourceId, Span};
pub use lion_ir::{ForeignFunction, Native, Type};
pub use lion_runtime::format::format_float;
pub use lion_runtime::ops::{self, RationalOp};
pub use lion_runtime::{BugKind, MAX_CALL_DEPTH};
pub use lion_vm::natives;
pub use lion_vm::shared::{self, Comparer, Stop};
pub use lion_vm::{Closure, EnumLayout, Layout, MapValue, Record, SetValue, Trap, Value, kinds};

/// A trap, boxed so that the results stay small on the path where nothing fails.
pub type Fault = Box<Trap>;
pub type R<T> = Result<T, Fault>;

/// A function of the program called through a value (§11): it receives the arguments
/// given, including those of a partial application (§11.3), then the values it captured
/// (§11.5).
pub type Dynamic = fn(&mut Rt, Vec<Value>, &[Value]) -> R<Value>;

/// What the runtime needs to know of a compiled program.
pub struct Program {
    /// The source files, in the order of their `SourceId`s, to show where a bug happens.
    pub sources: &'static [(&'static str, &'static str)],
    pub structs: &'static [Structure],
    pub enums: &'static [Enumeration],
    /// The texts written in the program.
    pub texts: &'static [&'static str],
    /// The name of each function, shown by a function value (C65).
    pub names: &'static [&'static str],
    /// The functions that may be called through a value, by their number.
    pub dynamic: &'static [Option<Dynamic>],
    pub foreign: &'static [Foreign],
    /// The number of files, each with its globals to initialize (§20.2).
    pub modules: usize,
    /// The statements of the script (§20.1).
    pub script: fn(&mut Rt) -> R<Value>,
}

/// A structure (§12): its name, the names of its fields, and its `equals` (§12.5).
pub struct Structure {
    pub name: &'static str,
    pub fields: &'static [&'static str],
    pub equals: Option<u32>,
}

/// An enumeration (§13.1): its name and the names of its values.
pub struct Enumeration {
    pub name: &'static str,
    pub values: &'static [&'static str],
}

/// A C function of the program (§21.2).
pub struct Foreign {
    pub library: &'static str,
    pub name: &'static str,
    pub params: &'static [CType],
    pub ret: CType,
}

/// The types that a C function takes and gives (C80).
#[derive(Clone, Copy)]
pub enum CType {
    Int,
    Float,
    Bool,
    Text,
    MaybeText,
    None,
}

impl CType {
    fn to_type(self) -> Type {
        match self {
            CType::Int => Type::Int,
            CType::Float => Type::Float,
            CType::Bool => Type::Bool,
            CType::Text => Type::Text,
            CType::MaybeText => Type::maybe(Type::Text),
            CType::None => Type::None,
        }
    }
}

/// The span of the source, as the code refers to it: a place of the program.
pub const fn span(source: u32, start: u32, end: u32) -> Option<Span> {
    Some(Span { source: SourceId::from_index(source), start, end })
}

/// The sizes of stack tried for the program, largest first: enough for the calls that
/// Lion allows (`MAX_CALL_DEPTH`, C13). Only the part that the calls use is given memory.
const STACK_SIZES: &[usize] = &[4 << 30, 1 << 30, 256 << 20];

/// Runs the program; the exit code is that of `lion run` (§22.2).
pub fn start(program: &'static Program) -> ExitCode {
    panic::set_hook(Box::new(|info| {
        let location = info.location().map(|l| format!(" at {}:{}", l.file(), l.line())).unwrap_or_default();
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_default();
        eprintln!("internal error: {payload}");
        eprintln!("  = note: raised{location}");
        eprintln!(
            "  = note: this is a defect in the Lion implementation, not in your program; please report it"
        );
    }));
    let mut problem = None;
    for &size in STACK_SIZES {
        match std::thread::Builder::new().stack_size(size).spawn(move || run(program)) {
            Ok(thread) => return ExitCode::from(thread.join().unwrap_or(INTERNAL)),
            Err(error) => problem = Some(error),
        }
    }
    eprintln!("error: cannot start the program: {}", problem.expect("a size was tried"));
    ExitCode::from(INTERNAL)
}

/// The exit code of a defect of the implementation.
const INTERNAL: u8 = 70;

fn run(program: &'static Program) -> u8 {
    let mut rt = Rt::new(program);
    let result = (program.script)(&mut rt);
    // Everything the program wrote appears before the report of a bug.
    let _ = rt.out.flush();
    let trap = match result {
        Ok(_) => return 0,
        Err(trap) => *trap,
    };
    if let Trap::Exit(code) = trap {
        return code;
    }
    let mut sources = SourceMap::new();
    for (name, text) in program.sources {
        sources.add(*name, *text);
    }
    // A bug in the standard library is shown at the call that led there (C64).
    let hidden = |span: Span| sources.get(span.source).name().starts_with("<std>/");
    let trap = trap.located(&hidden);
    eprintln!("{}", render(&trap.to_diagnostic(), &sources));
    match trap {
        Trap::Bug { .. } => 2,
        Trap::Io(_) | Trap::Failure { .. } => 1,
        Trap::Exit(code) => code,
    }
}

/// The state of a running program.
pub struct Rt {
    out: BufWriter<io::Stdout>,
    input: io::StdinLock<'static>,
    /// The calls in progress, the script included (C13).
    depth: usize,
    /// For each file, whether its globals have their values, or are getting them (D81).
    initialized: Vec<bool>,
    layouts: Vec<Rc<Layout>>,
    enums: Vec<Rc<EnumLayout>>,
    texts: Vec<Rc<String>>,
    names: Vec<Rc<str>>,
    dynamic: &'static [Option<Dynamic>],
    custom_equality: bool,
    foreign: Vec<ForeignFunction>,
    /// The C functions found so far (§21.2).
    found: Vec<Option<*mut c_void>>,
    /// The failed `expect`s of a test (§24.1).
    pub failures: Vec<(Option<Span>, String)>,
}

impl Rt {
    fn new(program: &'static Program) -> Rt {
        let layouts: Vec<Rc<Layout>> = program
            .structs
            .iter()
            .enumerate()
            .map(|(index, structure)| {
                Rc::new(Layout {
                    index: index as u32,
                    name: structure.name.to_string(),
                    fields: structure.fields.iter().map(|field| field.to_string()).collect(),
                    equals: structure.equals,
                })
            })
            .collect();
        // The type numbers of the enumerations follow those of the structures.
        let enums = program
            .enums
            .iter()
            .enumerate()
            .map(|(index, enumeration)| {
                Rc::new(EnumLayout {
                    index: (program.structs.len() + index) as u32,
                    name: enumeration.name.to_string(),
                    values: enumeration.values.iter().map(|value| value.to_string()).collect(),
                })
            })
            .collect();
        let foreign = program
            .foreign
            .iter()
            .map(|function| ForeignFunction {
                library: function.library.to_string(),
                name: function.name.to_string(),
                params: function.params.iter().map(|param| param.to_type()).collect(),
                ret: function.ret.to_type(),
            })
            .collect();
        Rt {
            out: BufWriter::new(io::stdout()),
            input: io::stdin().lock(),
            depth: 1,
            initialized: vec![false; program.modules],
            custom_equality: layouts.iter().any(|layout| layout.equals.is_some()),
            layouts,
            enums,
            texts: program.texts.iter().map(|text| Rc::new(text.to_string())).collect(),
            names: program.names.iter().map(|&name| Rc::from(name)).collect(),
            dynamic: program.dynamic,
            foreign,
            found: vec![None; program.foreign.len()],
            failures: Vec::new(),
        }
    }

    /// Before a call: the call made at `span` would be one too many (C13).
    #[inline]
    pub fn enter(&mut self, span: Option<Span>) -> R<()> {
        if self.depth >= MAX_CALL_DEPTH {
            return Err(self.bug(BugKind::StackOverflow, span));
        }
        self.depth += 1;
        Ok(())
    }

    /// After a call made at `span`: a bug met inside records the call (§18.4).
    #[inline]
    pub fn leave<T>(&mut self, result: R<T>, span: Option<Span>) -> R<T> {
        self.depth -= 1;
        match result {
            Ok(value) => Ok(value),
            Err(fault) => Err(called_from(fault, span)),
        }
    }

    #[cold]
    pub fn bug(&self, kind: BugKind, span: Option<Span>) -> Fault {
        Box::new(Trap::Bug { kind, span, calls: Vec::new() })
    }

    /// The result of an operation, or the bug it met at `span`.
    #[inline]
    pub fn check<T>(&self, result: Result<T, BugKind>, span: Option<Span>) -> R<T> {
        match result {
            Ok(value) => Ok(value),
            Err(kind) => Err(self.bug(kind, span)),
        }
    }

    /// The result of a shared operation, or why it stopped at `span`.
    #[inline]
    pub fn stop<T>(&self, result: Result<T, Stop>, span: Option<Span>) -> R<T> {
        match result {
            Ok(value) => Ok(value),
            Err(Stop::Bug(kind)) => Err(self.bug(kind, span)),
            Err(Stop::Fault(fault)) => Err(fault),
        }
    }

    /// The comparisons of values made at `span`, which may call `equals` (§12.5).
    #[inline]
    pub fn at(&mut self, span: Option<Span>) -> At<'_> {
        At { rt: self, span }
    }

    /// `show(value)` (§23).
    pub fn show(&mut self, value: &Value) -> R<()> {
        writeln!(self.out, "{}", value.to_text()).map_err(|error| Box::new(Trap::Io(error)))
    }

    /// `ask(prompt)` (§23, C59).
    pub fn ask(&mut self, prompt: &Value) -> R<Value> {
        match shared::ask(&mut self.out, &mut self.input, text(prompt)) {
            Ok(line) => Ok(Value::Text(Rc::new(line))),
            Err(error) => Err(Box::new(Trap::Io(error))),
        }
    }

    /// A failed `expect` of a test (§24.1).
    pub fn expect_failed(&mut self, message: &Value, span: Option<Span>) {
        self.failures.push((span, text(message).to_string()));
    }

    /// The text `index` of the program.
    #[inline]
    pub fn text(&self, index: usize) -> Value {
        Value::Text(Rc::clone(&self.texts[index]))
    }

    /// A value of the structure `layout`, from the values of its fields (§12.2).
    pub fn record(&self, layout: usize, fields: Vec<Value>) -> Value {
        Value::Struct(Rc::new(Record { layout: Rc::clone(&self.layouts[layout]), fields }))
    }

    /// The value at `position` of the enumeration `enumeration` (§13.1).
    pub fn enumeration(&self, enumeration: usize, position: u32) -> Value {
        Value::Enum(Rc::clone(&self.enums[enumeration]), position)
    }

    /// The function `function` as a value, with the values it captures (§11.5).
    pub fn closure(&self, function: u32, captures: Vec<Value>) -> Value {
        let name = Rc::clone(&self.names[function as usize]);
        Value::Function(Rc::new(Closure { function, name, bound: Vec::new(), captures }))
    }

    /// A call of a function value, made at `span` (§11).
    pub fn call_value(&mut self, callee: &Value, args: Vec<Value>, span: Option<Span>) -> R<Value> {
        let closure = match callee {
            Value::Function(closure) => Rc::clone(closure),
            other => mismatch("function", other),
        };
        // The arguments given before come first (§11.3).
        let args = if closure.bound.is_empty() {
            args
        } else {
            let mut all = closure.bound.clone();
            all.extend(args);
            all
        };
        self.call_dynamic(closure.function, args, &closure.captures, span)
    }

    /// A call of the function `function` of the program with values, made at `span`.
    pub fn call_dynamic(
        &mut self,
        function: u32,
        args: Vec<Value>,
        captures: &[Value],
        span: Option<Span>,
    ) -> R<Value> {
        let Some(dynamic) = self.dynamic[function as usize] else {
            panic!("the function {function} is not called through a value")
        };
        self.enter(span)?;
        let result = dynamic(self, args, captures);
        self.leave(result, span)
    }

    /// Whether the globals of the file `module` still need their values; from now on,
    /// they are getting them (D81).
    #[inline]
    pub fn begin_module(&mut self, module: usize) -> bool {
        !std::mem::replace(&mut self.initialized[module], true)
    }

    /// A call of the C function `index` (§21.2).
    pub fn call_foreign(&mut self, index: usize, args: &[Value]) -> Result<Value, BugKind> {
        shared::call_foreign(&mut self.found[index], &self.foreign[index], args)
    }
}

/// The program as a [`Comparer`], for an operation made at `span`: it calls `equals`
/// itself (§12.5).
pub struct At<'r> {
    rt: &'r mut Rt,
    span: Option<Span>,
}

impl Comparer for At<'_> {
    fn custom_equality(&self) -> bool {
        self.rt.custom_equality
    }

    fn call_equals(&mut self, function: u32, a: Value, b: Value) -> Result<Value, Fault> {
        self.rt.call_dynamic(function, vec![a, b], &[], self.span)
    }
}

/// A bug met in a call records the place of the call, innermost first (§18.4).
#[cold]
pub fn called_from(mut fault: Fault, span: Option<Span>) -> Fault {
    if let (Trap::Bug { calls, .. }, Some(span)) = (&mut *fault, span) {
        calls.push(span);
    }
    fault
}

/// `try` in the script met an Error: the script stops with its message (§18.3, C39).
#[cold]
pub fn failure(value: &Value, span: Option<Span>) -> Fault {
    Box::new(Trap::Failure { message: shared::failure_message(value), span })
}

/// Stops the script with the Error whose message is given (§18.3).
#[cold]
pub fn fail(message: &Value, span: Option<Span>) -> Fault {
    Box::new(Trap::Failure { message: text(message).to_string(), span })
}

/// `exit(code)` (§20.1).
#[cold]
pub fn exit(code: u8) -> Fault {
    Box::new(Trap::Exit(code))
}

/// The structure named by the first value no longer satisfies its invariants (§12.3).
#[cold]
pub fn broken(name: &Value, detail: &Value) -> BugKind {
    BugKind::BrokenInvariant { structure: text(name).to_string(), detail: text(detail).to_string() }
}

// A value of the wrong kind is a defect of the implementation, not of the program: it
// panics, and the program reports an internal error.

#[cold]
fn mismatch(expected: &str, found: &Value) -> ! {
    panic!("the compiled program expected {expected} but found {}", found.type_name())
}

#[inline]
pub fn int(value: &Value) -> i64 {
    match value {
        Value::Int(value) => *value,
        other => mismatch("Int", other),
    }
}

#[inline]
pub fn float(value: &Value) -> f64 {
    match value {
        Value::Float(value) => *value,
        other => mismatch("Float", other),
    }
}

#[inline]
pub fn boolean(value: &Value) -> bool {
    match value {
        Value::Bool(value) => *value,
        other => mismatch("Bool", other),
    }
}

#[inline]
pub fn text(value: &Value) -> &str {
    match value {
        Value::Text(text) => text,
        other => mismatch("Text", other),
    }
}

#[inline]
pub fn rational(value: &Value) -> [i64; 2] {
    match value {
        Value::Rational(value) => **value,
        other => mismatch("Rational", other),
    }
}

#[inline]
pub fn range(value: &Value) -> [i64; 2] {
    match value {
        Value::Range(bounds) => **bounds,
        other => mismatch("Range", other),
    }
}

#[inline]
pub fn set(value: &Value) -> &SetValue {
    match value {
        Value::Set(set) => set,
        other => mismatch("Set", other),
    }
}

#[inline]
pub fn map(value: &Value) -> &MapValue {
    match value {
        Value::Map(map) => map,
        other => mismatch("Map", other),
    }
}

/// The field at `position`, from 0, of a value of a structure.
#[inline]
pub fn field(value: &Value, position: usize) -> Value {
    match value {
        Value::Struct(record) => record.fields[position].clone(),
        other => mismatch("structure", other),
    }
}

/// The position of a value of an enumeration, from 0 (D33).
pub fn enum_position(value: &Value) -> i64 {
    match value {
        Value::Enum(_, position) => i64::from(*position),
        other => mismatch("enumeration", other),
    }
}

pub fn new_text(text: String) -> Value {
    Value::Text(Rc::new(text))
}

pub fn new_rational(value: [i64; 2]) -> Value {
    Value::Rational(Rc::new(value))
}

pub fn new_list(elements: Vec<Value>) -> Value {
    Value::List(Rc::new(elements))
}

pub fn new_tuple(elements: Vec<Value>) -> Value {
    Value::Tuple(Rc::new(elements))
}

pub fn new_range(start: i64, end: i64) -> Value {
    Value::Range(Rc::new([start, end]))
}

pub fn new_set(set: SetValue) -> Value {
    Value::Set(Rc::new(set))
}

pub fn new_map(map: MapValue) -> Value {
    Value::Map(Rc::new(map))
}

/// `error(message)` (§18.2, D65).
pub fn new_error(message: &Value) -> Value {
    Value::Error(Rc::new(text(message).to_string()))
}

/// Joins texts, in order (interpolation, §4.5).
pub fn concat(parts: &[&Value]) -> Value {
    let mut joined = String::new();
    for part in parts {
        joined.push_str(text(part));
    }
    new_text(joined)
}

/// `task value`: this version computes it at once (C71).
pub fn new_task(result: Value) -> Value {
    Value::Task(Rc::new(result))
}

/// `wait t` (§19.1).
pub fn wait(task: &Value) -> Value {
    match task {
        Value::Task(result) => (**result).clone(),
        other => mismatch("task", other),
    }
}

/// `f(1)` with fewer arguments than `f` requires (§11.3).
pub fn bind(callee: &Value, args: Vec<Value>) -> Value {
    let closure = match callee {
        Value::Function(closure) => closure,
        other => mismatch("function", other),
    };
    let mut bound = closure.bound.clone();
    bound.extend(args);
    let name = Rc::clone(&closure.name);
    let captures = closure.captures.clone();
    Value::Function(Rc::new(Closure { function: closure.function, name, bound, captures }))
}

/// A new variable shared by a function and the code around it (§11.5).
pub fn new_cell() -> Value {
    Value::Cell(Rc::new(RefCell::new(Value::None)))
}

fn cell(value: &Value) -> &RefCell<Value> {
    match value {
        Value::Cell(cell) => cell,
        other => mismatch("cell", other),
    }
}

pub fn cell_get(value: &Value) -> Value {
    cell(value).borrow().clone()
}

pub fn cell_set(value: &Value, content: Value) {
    *cell(value).borrow_mut() = content;
}

/// Moves the value out of the cell, to change it in place and store it back.
pub fn cell_take(value: &Value) -> Value {
    std::mem::take(&mut *cell(value).borrow_mut())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bug_records_the_calls_it_goes_through_innermost_first() {
        let bug = Box::new(Trap::Bug { kind: BugKind::EmptyList, span: span(0, 1, 2), calls: Vec::new() });
        let bug = called_from(bug, span(0, 10, 12));
        // A call without a place in the source, as the one that initializes a module.
        let bug = called_from(bug, None);
        let bug = called_from(bug, span(1, 3, 4));
        let Trap::Bug { calls, .. } = *bug else { panic!("a bug") };
        assert_eq!(calls, vec![span(0, 10, 12).unwrap(), span(1, 3, 4).unwrap()]);
    }

    #[test]
    fn other_traps_record_no_calls() {
        let exit = called_from(exit(3), span(0, 1, 2));
        assert!(matches!(*exit, Trap::Exit(3)));
    }

    #[test]
    fn values_are_read_as_their_kind() {
        assert_eq!(int(&Value::Int(4)), 4);
        assert_eq!(text(&new_text("é".to_string())), "é");
        assert_eq!(range(&new_range(1, 9)), [1, 9]);
    }

    #[test]
    fn a_function_value_keeps_the_arguments_given_first() {
        let closure = Value::Function(Rc::new(Closure {
            function: 3,
            name: Rc::from("f"),
            bound: vec![Value::Int(1)],
            captures: vec![Value::Bool(true)],
        }));
        let Value::Function(bound) = bind(&closure, vec![Value::Int(2)]) else { panic!("a function") };
        assert_eq!(bound.function, 3);
        assert!(bound.bound[1].equals(&Value::Int(2)) && bound.bound.len() == 2);
        assert_eq!(bound.captures.len(), 1);
    }

    #[test]
    fn a_cell_is_shared() {
        let cell = new_cell();
        let other = cell.clone();
        cell_set(&cell, Value::Int(5));
        assert_eq!(int(&cell_get(&other)), 5);
        assert_eq!(int(&cell_take(&other)), 5);
        assert!(matches!(cell_get(&cell), Value::None));
    }
}
