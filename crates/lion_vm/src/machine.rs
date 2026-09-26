//! Execution of bytecode.

use std::collections::HashSet;
use std::io::{self, Write};
use std::rc::Rc;

use lion_diagnostics::{Diagnostic, Span};
use lion_runtime::format::format_float;
use lion_runtime::{BugKind, MAX_CALL_DEPTH, ops};

use crate::bytecode::{Chunk, Instr, Program, Reg};
use crate::value::Value;

/// Why execution stopped before the end of the program.
#[derive(Debug)]
pub enum Trap {
    /// A bug in the program (spec §18.1). `calls` are the calls in progress, innermost first.
    Bug { kind: BugKind, span: Option<Span>, calls: Vec<Span> },
    /// Writing the output failed.
    Io(io::Error),
}

impl Trap {
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
        }
    }
}

/// A non-blocking remark of the interpreted mode (spec §22.3).
#[derive(Clone, Debug)]
pub struct Alert {
    pub kind: AlertKind,
    pub span: Option<Span>,
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

/// Runs a program to its end, writing the output of `show` to `out` and reporting
/// alerts to `on_alert` as they happen; `out` is flushed before each alert.
pub fn run(program: &Program, out: &mut dyn Write, on_alert: &mut dyn FnMut(Alert)) -> Result<(), Trap> {
    let main = &program.functions[program.main];
    let mut machine = Machine {
        program,
        chunk: main,
        base: 0,
        stack: vec![Value::None; main.registers as usize],
        frames: vec![Frame { function: program.main, base: 0, resume: 0, dst: 0, args: 0, call: 0 }],
        out,
        on_alert,
        alerted: HashSet::new(),
    };
    machine.run()
}

/// A call in progress. The script's frame is at the bottom of the stack; its
/// registers are the globals.
struct Frame {
    function: usize,
    /// Where its registers start in the stack.
    base: usize,
    /// Where the function resumes once the call it made returns.
    resume: usize,
    /// The caller's register that receives the result.
    dst: Reg,
    /// How many arguments the call gave.
    args: u32,
    /// The call instruction in the caller.
    call: usize,
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
    on_alert: &'a mut dyn FnMut(Alert),
    /// Instructions (function, index) that already reported an alert.
    alerted: HashSet<(usize, usize)>,
}

impl Machine<'_> {
    fn run(&mut self) -> Result<(), Trap> {
        let mut pc = 0;
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

                Instr::AddFloat { dst, a, b } => self.float_op(dst, a, b, at, |x, y| x + y)?,
                Instr::SubFloat { dst, a, b } => self.float_op(dst, a, b, at, |x, y| x - y)?,
                Instr::MulFloat { dst, a, b } => self.float_op(dst, a, b, at, |x, y| x * y)?,
                Instr::DivFloat { dst, a, b } => self.float_op(dst, a, b, at, |x, y| x / y)?,
                Instr::PowFloat { dst, a, b } => self.float_op(dst, a, b, at, ops::float_pow)?,
                Instr::NegFloat { dst, a } => {
                    let value = -self.float(a);
                    self.set(dst, Value::Float(value));
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
                Instr::JumpIfTrue { cond, target } => {
                    if self.bool(cond) {
                        pc = target as usize;
                    }
                }

                Instr::Show { src } => {
                    let text = self.stack[self.base + src as usize].to_text();
                    writeln!(self.out, "{text}").map_err(Trap::Io)?;
                }
                Instr::Call { function, dst, args, count } => {
                    pc = self.call(function as usize, dst, args, count, pc, at)?;
                    code = &self.chunk.code;
                }
                Instr::Return { src } => {
                    let value = std::mem::take(&mut self.stack[self.base + src as usize]);
                    pc = self.return_to_caller(value);
                    code = &self.chunk.code;
                }
                Instr::ReturnNone => {
                    pc = self.return_to_caller(Value::None);
                    code = &self.chunk.code;
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
    ) -> Result<usize, Trap> {
        if self.frames.len() >= MAX_CALL_DEPTH {
            return Err(self.bug(BugKind::StackOverflow, at));
        }
        let program = self.program;
        let callee = &program.functions[function];
        let base = self.base + self.chunk.registers as usize;
        self.stack.resize(base + callee.registers as usize, Value::None);
        for offset in 0..count as usize {
            self.stack[base + offset] = std::mem::take(&mut self.stack[self.base + args as usize + offset]);
        }
        self.frames.last_mut().expect("a frame runs").resume = resume;
        self.frames.push(Frame { function, base, resume: 0, dst, args: count, call: at });
        self.chunk = callee;
        self.base = base;
        Ok(0)
    }

    /// Leaves the current function with its result; returns where the caller resumes.
    fn return_to_caller(&mut self, value: Value) -> usize {
        let finished = self.frames.pop().expect("a function returns");
        self.stack.truncate(finished.base);
        let caller = self.frames.last().expect("the script does not return");
        let program = self.program;
        self.chunk = &program.functions[caller.function];
        self.base = caller.base;
        self.stack[self.base + finished.dst as usize] = value;
        caller.resume
    }

    fn int_op(
        &mut self,
        dst: Reg,
        a: Reg,
        b: Reg,
        at: usize,
        op: fn(i64, i64) -> Result<i64, BugKind>,
    ) -> Result<(), Trap> {
        let value = op(self.int(a), self.int(b)).map_err(|kind| self.bug(kind, at))?;
        self.set(dst, Value::Int(value));
        Ok(())
    }

    fn float_op(&mut self, dst: Reg, a: Reg, b: Reg, at: usize, op: fn(f64, f64) -> f64) -> Result<(), Trap> {
        let (x, y) = (self.float(a), self.float(b));
        let value = op(x, y);
        if !value.is_finite() && x.is_finite() && y.is_finite() {
            self.alert(AlertKind::SpecialFloat { value }, at);
        }
        self.set(dst, Value::Float(value));
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

    fn bug(&self, kind: BugKind, at: usize) -> Trap {
        // Each frame but the script's was entered by a call in the frame below it.
        let calls = self
            .frames
            .windows(2)
            .rev()
            .filter_map(|pair| self.program.functions[pair[0].function].spans[pair[1].call])
            .collect();
        Trap::Bug { kind, span: self.chunk.spans[at], calls }
    }

    fn alert(&mut self, kind: AlertKind, at: usize) {
        let function = self.frames.last().expect("a frame runs").function;
        if self.alerted.insert((function, at)) {
            // What the program wrote so far comes before the alert.
            let _ = self.out.flush();
            (self.on_alert)(Alert { kind, span: self.chunk.spans[at] });
        }
    }
}
