use std::fmt::Write;
use std::rc::Rc;

use lion_diagnostics::Span;

/// A register index in the current frame.
pub type Reg = u32;

/// One instruction. Operands are registers; `dst` receives the result. Each
/// arithmetic instruction is specialised to its operand types.
#[derive(Clone, Copy, Debug)]
#[rustfmt::skip]
pub enum Instr {
    LoadInt { dst: Reg, value: i64 },
    LoadFloat { dst: Reg, value: f64 },
    LoadBool { dst: Reg, value: bool },
    LoadNone { dst: Reg },
    LoadText { dst: Reg, index: u32 },
    Move { dst: Reg, src: Reg },

    AddInt { dst: Reg, a: Reg, b: Reg },
    SubInt { dst: Reg, a: Reg, b: Reg },
    MulInt { dst: Reg, a: Reg, b: Reg },
    DivInt { dst: Reg, a: Reg, b: Reg },
    ModInt { dst: Reg, a: Reg, b: Reg },
    PowInt { dst: Reg, a: Reg, b: Reg },
    NegInt { dst: Reg, a: Reg },

    AddFloat { dst: Reg, a: Reg, b: Reg },
    SubFloat { dst: Reg, a: Reg, b: Reg },
    MulFloat { dst: Reg, a: Reg, b: Reg },
    DivFloat { dst: Reg, a: Reg, b: Reg },
    PowFloat { dst: Reg, a: Reg, b: Reg },
    NegFloat { dst: Reg, a: Reg },

    EqInt { dst: Reg, a: Reg, b: Reg },
    NeInt { dst: Reg, a: Reg, b: Reg },
    LtInt { dst: Reg, a: Reg, b: Reg },
    LeInt { dst: Reg, a: Reg, b: Reg },
    GtInt { dst: Reg, a: Reg, b: Reg },
    GeInt { dst: Reg, a: Reg, b: Reg },
    EqFloat { dst: Reg, a: Reg, b: Reg },
    NeFloat { dst: Reg, a: Reg, b: Reg },
    LtFloat { dst: Reg, a: Reg, b: Reg },
    LeFloat { dst: Reg, a: Reg, b: Reg },
    GtFloat { dst: Reg, a: Reg, b: Reg },
    GeFloat { dst: Reg, a: Reg, b: Reg },
    EqBool { dst: Reg, a: Reg, b: Reg },
    NeBool { dst: Reg, a: Reg, b: Reg },
    EqText { dst: Reg, a: Reg, b: Reg },
    NeText { dst: Reg, a: Reg, b: Reg },
    Not { dst: Reg, a: Reg },

    IntToFloat { dst: Reg, a: Reg },
    FloatToInt { dst: Reg, a: Reg },
    ToText { dst: Reg, a: Reg },
    /// Joins the Texts in registers `start .. start + count`.
    Concat { dst: Reg, start: Reg, count: u32 },

    Jump { target: u32 },
    JumpIfFalse { cond: Reg, target: u32 },
    JumpIfTrue { cond: Reg, target: u32 },

    /// Calls a function with the `count` values of registers `args ..`; its result goes
    /// to `dst`.
    Call { function: u32, dst: Reg, args: Reg, count: u32 },
    Return { src: Reg },
    ReturnNone,
    /// Jumps when the call gave at least `count` arguments (default values, §11.2).
    JumpIfArgs { count: u32, target: u32 },
    /// Globals are the registers of the script's frame, at the bottom of the stack.
    LoadGlobal { dst: Reg, global: u32 },
    StoreGlobal { global: u32, src: Reg },
    /// References, for `var` parameters (§11.2).
    RefLocal { dst: Reg, src: Reg },
    RefGlobal { dst: Reg, global: u32 },
    LoadRef { dst: Reg, reference: Reg },
    StoreRef { reference: Reg, src: Reg },

    Show { src: Reg },
    Halt,
}

/// A compiled program: one chunk per function.
pub struct Program {
    pub functions: Vec<Chunk>,
    /// The script, which runs first.
    pub main: usize,
}

/// Compiled code for one function.
pub struct Chunk {
    pub name: String,
    pub code: Vec<Instr>,
    /// The source span of each instruction, for bug and alert reports.
    pub spans: Vec<Option<Span>>,
    pub texts: Vec<Rc<String>>,
    /// The number of registers the frame needs.
    pub registers: u32,
}

/// A readable listing, for `lion debug bytecode`.
pub fn disassemble(program: &Program) -> String {
    let mut out = String::new();
    for (index, chunk) in program.functions.iter().enumerate() {
        out.push_str(&format!("function {index} {}\n", chunk.name));
        out.push_str(&disassemble_chunk(chunk));
    }
    out
}

fn disassemble_chunk(chunk: &Chunk) -> String {
    let mut out = format!("registers {}\n", chunk.registers);
    if !chunk.texts.is_empty() {
        out.push_str("texts\n");
        for (index, text) in chunk.texts.iter().enumerate() {
            let _ = writeln!(out, "  {index} {text:?}");
        }
    }
    out.push_str("code\n");
    for (index, instr) in chunk.code.iter().enumerate() {
        let _ = writeln!(out, "  {index:04} {instr:?}");
    }
    out
}
