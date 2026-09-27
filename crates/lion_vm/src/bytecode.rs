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

    /// Int operations with a constant right operand, which fits in 32 bits (never 0
    /// for `div` and `mod`).
    AddIntImm { dst: Reg, a: Reg, imm: i32 },
    SubIntImm { dst: Reg, a: Reg, imm: i32 },
    MulIntImm { dst: Reg, a: Reg, imm: i32 },
    DivIntImm { dst: Reg, a: Reg, imm: i32 },
    ModIntImm { dst: Reg, a: Reg, imm: i32 },

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
    /// Compares two Int registers and jumps when the comparison is false: the test of
    /// an `if` or a `while`, in one instruction.
    JumpUnlessInt { cmp: Cmp, a: Reg, b: Reg, target: u32 },
    /// The same, with a constant right operand.
    JumpUnlessIntImm { cmp: Cmp, a: Reg, imm: i32, target: u32 },
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

    /// `a..b` from two Int registers.
    MakeRange { dst: Reg, a: Reg, b: Reg },
    /// Whether the Int in `a` is in the Range in `b`.
    InRange { dst: Reg, a: Reg, b: Reg },
    /// Starts going through the Range in `range`: jumps to `target` when it is empty,
    /// else puts its first value in `counter`.
    ForRange { range: Reg, counter: Reg, target: u32 },
    /// Goes to the next value of the Range: jumps back to `target` with `counter`
    /// increased, unless it was the last value. Never overflows.
    NextRange { range: Reg, counter: Reg, target: u32 },

    MakeList { dst: Reg, start: Reg, count: u32 },
    /// An element of a List, or a character of a Text, from 1 (§16.2).
    GetIndex { dst: Reg, object: Reg, index: Reg },
    /// `l[a..b]` of a List or a Text (D41).
    GetSlice { dst: Reg, object: Reg, range: Reg },
    /// The size of a List, a Text or a Range.
    GetSize { dst: Reg, object: Reg },
    GetFirst { dst: Reg, list: Reg },
    GetLast { dst: Reg, list: Reg },
    /// Whether the value in `a` is an element of the List in `b`.
    InList { dst: Reg, a: Reg, b: Reg },
    /// Equality of content of any two values of the same type.
    EqValue { dst: Reg, a: Reg, b: Reg },
    NeValue { dst: Reg, a: Reg, b: Reg },
    /// Starts going through a List: jumps to `target` when it is empty.
    ForList { list: Reg, counter: Reg, target: u32 },
    /// The element at the position in `counter`, counted from 0.
    ElementAt { dst: Reg, list: Reg, counter: Reg },
    /// Jumps back to `target` with the next position, unless it was the last.
    NextList { list: Reg, counter: Reg, target: u32 },
    /// Replaces the part reached from `target` through the `depth` steps in registers
    /// `indices ..`: an Int is an index from 1 in a List, or the position of a field,
    /// from 0, in a structure.
    StoreElement { target: Target, indices: Reg, depth: u32, src: Reg },
    /// Adds at the end of the list reached from `target` through the indices.
    AddElement { target: Target, indices: Reg, depth: u32, src: Reg },
    SumInt { dst: Reg, values: Reg },
    SumFloat { dst: Reg, values: Reg },

    /// Whether the kind of the value in `src` is one of the bits of `kinds`.
    TypeTest { dst: Reg, src: Reg, kinds: u16 },
    /// Whether the kind of the value in `src` is one of the bits of `kinds`, or the value
    /// is a structure of one of the layouts in `Program::layout_sets[set]`.
    TypeTestStruct { dst: Reg, src: Reg, kinds: u16, set: u32 },
    /// The value in `src`, unless it is an Error: then the function returns it, or the
    /// script stops (§18.3).
    Try { dst: Reg, src: Reg },
    NewError { dst: Reg, a: Reg },
    ErrorMessage { dst: Reg, a: Reg },
    TextToInt { dst: Reg, a: Reg },
    TextToFloat { dst: Reg, a: Reg },

    /// A value of the structure `layout`, from the values of its fields in the `count`
    /// registers from `start` (§12.2).
    MakeStruct { dst: Reg, layout: u32, start: Reg, count: u32 },
    /// A field of a structure, by position.
    GetField { dst: Reg, object: Reg, field: u32 },
    /// The text of any value as a literal: a Text between quotes (C24).
    Literal { dst: Reg, a: Reg },
    /// Stops with a bug: the structure named by the Text in `name` breaks one of its
    /// invariants, as the Text in `detail` says (§12.3, D40).
    Broken { name: Reg, detail: Reg },

    Show { src: Reg },
    Halt,
}

/// A comparison of two Int values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl Cmp {
    #[inline]
    pub fn holds(self, a: i64, b: i64) -> bool {
        match self {
            Cmp::Eq => a == b,
            Cmp::Ne => a != b,
            Cmp::Lt => a < b,
            Cmp::Le => a <= b,
            Cmp::Gt => a > b,
            Cmp::Ge => a >= b,
        }
    }
}

/// The variable in which a change happens.
#[derive(Clone, Copy, Debug)]
pub enum Target {
    Register(Reg),
    Global(u32),
    /// The variable designated by the reference in a register (a `var` parameter).
    Reference(Reg),
}

/// A compiled program: one chunk per function.
pub struct Program {
    pub functions: Vec<Chunk>,
    /// The structures, by layout index.
    pub layouts: Vec<Rc<Layout>>,
    /// Sets of layout indices, for the type tests that tell structures apart.
    pub layout_sets: Vec<Vec<u32>>,
    /// The script, which runs first.
    pub main: usize,
}

/// What a value of a structure needs to be shown and compared (§12).
#[derive(Debug)]
pub struct Layout {
    pub index: u32,
    pub name: String,
    pub fields: Vec<String>,
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
