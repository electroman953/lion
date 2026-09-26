//! Execution of bytecode.

use std::collections::HashSet;
use std::io::{self, Write};

use lion_diagnostics::{Diagnostic, Span};
use lion_runtime::format::format_float;
use lion_runtime::{BugKind, ops};

use crate::bytecode::{Chunk, Instr, Reg};
use crate::value::Value;

/// Why execution stopped before the end of the program.
#[derive(Debug)]
pub enum Trap {
    /// A bug in the program (spec §18.1).
    Bug { kind: BugKind, span: Option<Span> },
    /// Writing the output failed.
    Io(io::Error),
    /// A defect in the implementation: the bytecode does not match its types.
    Internal { message: String, span: Option<Span> },
}

impl Trap {
    pub fn to_diagnostic(&self) -> Diagnostic {
        match self {
            Trap::Bug { kind, span } => {
                let mut diagnostic = Diagnostic::bug(kind.message());
                if let Some(span) = span {
                    diagnostic = diagnostic.with_primary(*span, "");
                }
                diagnostic.with_note(kind.details()).with_help(kind.help())
            }
            Trap::Io(error) => Diagnostic::error(format!("cannot write the output: {error}")),
            Trap::Internal { message, span } => {
                let diagnostic = Diagnostic::internal(message.clone());
                match span {
                    Some(span) => diagnostic.with_primary(*span, ""),
                    None => diagnostic,
                }
            }
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

/// Runs a chunk to its end, writing the output of `show` to `out` and reporting
/// alerts to `on_alert` as they happen; `out` is flushed before each alert.
pub fn run(chunk: &Chunk, out: &mut dyn Write, on_alert: &mut dyn FnMut(Alert)) -> Result<(), Trap> {
    let mut machine = Machine {
        chunk,
        registers: vec![Value::None; chunk.registers as usize],
        out,
        on_alert,
        alerted: HashSet::new(),
    };
    machine.run()
}

struct Machine<'a> {
    chunk: &'a Chunk,
    registers: Vec<Value>,
    out: &'a mut dyn Write,
    on_alert: &'a mut dyn FnMut(Alert),
    /// Instructions that already reported an alert.
    alerted: HashSet<usize>,
}

impl Machine<'_> {
    fn run(&mut self) -> Result<(), Trap> {
        let chunk = self.chunk;
        let mut pc = 0;
        loop {
            let at = pc;
            pc += 1;
            match chunk.code[at] {
                Instr::LoadInt { dst, value } => self.set(dst, Value::Int(value)),
                Instr::LoadFloat { dst, value } => self.set(dst, Value::Float(value)),
                Instr::LoadBool { dst, value } => self.set(dst, Value::Bool(value)),
                Instr::LoadNone { dst } => self.set(dst, Value::None),
                Instr::LoadText { dst, index } => {
                    self.set(dst, Value::Text(chunk.texts[index as usize].clone()))
                }
                Instr::Move { dst, src } => {
                    let value = self.registers[src as usize].clone();
                    self.set(dst, value);
                }

                Instr::AddInt { dst, a, b } => self.int_op(dst, a, b, at, ops::int_add)?,
                Instr::SubInt { dst, a, b } => self.int_op(dst, a, b, at, ops::int_sub)?,
                Instr::MulInt { dst, a, b } => self.int_op(dst, a, b, at, ops::int_mul)?,
                Instr::DivInt { dst, a, b } => self.int_op(dst, a, b, at, ops::int_div)?,
                Instr::ModInt { dst, a, b } => self.int_op(dst, a, b, at, ops::int_mod)?,
                Instr::PowInt { dst, a, b } => self.int_op(dst, a, b, at, ops::int_pow)?,
                Instr::NegInt { dst, a } => {
                    let value = ops::int_neg(self.int(a, at)?).map_err(|kind| self.bug(kind, at))?;
                    self.set(dst, Value::Int(value));
                }

                Instr::AddFloat { dst, a, b } => self.float_op(dst, a, b, at, |x, y| x + y)?,
                Instr::SubFloat { dst, a, b } => self.float_op(dst, a, b, at, |x, y| x - y)?,
                Instr::MulFloat { dst, a, b } => self.float_op(dst, a, b, at, |x, y| x * y)?,
                Instr::DivFloat { dst, a, b } => self.float_op(dst, a, b, at, |x, y| x / y)?,
                Instr::PowFloat { dst, a, b } => self.float_op(dst, a, b, at, ops::float_pow)?,
                Instr::NegFloat { dst, a } => {
                    let value = -self.float(a, at)?;
                    self.set(dst, Value::Float(value));
                }

                Instr::EqInt { dst, a, b } => self.int_test(dst, a, b, at, |x, y| x == y)?,
                Instr::NeInt { dst, a, b } => self.int_test(dst, a, b, at, |x, y| x != y)?,
                Instr::LtInt { dst, a, b } => self.int_test(dst, a, b, at, |x, y| x < y)?,
                Instr::LeInt { dst, a, b } => self.int_test(dst, a, b, at, |x, y| x <= y)?,
                Instr::GtInt { dst, a, b } => self.int_test(dst, a, b, at, |x, y| x > y)?,
                Instr::GeInt { dst, a, b } => self.int_test(dst, a, b, at, |x, y| x >= y)?,
                Instr::EqFloat { dst, a, b } => self.float_test(dst, a, b, at, |x, y| x == y)?,
                Instr::NeFloat { dst, a, b } => self.float_test(dst, a, b, at, |x, y| x != y)?,
                Instr::LtFloat { dst, a, b } => self.float_test(dst, a, b, at, |x, y| x < y)?,
                Instr::LeFloat { dst, a, b } => self.float_test(dst, a, b, at, |x, y| x <= y)?,
                Instr::GtFloat { dst, a, b } => self.float_test(dst, a, b, at, |x, y| x > y)?,
                Instr::GeFloat { dst, a, b } => self.float_test(dst, a, b, at, |x, y| x >= y)?,
                Instr::EqBool { dst, a, b } => {
                    let value = self.bool(a, at)? == self.bool(b, at)?;
                    self.set(dst, Value::Bool(value));
                }
                Instr::NeBool { dst, a, b } => {
                    let value = self.bool(a, at)? != self.bool(b, at)?;
                    self.set(dst, Value::Bool(value));
                }
                Instr::EqText { dst, a, b } => {
                    let value = self.text(a, at)? == self.text(b, at)?;
                    self.set(dst, Value::Bool(value));
                }
                Instr::NeText { dst, a, b } => {
                    let value = self.text(a, at)? != self.text(b, at)?;
                    self.set(dst, Value::Bool(value));
                }
                Instr::Not { dst, a } => {
                    let value = !self.bool(a, at)?;
                    self.set(dst, Value::Bool(value));
                }

                Instr::IntToFloat { dst, a } => {
                    let value = self.int(a, at)?;
                    let (converted, exact) = ops::int_to_float(value);
                    if !exact {
                        self.alert(AlertKind::PrecisionLoss { value, converted }, at);
                    }
                    self.set(dst, Value::Float(converted));
                }
                Instr::FloatToInt { dst, a } => {
                    let value = ops::float_to_int(self.float(a, at)?).map_err(|kind| self.bug(kind, at))?;
                    self.set(dst, Value::Int(value));
                }
                Instr::ToText { dst, a } => {
                    let text = self.registers[a as usize].to_text();
                    self.set(dst, Value::Text(text.into()));
                }
                Instr::Concat { dst, start, count } => {
                    let mut joined = String::new();
                    for reg in start..start + count {
                        joined.push_str(self.text(reg, at)?);
                    }
                    self.set(dst, Value::Text(joined.into()));
                }

                Instr::Jump { target } => pc = target as usize,
                Instr::JumpIfFalse { cond, target } => {
                    if !self.bool(cond, at)? {
                        pc = target as usize;
                    }
                }
                Instr::JumpIfTrue { cond, target } => {
                    if self.bool(cond, at)? {
                        pc = target as usize;
                    }
                }

                Instr::Show { src } => {
                    let text = self.registers[src as usize].to_text();
                    writeln!(self.out, "{text}").map_err(Trap::Io)?;
                }
                Instr::Halt => return Ok(()),
            }
        }
    }

    fn int_op(
        &mut self,
        dst: Reg,
        a: Reg,
        b: Reg,
        at: usize,
        op: fn(i64, i64) -> Result<i64, BugKind>,
    ) -> Result<(), Trap> {
        let value = op(self.int(a, at)?, self.int(b, at)?).map_err(|kind| self.bug(kind, at))?;
        self.set(dst, Value::Int(value));
        Ok(())
    }

    fn float_op(&mut self, dst: Reg, a: Reg, b: Reg, at: usize, op: fn(f64, f64) -> f64) -> Result<(), Trap> {
        let (x, y) = (self.float(a, at)?, self.float(b, at)?);
        let value = op(x, y);
        if !value.is_finite() && x.is_finite() && y.is_finite() {
            self.alert(AlertKind::SpecialFloat { value }, at);
        }
        self.set(dst, Value::Float(value));
        Ok(())
    }

    fn int_test(
        &mut self,
        dst: Reg,
        a: Reg,
        b: Reg,
        at: usize,
        test: fn(i64, i64) -> bool,
    ) -> Result<(), Trap> {
        let value = test(self.int(a, at)?, self.int(b, at)?);
        self.set(dst, Value::Bool(value));
        Ok(())
    }

    fn float_test(
        &mut self,
        dst: Reg,
        a: Reg,
        b: Reg,
        at: usize,
        test: fn(f64, f64) -> bool,
    ) -> Result<(), Trap> {
        let value = test(self.float(a, at)?, self.float(b, at)?);
        self.set(dst, Value::Bool(value));
        Ok(())
    }

    fn set(&mut self, reg: Reg, value: Value) {
        self.registers[reg as usize] = value;
    }

    fn int(&self, reg: Reg, at: usize) -> Result<i64, Trap> {
        match self.registers[reg as usize] {
            Value::Int(value) => Ok(value),
            ref other => Err(self.mismatch("Int", other, at)),
        }
    }

    fn float(&self, reg: Reg, at: usize) -> Result<f64, Trap> {
        match self.registers[reg as usize] {
            Value::Float(value) => Ok(value),
            ref other => Err(self.mismatch("Float", other, at)),
        }
    }

    fn bool(&self, reg: Reg, at: usize) -> Result<bool, Trap> {
        match self.registers[reg as usize] {
            Value::Bool(value) => Ok(value),
            ref other => Err(self.mismatch("Bool", other, at)),
        }
    }

    fn text(&self, reg: Reg, at: usize) -> Result<&str, Trap> {
        match &self.registers[reg as usize] {
            Value::Text(text) => Ok(text),
            other => Err(self.mismatch("Text", other, at)),
        }
    }

    fn bug(&self, kind: BugKind, at: usize) -> Trap {
        Trap::Bug { kind, span: self.chunk.spans[at] }
    }

    fn mismatch(&self, expected: &str, found: &Value, at: usize) -> Trap {
        Trap::Internal {
            message: format!(
                "the virtual machine expected {expected} but found {} in `{:?}`",
                found.type_name(),
                self.chunk.code[at]
            ),
            span: self.chunk.spans[at],
        }
    }

    fn alert(&mut self, kind: AlertKind, at: usize) {
        if self.alerted.insert(at) {
            // What the program wrote so far comes before the alert.
            let _ = self.out.flush();
            (self.on_alert)(Alert { kind, span: self.chunk.spans[at] });
        }
    }
}
