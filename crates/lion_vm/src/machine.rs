//! Execution of bytecode.

use std::cell::RefCell;
use std::collections::HashSet;
use std::io::{self, BufRead, Write};
use std::rc::Rc;

use lion_diagnostics::{Diagnostic, Span};
use lion_runtime::format::format_float;
use lion_runtime::ops::RationalOp;
use lion_runtime::{BugKind, MAX_CALL_DEPTH, ops};

use crate::bytecode::{Chunk, Instr, Program, Reg, Target};
use crate::set::{SetValue, holds_nan};
use crate::value::{Closure, Record, Value};

/// Why execution stopped before the end of the program.
#[derive(Debug)]
pub enum Trap {
    /// A bug in the program (spec §18.1). `calls` are the calls in progress, innermost first.
    Bug { kind: BugKind, span: Option<Span>, calls: Vec<Span> },
    /// `try` in the script met an Error: the script stops with its message (§18.3, D80).
    Failure { message: String, span: Option<Span> },
    /// Writing the output failed.
    Io(io::Error),
    /// `exit(code)` stopped the program (§20.1).
    Exit(u8),
}

impl Trap {
    /// The same trap, placed in the code of the program rather than in hidden code,
    /// such as the standard library: at the innermost call made from visible code.
    pub fn located(self, hidden: &dyn Fn(Span) -> bool) -> Trap {
        match self {
            Trap::Bug { kind, span, calls } => {
                let (span, calls) = relocate(span, calls, hidden);
                Trap::Bug { kind, span, calls }
            }
            other => other,
        }
    }

    pub fn to_diagnostic(&self) -> Diagnostic {
        match self {
            Trap::Bug { kind, span, calls } => {
                let mut diagnostic = Diagnostic::bug(kind.message());
                if let Some(span) = span {
                    diagnostic = diagnostic.with_primary(*span, "");
                }
                // The calls that led here, without repeating a place (as in a recursion).
                let mut shown: Vec<Span> = Vec::new();
                for call in calls {
                    if shown.len() < 3 && !shown.contains(call) && Some(*call) != *span {
                        shown.push(*call);
                    }
                }
                for call in &shown {
                    diagnostic = diagnostic.with_secondary(*call, "in the call made here");
                }
                if calls.len() > 1 && *kind != BugKind::StackOverflow {
                    diagnostic = diagnostic.with_note(format!("{} calls were in progress", calls.len()));
                }
                diagnostic.with_note(kind.details()).with_help(kind.help())
            }
            Trap::Io(error) => Diagnostic::error(format!("cannot write the output: {error}")),
            Trap::Exit(code) => Diagnostic::internal(format!("the program stopped with `exit({code})`")),
            Trap::Failure { message, span } => {
                let mut diagnostic = Diagnostic::error(message.clone());
                if let Some(span) = span {
                    diagnostic = diagnostic.with_primary(*span, "this `try` met the error");
                }
                diagnostic.with_note("the script stops on an Error that it does not handle (§18.3)")
            }
        }
    }
}

/// A non-blocking remark of the interpreted mode (spec §22.3).
#[derive(Clone, Debug)]
pub struct Alert {
    pub kind: AlertKind,
    pub span: Option<Span>,
    /// The calls in progress, innermost first.
    pub calls: Vec<Span>,
}

impl Alert {
    /// The same alert, placed in the code of the program rather than in hidden code,
    /// such as the standard library: at the innermost call made from visible code.
    pub fn located(self, hidden: &dyn Fn(Span) -> bool) -> Alert {
        let (span, calls) = relocate(self.span, self.calls, hidden);
        Alert { span, calls, ..self }
    }
}

/// `span`, or the innermost of `calls` that is not hidden when it is, and the calls
/// outside the hidden code.
fn relocate(
    span: Option<Span>,
    calls: Vec<Span>,
    hidden: &dyn Fn(Span) -> bool,
) -> (Option<Span>, Vec<Span>) {
    if !span.is_some_and(hidden) {
        return (span, calls.into_iter().filter(|call| !hidden(*call)).collect());
    }
    let mut visible = calls.into_iter().filter(|call| !hidden(*call));
    let span = visible.next();
    (span, visible.collect())
}

#[derive(Clone, Copy, Debug)]
pub enum AlertKind {
    /// An Int beyond 2^53 lost precision when converted to Float (§8.5).
    PrecisionLoss { value: i64, converted: f64 },
    /// A Float operation on finite values produced an infinity or NaN (§8.2).
    SpecialFloat { value: f64 },
}

impl Alert {
    pub fn to_diagnostic(&self) -> Diagnostic {
        let mut diagnostic = match self.kind {
            AlertKind::PrecisionLoss { value, converted } => Diagnostic::alert(
                "this Int has no exact Float value",
            )
            .with_note(format!("{value} became {} when converted to Float (§8.5)", format_float(converted))),
            AlertKind::SpecialFloat { value } => {
                Diagnostic::alert(format!("this operation produced {}", format_float(value)))
                    .with_note("Float follows IEEE 754 (§8.2)")
            }
        };
        if let Some(span) = self.span {
            diagnostic = diagnostic.with_primary(span, "");
        }
        diagnostic.with_note("alerts are reported only in interpreted mode, once per location (§22.3)")
    }
}

/// Runs a program to its end, writing the output of `show` to `out`, reading the lines
/// of `ask` from `input`, and reporting alerts to `on_alert` as they happen; `out` is
/// flushed before each alert.
pub fn run(
    program: &Program,
    out: &mut dyn Write,
    input: &mut dyn BufRead,
    on_alert: &mut dyn FnMut(Alert),
) -> Result<(), Trap> {
    let mut machine = Machine::new(program, out, input, on_alert);
    machine.run().map_err(|fault| *fault)
}

/// What the interactive mode keeps from one input to the next: the registers of the
/// script, which hold its variables, and the modules already initialized (§24, D26).
#[derive(Default)]
pub struct Session {
    stack: Vec<Value>,
    initialized: Vec<bool>,
}

/// Runs the statements of the script from the one at `first`, on the variables of the
/// session. On a trap, the session keeps the values it had reached.
pub fn run_from(
    program: &Program,
    session: &mut Session,
    first: usize,
    out: &mut dyn Write,
    input: &mut dyn BufRead,
    on_alert: &mut dyn FnMut(Alert),
) -> Result<(), Trap> {
    let mut machine = Machine::new(program, out, input, on_alert);
    let stack = std::mem::take(&mut session.stack);
    let size = machine.stack.len().max(stack.len());
    machine.stack = stack;
    machine.stack.resize(size, Value::None);
    for (index, done) in session.initialized.iter().enumerate() {
        if let Some(flag) = machine.initialized.get_mut(index) {
            *flag = *done;
        }
    }
    let main = machine.chunk;
    let start = main.starts.get(first).map_or(main.code.len() - 1, |&start| start as usize);
    let result = machine.run_at(start);
    // The registers of the script are kept; those of calls that a bug stopped are not.
    machine.stack.truncate(main.registers as usize);
    session.stack = std::mem::take(&mut machine.stack);
    session.initialized = machine.initialized;
    result.map_err(|fault| *fault)
}

/// Runs a function without arguments on a machine of its own, and gives its result:
/// the value of a `compile` expression (§21.1).
pub(crate) fn evaluate(program: &Program, function: usize) -> Result<Value, Trap> {
    let (mut out, mut input) = (io::sink(), io::empty());
    let mut ignore = |_: Alert| {};
    let mut machine = Machine::new(program, &mut out, &mut input, &mut ignore);
    let halt = machine.chunk.code.len() - 1;
    let registers = machine.chunk.registers;
    machine.call(function, 0, registers, 0, halt, halt).map_err(|fault| *fault)?;
    machine.run().map_err(|fault| *fault)?;
    Ok(std::mem::take(&mut machine.stack[0]))
}

/// A failed `expect` of a test: where, and why (§24.1, D72).
#[derive(Debug)]
pub struct Failure {
    pub span: Option<Span>,
    pub message: String,
}

/// Runs the test `index` of `program.tests`, without the statements of the script: the
/// failed `expect`s, or the trap that stopped it (§24.1).
pub fn run_test(
    program: &Program,
    index: usize,
    out: &mut dyn Write,
    input: &mut dyn BufRead,
    on_alert: &mut dyn FnMut(Alert),
) -> Result<Vec<Failure>, Trap> {
    let mut machine = Machine::new(program, out, input, on_alert);
    // The declarations of the globals of the script run first, in its frame (C68).
    if let Some(declarations) = &program.declarations {
        machine.chunk = declarations;
        machine.run().map_err(|fault| *fault)?;
        machine.chunk = &program.functions[program.main];
    }
    // The test runs above the frame of the script, which then stops at its `Halt`.
    let halt = machine.chunk.code.len() - 1;
    let registers = machine.chunk.registers;
    let function = program.tests[index].1 as usize;
    // The `Halt` has no place in the source: the trace of a bug starts in the test.
    machine.call(function, 0, registers, 0, halt, halt).map_err(|fault| *fault)?;
    machine.run().map_err(|fault| *fault)?;
    Ok(machine.failures)
}

/// A trap, boxed so that the results of the operations stay small on the path where
/// nothing fails.
type Fault = Box<Trap>;

/// A call in progress. The script's frame is at the bottom of the stack; its
/// registers are the globals.
struct Frame {
    function: u32,
    /// Where its registers start in the stack.
    base: u32,
    /// Where the function resumes once the call it made returns.
    resume: u32,
    /// The caller's register that receives the result.
    dst: Reg,
    /// How many arguments the call gave.
    args: u32,
    /// The call instruction in the caller.
    call: u32,
}

struct Machine<'a> {
    program: &'a Program,
    /// The code of the function running now.
    chunk: &'a Chunk,
    /// Where its registers start.
    base: usize,
    stack: Vec<Value>,
    frames: Vec<Frame>,
    out: &'a mut dyn Write,
    input: &'a mut dyn BufRead,
    on_alert: &'a mut dyn FnMut(Alert),
    /// Instructions (function, index) that already reported an alert.
    alerted: HashSet<(usize, usize)>,
    /// For each file, whether its globals have their values, or are getting them (D81).
    initialized: Vec<bool>,
    /// The failed `expect`s of the test that runs (§24.1).
    failures: Vec<Failure>,
    /// A function that the machine calls itself, as `equals`, runs until the number of
    /// frames goes below this.
    stop_depth: usize,
}

impl<'a> Machine<'a> {
    fn new(
        program: &'a Program,
        out: &'a mut dyn Write,
        input: &'a mut dyn BufRead,
        on_alert: &'a mut dyn FnMut(Alert),
    ) -> Machine<'a> {
        let main = &program.functions[program.main];
        Machine {
            program,
            chunk: main,
            base: 0,
            stack: vec![Value::None; main.registers as usize],
            frames: vec![Frame {
                function: program.main as u32,
                base: 0,
                resume: 0,
                dst: 0,
                args: 0,
                call: 0,
            }],
            out,
            input,
            on_alert,
            alerted: HashSet::new(),
            initialized: vec![false; program.module_inits.len()],
            failures: Vec::new(),
            stop_depth: 0,
        }
    }

    fn run(&mut self) -> Result<(), Fault> {
        self.run_at(0)
    }

    fn run_at(&mut self, start: usize) -> Result<(), Fault> {
        let mut pc = start;
        // The code of the running function, reloaded when a call enters or leaves one.
        let mut code: &[Instr] = &self.chunk.code;
        loop {
            let at = pc;
            pc += 1;
            match code[at] {
                Instr::LoadInt { dst, value } => self.set(dst, Value::Int(value)),
                Instr::LoadFloat { dst, value } => self.set(dst, Value::Float(value)),
                Instr::LoadBool { dst, value } => self.set(dst, Value::Bool(value)),
                Instr::LoadNone { dst } => self.set(dst, Value::None),
                Instr::LoadText { dst, index } => {
                    self.set(dst, Value::Text(self.chunk.texts[index as usize].clone()))
                }
                Instr::Move { dst, src } => {
                    let value = self.stack[self.base + src as usize].clone();
                    self.set(dst, value);
                }

                Instr::AddInt { dst, a, b } => self.int_op(dst, a, b, at, ops::int_add)?,
                Instr::SubInt { dst, a, b } => self.int_op(dst, a, b, at, ops::int_sub)?,
                Instr::MulInt { dst, a, b } => self.int_op(dst, a, b, at, ops::int_mul)?,
                Instr::DivInt { dst, a, b } => self.int_op(dst, a, b, at, ops::int_div)?,
                Instr::ModInt { dst, a, b } => self.int_op(dst, a, b, at, ops::int_mod)?,
                Instr::PowInt { dst, a, b } => self.int_op(dst, a, b, at, ops::int_pow)?,
                Instr::NegInt { dst, a } => {
                    let value = ops::int_neg(self.int(a)).map_err(|kind| self.bug(kind, at))?;
                    self.set(dst, Value::Int(value));
                }

                Instr::AddIntImm { dst, a, imm } => self.int_imm(dst, a, imm, at, ops::int_add)?,
                Instr::SubIntImm { dst, a, imm } => self.int_imm(dst, a, imm, at, ops::int_sub)?,
                Instr::MulIntImm { dst, a, imm } => self.int_imm(dst, a, imm, at, ops::int_mul)?,
                Instr::DivIntImm { dst, a, imm } => self.int_imm(dst, a, imm, at, ops::int_div)?,
                Instr::ModIntImm { dst, a, imm } => self.int_imm(dst, a, imm, at, ops::int_mod)?,
                Instr::AddFloat { dst, a, b } => self.float_op(dst, a, b, at, |x, y| x + y)?,
                Instr::SubFloat { dst, a, b } => self.float_op(dst, a, b, at, |x, y| x - y)?,
                Instr::MulFloat { dst, a, b } => self.float_op(dst, a, b, at, |x, y| x * y)?,
                Instr::DivFloat { dst, a, b } => self.float_op(dst, a, b, at, |x, y| x / y)?,
                Instr::PowFloat { dst, a, b } => self.float_op(dst, a, b, at, ops::float_pow)?,
                Instr::NegFloat { dst, a } => {
                    let value = -self.float(a);
                    self.set(dst, Value::Float(value));
                }
                Instr::MakeRational { dst, a, b } => {
                    let value = ops::rational(self.int(a), self.int(b)).map_err(|kind| self.bug(kind, at))?;
                    self.set(dst, Value::Rational(Rc::new(value)));
                }
                Instr::AddRational { dst, a, b } => self.rational_op(dst, a, b, at, RationalOp::Add)?,
                Instr::SubRational { dst, a, b } => self.rational_op(dst, a, b, at, RationalOp::Subtract)?,
                Instr::MulRational { dst, a, b } => self.rational_op(dst, a, b, at, RationalOp::Multiply)?,
                Instr::DivRational { dst, a, b } => self.rational_op(dst, a, b, at, RationalOp::Divide)?,
                Instr::PowRational { dst, a, b } => {
                    let value = ops::rational_pow(self.rational(a), self.int(b))
                        .map_err(|kind| self.bug(kind, at))?;
                    self.set(dst, Value::Rational(Rc::new(value)));
                }
                Instr::NegRational { dst, a } => {
                    let value = ops::rational_op(RationalOp::Subtract, [0, 1], self.rational(a))
                        .map_err(|kind| self.bug(kind, at))?;
                    self.set(dst, Value::Rational(Rc::new(value)));
                }
                Instr::CmpRational { dst, cmp, a, b } => {
                    let ordering = ops::rational_compare(self.rational(a), self.rational(b));
                    self.set(dst, Value::Bool(cmp.holds_for(ordering)));
                }

                Instr::EqInt { dst, a, b } => self.int_test(dst, a, b, |x, y| x == y),
                Instr::NeInt { dst, a, b } => self.int_test(dst, a, b, |x, y| x != y),
                Instr::LtInt { dst, a, b } => self.int_test(dst, a, b, |x, y| x < y),
                Instr::LeInt { dst, a, b } => self.int_test(dst, a, b, |x, y| x <= y),
                Instr::GtInt { dst, a, b } => self.int_test(dst, a, b, |x, y| x > y),
                Instr::GeInt { dst, a, b } => self.int_test(dst, a, b, |x, y| x >= y),
                Instr::EqFloat { dst, a, b } => self.float_test(dst, a, b, |x, y| x == y),
                Instr::NeFloat { dst, a, b } => self.float_test(dst, a, b, |x, y| x != y),
                Instr::LtFloat { dst, a, b } => self.float_test(dst, a, b, |x, y| x < y),
                Instr::LeFloat { dst, a, b } => self.float_test(dst, a, b, |x, y| x <= y),
                Instr::GtFloat { dst, a, b } => self.float_test(dst, a, b, |x, y| x > y),
                Instr::GeFloat { dst, a, b } => self.float_test(dst, a, b, |x, y| x >= y),
                Instr::EqBool { dst, a, b } => {
                    let value = self.bool(a) == self.bool(b);
                    self.set(dst, Value::Bool(value));
                }
                Instr::NeBool { dst, a, b } => {
                    let value = self.bool(a) != self.bool(b);
                    self.set(dst, Value::Bool(value));
                }
                Instr::EqText { dst, a, b } => {
                    let value = self.text(a) == self.text(b);
                    self.set(dst, Value::Bool(value));
                }
                Instr::NeText { dst, a, b } => {
                    let value = self.text(a) != self.text(b);
                    self.set(dst, Value::Bool(value));
                }
                Instr::Not { dst, a } => {
                    let value = !self.bool(a);
                    self.set(dst, Value::Bool(value));
                }

                Instr::IntToFloat { dst, a } => {
                    let value = self.int(a);
                    let (converted, exact) = ops::int_to_float(value);
                    if !exact {
                        self.alert(AlertKind::PrecisionLoss { value, converted }, at);
                    }
                    self.set(dst, Value::Float(converted));
                }
                Instr::FloatToInt { dst, a } => {
                    let value = ops::float_to_int(self.float(a)).map_err(|kind| self.bug(kind, at))?;
                    self.set(dst, Value::Int(value));
                }
                Instr::IntToRational { dst, a } => {
                    let value = self.int(a);
                    self.set(dst, Value::Rational(Rc::new([value, 1])));
                }
                Instr::RationalToFloat { dst, a } => {
                    let value = ops::rational_to_float(self.rational(a));
                    self.set(dst, Value::Float(value));
                }
                Instr::ToText { dst, a } => {
                    let text = self.stack[self.base + a as usize].to_text();
                    self.set(dst, Value::Text(Rc::new(text)));
                }
                Instr::Concat { dst, start, count } => {
                    let mut joined = String::new();
                    for reg in start..start + count {
                        joined.push_str(self.text(reg));
                    }
                    self.set(dst, Value::Text(Rc::new(joined)));
                }

                Instr::Jump { target } => pc = target as usize,
                Instr::JumpIfFalse { cond, target } => {
                    if !self.bool(cond) {
                        pc = target as usize;
                    }
                }
                Instr::JumpUnlessInt { cmp, a, b, target } => {
                    if !cmp.holds(self.int(a), self.int(b)) {
                        pc = target as usize;
                    }
                }
                Instr::JumpUnlessIntImm { cmp, a, imm, target } => {
                    if !cmp.holds(self.int(a), i64::from(imm)) {
                        pc = target as usize;
                    }
                }
                Instr::JumpIfTrue { cond, target } => {
                    if self.bool(cond) {
                        pc = target as usize;
                    }
                }

                Instr::Show { src } => {
                    let text = self.stack[self.base + src as usize].to_text();
                    writeln!(self.out, "{text}").map_err(|error| Box::new(Trap::Io(error)))?;
                }
                Instr::ExpectFailed { message } => {
                    let message = self.text(message).to_string();
                    self.failures.push(Failure { span: self.chunk.spans[at], message });
                }
                Instr::Ask { dst, prompt } => {
                    let line = self.ask(prompt).map_err(|error| Box::new(Trap::Io(error)))?;
                    self.set(dst, Value::Text(Rc::new(line)));
                }
                Instr::Exit { code } => {
                    let code = self.int(code);
                    let code =
                        u8::try_from(code).map_err(|_| self.bug(BugKind::InvalidExitCode { code }, at))?;
                    return Err(Box::new(Trap::Exit(code)));
                }
                Instr::Reverse { dst, a } => {
                    let reversed = match &self.stack[self.base + a as usize] {
                        Value::List(elements) => {
                            Value::List(Rc::new(elements.iter().rev().cloned().collect()))
                        }
                        Value::Text(text) => Value::Text(Rc::new(text.chars().rev().collect())),
                        other => self.mismatch("List or Text", other),
                    };
                    self.set(dst, reversed);
                }
                Instr::Floor { dst, a } => {
                    let value = ops::float_floor(self.float(a)).map_err(|kind| self.bug(kind, at))?;
                    self.set(dst, Value::Int(value));
                }
                Instr::Ceil { dst, a } => {
                    let value = ops::float_ceil(self.float(a)).map_err(|kind| self.bug(kind, at))?;
                    self.set(dst, Value::Int(value));
                }
                Instr::Round { dst, a } => {
                    let value = ops::float_round(self.float(a)).map_err(|kind| self.bug(kind, at))?;
                    self.set(dst, Value::Int(value));
                }
                Instr::Native { dst, native, start, count } => {
                    let first = self.base + start as usize;
                    let args = &self.stack[first..first + count as usize];
                    let result = crate::natives::call(native, args).map_err(|kind| self.bug(kind, at))?;
                    if let Some(value) = crate::natives::made_special_float(native, args, &result) {
                        self.alert(AlertKind::SpecialFloat { value }, at);
                    }
                    self.set(dst, result);
                }
                Instr::MakeClosure { dst, function, start, count } => {
                    let first = self.base + start as usize;
                    let captures = self.stack[first..first + count as usize].to_vec();
                    let name = Rc::clone(&self.program.functions[function as usize].label);
                    let closure = Closure { function, name, bound: Vec::new(), captures };
                    self.set(dst, Value::Function(Rc::new(closure)));
                }
                Instr::Bind { dst, callee, start, count } => {
                    let closure = match &self.stack[self.base + callee as usize] {
                        Value::Function(closure) => Rc::clone(closure),
                        other => self.mismatch("function", other),
                    };
                    let first = self.base + start as usize;
                    let mut bound = closure.bound.clone();
                    bound.extend_from_slice(&self.stack[first..first + count as usize]);
                    let name = Rc::clone(&closure.name);
                    let captures = closure.captures.clone();
                    let bound = Closure { function: closure.function, name, bound, captures };
                    self.set(dst, Value::Function(Rc::new(bound)));
                }
                Instr::CallValue { dst, callee, args, count } => {
                    let closure = match &self.stack[self.base + callee as usize] {
                        Value::Function(closure) => Rc::clone(closure),
                        other => self.mismatch("function", other),
                    };
                    // The arguments given before come first (§11.3); the captured values
                    // follow the parameters (§11.5).
                    let function = closure.function as usize;
                    let params = self.program.functions[function].params as usize;
                    let start = self.base + args as usize;
                    let bound = closure.bound.len();
                    let needed = start + params.max(bound + count as usize) + closure.captures.len();
                    if self.stack.len() < needed {
                        self.stack.resize(needed, Value::None);
                    }
                    if bound > 0 {
                        for offset in (0..count as usize).rev() {
                            self.stack[start + bound + offset] =
                                std::mem::take(&mut self.stack[start + offset]);
                        }
                        for (offset, value) in closure.bound.iter().enumerate() {
                            self.stack[start + offset] = value.clone();
                        }
                    }
                    for (offset, value) in closure.captures.iter().enumerate() {
                        self.stack[start + params + offset] = value.clone();
                    }
                    let count = count + bound as u32;
                    pc = self.call(function, dst, args, count, pc, at)?;
                    code = &self.chunk.code;
                }
                Instr::MakeTask { dst, src } => {
                    let result = self.stack[self.base + src as usize].clone();
                    self.set(dst, Value::Task(Rc::new(result)));
                }
                Instr::Wait { dst, src } => {
                    let result = match &self.stack[self.base + src as usize] {
                        Value::Task(result) => (**result).clone(),
                        other => self.mismatch("task", other),
                    };
                    self.set(dst, result);
                }
                Instr::NewCell { dst } => {
                    self.set(dst, Value::Cell(Rc::new(RefCell::new(Value::None))));
                }
                Instr::LoadCell { dst, cell } => {
                    let value = self.cell(cell).borrow().clone();
                    self.set(dst, value);
                }
                Instr::StoreCell { cell, src } => {
                    let value = self.stack[self.base + src as usize].clone();
                    *self.cell(cell).borrow_mut() = value;
                }
                Instr::TakeCell { dst, cell } => {
                    let value = std::mem::take(&mut *self.cell(cell).borrow_mut());
                    self.set(dst, value);
                }
                Instr::InitModule { module, dst } => {
                    let module = module as usize;
                    if !self.initialized[module] {
                        self.initialized[module] = true;
                        if let Some(init) = self.program.module_inits[module] {
                            pc = self.call(init as usize, dst, dst, 0, pc, at)?;
                            code = &self.chunk.code;
                        }
                    }
                }
                Instr::Isqrt { dst, a } => {
                    let value = ops::isqrt(self.int(a)).map_err(|kind| self.bug(kind, at))?;
                    self.set(dst, Value::Int(value));
                }
                Instr::Call { function, dst, args, count } => {
                    pc = self.call(function as usize, dst, args, count, pc, at)?;
                    code = &self.chunk.code;
                }
                Instr::Return { src } => {
                    let value = std::mem::take(&mut self.stack[self.base + src as usize]);
                    pc = self.return_to_caller(value);
                    code = &self.chunk.code;
                    if self.frames.len() < self.stop_depth {
                        return Ok(());
                    }
                }
                Instr::ReturnNone => {
                    pc = self.return_to_caller(Value::None);
                    code = &self.chunk.code;
                    if self.frames.len() < self.stop_depth {
                        return Ok(());
                    }
                }
                Instr::JumpIfArgs { count, target } => {
                    if self.frames.last().expect("a frame runs").args >= count {
                        pc = target as usize;
                    }
                }
                Instr::LoadGlobal { dst, global } => {
                    let value = self.stack[global as usize].clone();
                    self.set(dst, value);
                }
                Instr::StoreGlobal { global, src } => {
                    self.stack[global as usize] = self.stack[self.base + src as usize].clone();
                }
                Instr::RefLocal { dst, src } => self.set(dst, Value::Ref((self.base + src as usize) as u32)),
                Instr::RefGlobal { dst, global } => self.set(dst, Value::Ref(global)),
                Instr::LoadRef { dst, reference } => {
                    let target = self.reference(reference);
                    let value = self.stack[target].clone();
                    self.set(dst, value);
                }
                Instr::StoreRef { reference, src } => {
                    let target = self.reference(reference);
                    self.stack[target] = self.stack[self.base + src as usize].clone();
                }
                Instr::MakeRange { dst, a, b } => {
                    let bounds = [self.int(a), self.int(b)];
                    self.set(dst, Value::Range(Rc::new(bounds)));
                }
                Instr::InRange { dst, a, b } => {
                    let value = self.int(a);
                    let [start, end] = self.range(b);
                    self.set(dst, Value::Bool(start <= value && value <= end));
                }
                Instr::ForRange { range, counter, target } => {
                    let [start, end] = self.range(range);
                    if start > end {
                        pc = target as usize;
                    } else {
                        self.set(counter, Value::Int(start));
                    }
                }
                Instr::NextRange { range, counter, target } => {
                    let [_, end] = self.range(range);
                    let current = self.int(counter);
                    if current < end {
                        self.set(counter, Value::Int(current + 1));
                        pc = target as usize;
                    }
                }
                Instr::MakeList { dst, start, count } => {
                    let first = self.base + start as usize;
                    let elements = self.stack[first..first + count as usize].to_vec();
                    self.set(dst, Value::List(Rc::new(elements)));
                }
                Instr::GetIndex { dst, object, index } => {
                    let value = self.get_index(object, index).map_err(|kind| self.bug(kind, at))?;
                    self.set(dst, value);
                }
                Instr::GetSlice { dst, object, range } => {
                    let value = self.get_slice(object, range).map_err(|kind| self.bug(kind, at))?;
                    self.set(dst, value);
                }
                Instr::GetSize { dst, object } => {
                    let size = match &self.stack[self.base + object as usize] {
                        Value::List(elements) => elements.len() as i64,
                        Value::Set(set) => set.len() as i64,
                        Value::Text(text) => text.chars().count() as i64,
                        Value::Range(bounds) => {
                            range_size(bounds[0], bounds[1]).map_err(|kind| self.bug(kind, at))?
                        }
                        other => self.mismatch("List, Text or Range", other),
                    };
                    self.set(dst, Value::Int(size));
                }
                Instr::GetFirst { dst, list } => {
                    let value =
                        self.list(list).first().cloned().ok_or_else(|| self.bug(BugKind::EmptyList, at))?;
                    self.set(dst, value);
                }
                Instr::GetLast { dst, list } => {
                    let value =
                        self.list(list).last().cloned().ok_or_else(|| self.bug(BugKind::EmptyList, at))?;
                    self.set(dst, value);
                }
                Instr::InList { dst, a, b } if self.program.custom_equality => {
                    let value = self.stack[self.base + a as usize].clone();
                    let Value::List(list) = self.stack[self.base + b as usize].clone() else {
                        self.mismatch("List", &self.stack[self.base + b as usize])
                    };
                    let mut found = false;
                    for element in list.iter() {
                        if self.equal(element, &value, at)? {
                            found = true;
                            break;
                        }
                    }
                    self.set(dst, Value::Bool(found));
                }
                Instr::InList { dst, a, b } => {
                    let value = &self.stack[self.base + a as usize];
                    let found = self.list(b).iter().any(|element| element.equals(value));
                    self.set(dst, Value::Bool(found));
                }
                Instr::EqValue { dst, a, b } | Instr::NeValue { dst, a, b } => {
                    let (first, second) = (
                        self.stack[self.base + a as usize].clone(),
                        self.stack[self.base + b as usize].clone(),
                    );
                    let equal = self.equal(&first, &second, at)?;
                    let negate = matches!(code[at], Instr::NeValue { .. });
                    self.set(dst, Value::Bool(equal != negate));
                }
                Instr::ForList { list, counter, target } => {
                    if self.list(list).is_empty() {
                        pc = target as usize;
                    } else {
                        self.set(counter, Value::Int(0));
                    }
                }
                Instr::ElementAt { dst, list, counter } => {
                    let position = self.int(counter) as usize;
                    let value = self.list(list)[position].clone();
                    self.set(dst, value);
                }
                Instr::NextList { list, counter, target } => {
                    let next = self.int(counter) + 1;
                    if (next as usize) < self.list(list).len() {
                        self.set(counter, Value::Int(next));
                        pc = target as usize;
                    }
                }
                Instr::StoreElement { target, indices, depth, src } => {
                    let value = self.stack[self.base + src as usize].clone();
                    let stored = self.element_mut(target, indices, depth).map(|slot| *slot = value);
                    stored.map_err(|kind| self.bug(kind, at))?;
                }
                Instr::AddElement { target, indices, depth, src } if self.program.custom_equality => {
                    let value = self.stack[self.base + src as usize].clone();
                    // A Set gets the value unless an element equals it, as its `equals` says.
                    let set = match self.element_mut(target, indices, depth) {
                        Ok(Value::Set(set)) => Some(Rc::clone(set)),
                        _ => None,
                    };
                    if let Some(set) = set {
                        if holds_nan(&value) {
                            return Err(self.bug(BugKind::NanInSet, at));
                        }
                        let present = self.set_contains(&set, &value, at)?;
                        drop(set);
                        if !present {
                            let added = self.element_mut(target, indices, depth).map(|slot| {
                                let Value::Set(set) = slot else { unreachable!("a Set above") };
                                Rc::make_mut(set).push_new(value);
                            });
                            added.map_err(|kind| self.bug(kind, at))?;
                        }
                    } else {
                        let added = self.element_mut(target, indices, depth).map(|slot| match slot {
                            Value::List(elements) => Rc::make_mut(elements).push(value),
                            other => {
                                panic!("the virtual machine expected a List but found {}", other.type_name())
                            }
                        });
                        added.map_err(|kind| self.bug(kind, at))?;
                    }
                }
                Instr::AddElement { target, indices, depth, src } => {
                    let value = self.stack[self.base + src as usize].clone();
                    let added = self.element_mut(target, indices, depth).and_then(|slot| match slot {
                        Value::List(elements) => {
                            Rc::make_mut(elements).push(value);
                            Ok(())
                        }
                        Value::Set(set) => Rc::make_mut(set).insert(value).map_err(|_| BugKind::NanInSet),
                        other => {
                            panic!(
                                "the virtual machine expected a List or a Set but found {}",
                                other.type_name()
                            )
                        }
                    });
                    added.map_err(|kind| self.bug(kind, at))?;
                }
                Instr::MakeSet { dst, start, count } if self.program.custom_equality => {
                    let first = self.base + start as usize;
                    let values = self.stack[first..first + count as usize].to_vec();
                    let mut set = SetValue::default();
                    for value in values {
                        self.set_insert(&mut set, value, at)?;
                    }
                    self.set(dst, Value::Set(Rc::new(set)));
                }
                Instr::InSet { dst, a, b } if self.program.custom_equality => {
                    let value = self.stack[self.base + a as usize].clone();
                    let set = self.set_rc(b);
                    let found = self.set_contains(&set, &value, at)?;
                    self.set(dst, Value::Bool(found));
                }
                Instr::SetUnion { dst, a, b }
                | Instr::SetInter { dst, a, b }
                | Instr::SetMinus { dst, a, b }
                | Instr::Subset { dst, a, b }
                    if self.program.custom_equality =>
                {
                    let (first, second) = (self.set_rc(a), self.set_rc(b));
                    let result = match code[at] {
                        Instr::SetUnion { .. } => {
                            let mut result = (*first).clone();
                            for value in second.items() {
                                self.set_insert(&mut result, value.clone(), at)?;
                            }
                            Value::Set(Rc::new(result))
                        }
                        Instr::Subset { .. } => Value::Bool(self.set_subset(&first, &second, at)?),
                        ref instr => {
                            let keep_common = matches!(instr, Instr::SetInter { .. });
                            let mut result = SetValue::default();
                            for value in first.items() {
                                if self.set_contains(&second, value, at)? == keep_common {
                                    result.push_new(value.clone());
                                }
                            }
                            Value::Set(Rc::new(result))
                        }
                    };
                    self.set(dst, result);
                }
                Instr::MakeSet { dst, start, count } => {
                    let first = self.base + start as usize;
                    let values = self.stack[first..first + count as usize].to_vec();
                    let set = SetValue::from_values(values).map_err(|_| self.bug(BugKind::NanInSet, at))?;
                    self.set(dst, Value::Set(Rc::new(set)));
                }
                Instr::MakeTuple { dst, start, count } => {
                    let first = self.base + start as usize;
                    let elements = self.stack[first..first + count as usize].to_vec();
                    self.set(dst, Value::Tuple(Rc::new(elements)));
                }
                Instr::InSet { dst, a, b } => {
                    let found = self.set_value(b).contains(&self.stack[self.base + a as usize]);
                    self.set(dst, Value::Bool(found));
                }
                Instr::SetUnion { dst, a, b } => {
                    let result = self.set_value(a).union(self.set_value(b));
                    self.set(dst, Value::Set(Rc::new(result)));
                }
                Instr::SetInter { dst, a, b } => {
                    let result = self.set_value(a).inter(self.set_value(b));
                    self.set(dst, Value::Set(Rc::new(result)));
                }
                Instr::SetMinus { dst, a, b } => {
                    let result = self.set_value(a).minus(self.set_value(b));
                    self.set(dst, Value::Set(Rc::new(result)));
                }
                Instr::Subset { dst, a, b } => {
                    let result = self.set_value(a).subset(self.set_value(b));
                    self.set(dst, Value::Bool(result));
                }
                Instr::SumInt { dst, values } => {
                    let total = match &self.stack[self.base + values as usize] {
                        Value::Range(bounds) => range_sum(bounds[0], bounds[1]),
                        _ => self.list(values).iter().try_fold(0i64, |total, element| match element {
                            Value::Int(value) => ops::int_add(total, *value),
                            other => self.mismatch("Int", other),
                        }),
                    }
                    .map_err(|kind| self.bug(kind, at))?;
                    self.set(dst, Value::Int(total));
                }
                Instr::SumFloat { dst, values } => {
                    let mut total = 0.0;
                    let mut finite = true;
                    for element in self.list(values).iter() {
                        let Value::Float(value) = element else { self.mismatch("Float", element) };
                        finite &= value.is_finite();
                        total += value;
                    }
                    if finite && !total.is_finite() {
                        self.alert(AlertKind::SpecialFloat { value: total }, at);
                    }
                    self.set(dst, Value::Float(total));
                }
                Instr::SumRational { dst, values } => {
                    let total = self
                        .list(values)
                        .iter()
                        .try_fold([0, 1], |total, element| match element {
                            Value::Rational(value) => ops::rational_op(RationalOp::Add, total, **value),
                            other => self.mismatch("Rational", other),
                        })
                        .map_err(|kind| self.bug(kind, at))?;
                    self.set(dst, Value::Rational(Rc::new(total)));
                }
                Instr::TypeTest { dst, src, kinds } => {
                    let kind = self.stack[self.base + src as usize].kind();
                    self.set(dst, Value::Bool(kind & kinds != 0));
                }
                Instr::TypeTestNamed { dst, src, kinds, set } => {
                    let value = &self.stack[self.base + src as usize];
                    let number = match value {
                        Value::Struct(record) => Some(record.layout.index),
                        Value::Enum(enumeration, _) => Some(enumeration.index),
                        _ => None,
                    };
                    let result = value.kind() & kinds != 0
                        || number
                            .is_some_and(|number| self.program.type_sets[set as usize].contains(&number));
                    self.set(dst, Value::Bool(result));
                }
                Instr::LoadEnum { dst, enumeration, value } => {
                    let enumeration = Rc::clone(&self.program.enums[enumeration as usize]);
                    self.set(dst, Value::Enum(enumeration, value));
                }
                Instr::EnumPosition { dst, a } => {
                    let position = match &self.stack[self.base + a as usize] {
                        Value::Enum(_, value) => i64::from(*value),
                        other => self.mismatch("enumeration", other),
                    };
                    self.set(dst, Value::Int(position));
                }
                Instr::MakeStruct { dst, layout, start, count } => {
                    let first = self.base + start as usize;
                    let fields = self.stack[first..first + count as usize].to_vec();
                    let layout = Rc::clone(&self.program.layouts[layout as usize]);
                    self.set(dst, Value::Struct(Rc::new(Record { layout, fields })));
                }
                Instr::GetField { dst, object, field } => {
                    let value = match &self.stack[self.base + object as usize] {
                        Value::Struct(record) => record.fields[field as usize].clone(),
                        other => self.mismatch("structure", other),
                    };
                    self.set(dst, value);
                }
                Instr::Literal { dst, a } => {
                    let text = self.stack[self.base + a as usize].literal();
                    self.set(dst, Value::Text(Rc::new(text)));
                }
                Instr::Broken { name, detail } => {
                    let kind = BugKind::BrokenInvariant {
                        structure: self.text(name).to_string(),
                        detail: self.text(detail).to_string(),
                    };
                    return Err(self.bug(kind, at));
                }
                Instr::Try { dst, src, errors } => {
                    let value = self.stack[self.base + src as usize].clone();
                    // An Error of the program: a structure or an enumeration of the set.
                    let own = errors != u32::MAX && {
                        let number = match &value {
                            Value::Struct(record) => Some(record.layout.index),
                            Value::Enum(enumeration, _) => Some(enumeration.index),
                            _ => None,
                        };
                        number.is_some_and(|number| self.program.type_sets[errors as usize].contains(&number))
                    };
                    if matches!(value, Value::Error(_)) || own {
                        if self.frames.len() == 1 {
                            let span = self.chunk.spans[at];
                            let message = match &value {
                                Value::Error(message) => message.to_string(),
                                other => other.to_text(),
                            };
                            return Err(Box::new(Trap::Failure { message, span }));
                        }
                        pc = self.return_to_caller(value);
                        code = &self.chunk.code;
                    } else {
                        self.set(dst, value);
                    }
                }
                Instr::Fail { message } => {
                    let message = self.text(message).to_string();
                    return Err(Box::new(Trap::Failure { message, span: self.chunk.spans[at] }));
                }
                Instr::NewError { dst, a } => {
                    let message = self.text(a).to_string();
                    self.set(dst, Value::Error(Rc::new(message)));
                }
                Instr::ErrorMessage { dst, a } => {
                    let message = match &self.stack[self.base + a as usize] {
                        Value::Error(message) => message.to_string(),
                        other => self.mismatch("Error", other),
                    };
                    self.set(dst, Value::Text(Rc::new(message)));
                }
                Instr::TextToInt { dst, a } => {
                    let value = match ops::parse_int(self.text(a)) {
                        Ok(value) => Value::Int(value),
                        Err(message) => Value::Error(Rc::new(message)),
                    };
                    self.set(dst, value);
                }
                Instr::TextToFloat { dst, a } => {
                    let value = match ops::parse_float(self.text(a)) {
                        Ok(value) => Value::Float(value),
                        Err(message) => Value::Error(Rc::new(message)),
                    };
                    self.set(dst, value);
                }
                Instr::Halt => return Ok(()),
            }
        }
    }

    /// Enters a function; returns where it starts.
    fn call(
        &mut self,
        function: usize,
        dst: Reg,
        args: Reg,
        count: u32,
        resume: usize,
        at: usize,
    ) -> Result<usize, Fault> {
        if self.frames.len() >= MAX_CALL_DEPTH {
            return Err(self.bug(BugKind::StackOverflow, at));
        }
        let program = self.program;
        let callee = &program.functions[function];
        // The frame of the callee starts at the arguments, which become its first
        // registers without being copied: the compiler puts the arguments in the last
        // registers it has reserved, so the ones above are free during the call. The
        // other registers are not reset, as the compiler writes every register before
        // reading it; the stack only grows.
        let _ = count;
        let base = self.base + args as usize;
        let needed = base + callee.registers as usize;
        if self.stack.len() < needed {
            self.stack.resize(needed, Value::None);
        }
        self.frames.last_mut().expect("a frame runs").resume = resume as u32;
        let (function, call) = (function as u32, at as u32);
        self.frames.push(Frame { function, base: base as u32, resume: 0, dst, args: count, call });
        self.chunk = callee;
        self.base = base;
        Ok(0)
    }

    /// Calls a function of the program from an instruction, as `equals`, and gives its
    /// result. Its frame starts above the registers of the current one.
    fn invoke(&mut self, function: u32, args: Vec<Value>, at: usize) -> Result<Value, Fault> {
        let offset = self.chunk.registers;
        let base = self.base + offset as usize;
        if self.stack.len() < base + args.len() {
            self.stack.resize(base + args.len(), Value::None);
        }
        let count = args.len() as u32;
        for (position, arg) in args.into_iter().enumerate() {
            self.stack[base + position] = arg;
        }
        self.call(function as usize, offset, offset, count, at + 1, at)?;
        let outer = std::mem::replace(&mut self.stop_depth, self.frames.len());
        let result = self.run_at(0);
        self.stop_depth = outer;
        result?;
        Ok(std::mem::take(&mut self.stack[self.base + offset as usize]))
    }

    /// Equality of content (§9.4), with the `equals` of the structures that define one
    /// (§12.5).
    fn equal(&mut self, a: &Value, b: &Value, at: usize) -> Result<bool, Fault> {
        if !self.program.custom_equality {
            return Ok(a.equals(b));
        }
        Ok(match (a, b) {
            (Value::Struct(x), Value::Struct(y)) if x.layout.index == y.layout.index => match x.layout.equals
            {
                Some(function) => match self.invoke(function, vec![a.clone(), b.clone()], at)? {
                    Value::Bool(equal) => equal,
                    other => self.mismatch("Bool", &other),
                },
                None => {
                    let (x, y) = (Rc::clone(x), Rc::clone(y));
                    for (first, second) in x.fields.iter().zip(&y.fields) {
                        if !self.equal(first, second, at)? {
                            return Ok(false);
                        }
                    }
                    true
                }
            },
            (Value::List(x), Value::List(y)) | (Value::Tuple(x), Value::Tuple(y)) => {
                if x.len() != y.len() {
                    return Ok(false);
                }
                let (x, y) = (Rc::clone(x), Rc::clone(y));
                for (first, second) in x.iter().zip(y.iter()) {
                    if !self.equal(first, second, at)? {
                        return Ok(false);
                    }
                }
                true
            }
            (Value::Set(x), Value::Set(y)) => {
                let (x, y) = (Rc::clone(x), Rc::clone(y));
                x.len() == y.len() && self.set_subset(&x, &y, at)?
            }
            _ => a.equals(b),
        })
    }

    /// Whether an element of the Set equals `value`, as the machine compares values.
    fn set_contains(&mut self, set: &SetValue, value: &Value, at: usize) -> Result<bool, Fault> {
        if holds_nan(value) {
            return Ok(false);
        }
        let candidates: Vec<Value> = set.candidates(value).cloned().collect();
        for candidate in &candidates {
            if self.equal(candidate, value, at)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn set_insert(&mut self, set: &mut SetValue, value: Value, at: usize) -> Result<(), Fault> {
        if holds_nan(&value) {
            return Err(self.bug(BugKind::NanInSet, at));
        }
        if !self.set_contains(set, &value, at)? {
            set.push_new(value);
        }
        Ok(())
    }

    fn set_subset(&mut self, first: &SetValue, second: &SetValue, at: usize) -> Result<bool, Fault> {
        for value in first.items() {
            if !self.set_contains(second, value, at)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn set_rc(&self, reg: Reg) -> Rc<SetValue> {
        match &self.stack[self.base + reg as usize] {
            Value::Set(set) => Rc::clone(set),
            other => self.mismatch("Set", other),
        }
    }

    /// Leaves the current function with its result; returns where the caller resumes.
    fn return_to_caller(&mut self, value: Value) -> usize {
        let finished = self.frames.pop().expect("a function returns");
        // The values of the finished frame that hold memory are released now (§17.3);
        // the others are overwritten later.
        let end = finished.base as usize + self.chunk.registers as usize;
        for value in &mut self.stack[finished.base as usize..end] {
            if value.holds_memory() {
                *value = Value::None;
            }
        }
        let caller = self.frames.last().expect("the script does not return");
        let program = self.program;
        self.chunk = &program.functions[caller.function as usize];
        self.base = caller.base as usize;
        self.stack[self.base + finished.dst as usize] = value;
        caller.resume as usize
    }

    #[inline]
    fn int_imm(
        &mut self,
        dst: Reg,
        a: Reg,
        imm: i32,
        at: usize,
        op: fn(i64, i64) -> Result<i64, BugKind>,
    ) -> Result<(), Fault> {
        let value = op(self.int(a), i64::from(imm)).map_err(|kind| self.bug(kind, at))?;
        self.set(dst, Value::Int(value));
        Ok(())
    }

    #[inline]
    fn int_op(
        &mut self,
        dst: Reg,
        a: Reg,
        b: Reg,
        at: usize,
        op: fn(i64, i64) -> Result<i64, BugKind>,
    ) -> Result<(), Fault> {
        let value = op(self.int(a), self.int(b)).map_err(|kind| self.bug(kind, at))?;
        self.set(dst, Value::Int(value));
        Ok(())
    }

    fn float_op(
        &mut self,
        dst: Reg,
        a: Reg,
        b: Reg,
        at: usize,
        op: fn(f64, f64) -> f64,
    ) -> Result<(), Fault> {
        let (x, y) = (self.float(a), self.float(b));
        let value = op(x, y);
        if !value.is_finite() && x.is_finite() && y.is_finite() {
            self.alert(AlertKind::SpecialFloat { value }, at);
        }
        self.set(dst, Value::Float(value));
        Ok(())
    }

    fn rational_op(&mut self, dst: Reg, a: Reg, b: Reg, at: usize, op: RationalOp) -> Result<(), Fault> {
        let value =
            ops::rational_op(op, self.rational(a), self.rational(b)).map_err(|kind| self.bug(kind, at))?;
        self.set(dst, Value::Rational(Rc::new(value)));
        Ok(())
    }

    fn int_test(&mut self, dst: Reg, a: Reg, b: Reg, test: fn(i64, i64) -> bool) {
        let value = test(self.int(a), self.int(b));
        self.set(dst, Value::Bool(value));
    }

    fn float_test(&mut self, dst: Reg, a: Reg, b: Reg, test: fn(f64, f64) -> bool) {
        let value = test(self.float(a), self.float(b));
        self.set(dst, Value::Bool(value));
    }

    fn set(&mut self, reg: Reg, value: Value) {
        self.stack[self.base + reg as usize] = value;
    }

    // A value of the wrong type in a register is a defect of the implementation, not
    // of the program: it panics, and the `lion` command reports an internal error.

    fn int(&self, reg: Reg) -> i64 {
        match self.stack[self.base + reg as usize] {
            Value::Int(value) => value,
            ref other => self.mismatch("Int", other),
        }
    }

    fn float(&self, reg: Reg) -> f64 {
        match self.stack[self.base + reg as usize] {
            Value::Float(value) => value,
            ref other => self.mismatch("Float", other),
        }
    }

    fn rational(&self, reg: Reg) -> [i64; 2] {
        match &self.stack[self.base + reg as usize] {
            Value::Rational(value) => **value,
            other => self.mismatch("Rational", other),
        }
    }

    fn bool(&self, reg: Reg) -> bool {
        match self.stack[self.base + reg as usize] {
            Value::Bool(value) => value,
            ref other => self.mismatch("Bool", other),
        }
    }

    fn text(&self, reg: Reg) -> &str {
        match &self.stack[self.base + reg as usize] {
            Value::Text(text) => text,
            other => self.mismatch("Text", other),
        }
    }

    /// The elements of a List, or of a Set in their order.
    fn list(&self, reg: Reg) -> &[Value] {
        match &self.stack[self.base + reg as usize] {
            Value::List(elements) => elements,
            Value::Set(set) => set.items(),
            other => self.mismatch("List or Set", other),
        }
    }

    fn cell(&self, reg: Reg) -> &RefCell<Value> {
        match &self.stack[self.base + reg as usize] {
            Value::Cell(cell) => cell,
            other => self.mismatch("cell", other),
        }
    }

    fn set_value(&self, reg: Reg) -> &SetValue {
        match &self.stack[self.base + reg as usize] {
            Value::Set(set) => set,
            other => self.mismatch("Set", other),
        }
    }

    /// `ask(prompt)`: the prompt, followed by a space, then a line without its end. At the
    /// end of the input, the line is empty (C59).
    fn ask(&mut self, prompt: Reg) -> io::Result<String> {
        let prompt = self.text(prompt).to_string();
        if !prompt.is_empty() {
            write!(self.out, "{prompt} ")?;
        }
        self.out.flush()?;
        let mut line = String::new();
        self.input.read_line(&mut line)?;
        if line.ends_with('\n') {
            line.pop();
            if line.ends_with('\r') {
                line.pop();
            }
        }
        Ok(line)
    }

    /// `l[i]` or `t[i]`, from 1.
    fn get_index(&self, object: Reg, index: Reg) -> Result<Value, BugKind> {
        let index = self.int(index);
        match &self.stack[self.base + object as usize] {
            Value::List(elements) => {
                let position = position(index, elements.len())?;
                Ok(elements[position].clone())
            }
            Value::Text(text) => {
                let size = text.chars().count();
                let position = position(index, size)?;
                let character = text.chars().nth(position).expect("the position is inside the text");
                Ok(Value::Text(Rc::new(character.to_string())))
            }
            other => self.mismatch("List or Text", other),
        }
    }

    /// `l[a..b]` or `t[a..b]`, bounds included; an empty interval gives an empty extract.
    fn get_slice(&self, object: Reg, range: Reg) -> Result<Value, BugKind> {
        let [start, end] = self.range(range);
        let object = &self.stack[self.base + object as usize];
        let size = match object {
            Value::List(elements) => elements.len(),
            Value::Text(text) => text.chars().count(),
            other => self.mismatch("List or Text", other),
        };
        let (from, to) = if start > end {
            (0, 0)
        } else if start < 1 || end > size as i64 {
            return Err(BugKind::SliceOutOfRange { start, end, size });
        } else {
            (start as usize - 1, end as usize)
        };
        Ok(match object {
            Value::List(elements) => Value::List(Rc::new(elements[from..to].to_vec())),
            Value::Text(text) => Value::Text(Rc::new(text.chars().skip(from).take(to - from).collect())),
            _ => unreachable!("checked above"),
        })
    }

    /// The element reached from a variable through indices, ready to be changed: every
    /// list on the way is copied first if it is shared (§17.1).
    fn element_mut(&mut self, target: Target, indices: Reg, depth: u32) -> Result<&mut Value, BugKind> {
        let root = match target {
            Target::Register(reg) => self.base + reg as usize,
            Target::Global(global) => global as usize,
            Target::Reference(reg) => self.reference(reg),
        };
        let steps: Vec<i64> = (0..depth).map(|offset| self.int(indices + offset)).collect();
        let mut slot = &mut self.stack[root];
        for step in steps {
            slot = match slot {
                Value::List(elements) => {
                    let elements = Rc::make_mut(elements);
                    let position = position(step, elements.len())?;
                    &mut elements[position]
                }
                // The position of a field, from 0.
                Value::Struct(record) => &mut Rc::make_mut(record).fields[step as usize],
                other => panic!(
                    "the virtual machine expected a List or a structure but found {}",
                    other.type_name()
                ),
            };
        }
        Ok(slot)
    }

    fn range(&self, reg: Reg) -> [i64; 2] {
        match &self.stack[self.base + reg as usize] {
            Value::Range(bounds) => **bounds,
            other => self.mismatch("Range", other),
        }
    }

    fn reference(&self, reg: Reg) -> usize {
        match self.stack[self.base + reg as usize] {
            Value::Ref(target) => target as usize,
            ref other => self.mismatch("reference", other),
        }
    }

    #[cold]
    fn mismatch(&self, expected: &str, found: &Value) -> ! {
        panic!(
            "the virtual machine expected {expected} but found {} in `{}`",
            found.type_name(),
            self.chunk.name
        )
    }

    fn bug(&self, kind: BugKind, at: usize) -> Fault {
        Box::new(Trap::Bug { kind, span: self.chunk.spans[at], calls: self.calls() })
    }

    /// The places of the calls in progress, innermost first: each frame but the
    /// script's was entered by a call in the frame below it.
    fn calls(&self) -> Vec<Span> {
        self.frames
            .windows(2)
            .rev()
            .filter_map(|pair| self.program.functions[pair[0].function as usize].spans[pair[1].call as usize])
            .collect()
    }

    fn alert(&mut self, kind: AlertKind, at: usize) {
        let function = self.frames.last().expect("a frame runs").function;
        if self.alerted.insert((function as usize, at)) {
            // What the program wrote so far comes before the alert.
            let _ = self.out.flush();
            let calls = self.calls();
            (self.on_alert)(Alert { kind, span: self.chunk.spans[at], calls });
        }
    }
}

/// The position, from 0, of the 1-based `index` in a sequence of `size` elements.
fn position(index: i64, size: usize) -> Result<usize, BugKind> {
    if index < 1 || index > size as i64 {
        return Err(BugKind::IndexOutOfRange { index, size });
    }
    Ok(index as usize - 1)
}

/// The number of integers in `start..end`, which may not fit in an Int.
fn range_size(start: i64, end: i64) -> Result<i64, BugKind> {
    if start > end {
        return Ok(0);
    }
    ops::int_add(ops::int_sub(end, start)?, 1)
}

/// The sum of the integers in `start..end`.
fn range_sum(start: i64, end: i64) -> Result<i64, BugKind> {
    let mut total = 0i64;
    if start <= end {
        let mut value = start;
        loop {
            total = ops::int_add(total, value)?;
            if value == end {
                break;
            }
            value += 1;
        }
    }
    Ok(total)
}
