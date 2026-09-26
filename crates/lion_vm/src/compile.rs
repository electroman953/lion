//! Translation of the typed IR into bytecode.
//!
//! Every local of the program has its own register; temporaries are allocated above
//! them like a stack and released as soon as the instruction that reads them is
//! emitted.

use std::collections::HashMap;
use std::rc::Rc;

use lion_diagnostics::Span;
use lion_ir::{self as ir, BinaryOp, Builtin, Conversion, ExprKind, UnaryOp};

use crate::bytecode::{Chunk, Instr, Reg};

pub fn compile(program: &ir::Program) -> Chunk {
    let locals = program.locals.len() as u32;
    let mut compiler = Compiler {
        code: Vec::new(),
        spans: Vec::new(),
        texts: Vec::new(),
        text_indices: HashMap::new(),
        next_temp: locals,
        registers: locals,
    };
    for stmt in &program.body {
        compiler.stmt(stmt);
    }
    compiler.emit(Instr::Halt, None);
    Chunk { code: compiler.code, spans: compiler.spans, texts: compiler.texts, registers: compiler.registers }
}

struct Compiler {
    code: Vec<Instr>,
    spans: Vec<Option<Span>>,
    texts: Vec<Rc<str>>,
    text_indices: HashMap<String, u32>,
    /// The first free temporary register.
    next_temp: Reg,
    /// The number of registers used so far.
    registers: u32,
}

impl Compiler {
    fn stmt(&mut self, stmt: &ir::Stmt) {
        let mark = self.next_temp;
        match stmt {
            ir::Stmt::Assign { local, value } => {
                let dst = register(*local);
                if writes_destination_early(value) {
                    // `x = y and x` must read the old `x` after `y` is computed.
                    let temp = self.temp();
                    self.expr_into(value, temp);
                    self.emit(Instr::Move { dst, src: temp }, Some(value.span));
                } else {
                    self.expr_into(value, dst);
                }
            }
            ir::Stmt::Expr(expr) => self.effect(expr),
        }
        self.next_temp = mark;
    }

    /// Evaluates an expression whose value is not used.
    fn effect(&mut self, expr: &ir::Expr) {
        match &expr.kind {
            ExprKind::CallBuiltin { builtin: Builtin::Show, args } => {
                let src = self.operand(&args[0]);
                self.emit(Instr::Show { src }, Some(expr.span));
            }
            _ => {
                let temp = self.temp();
                self.expr_into(expr, temp);
            }
        }
    }

    fn expr_into(&mut self, expr: &ir::Expr, dst: Reg) {
        let span = Some(expr.span);
        let mark = self.next_temp;
        match &expr.kind {
            ExprKind::Int(value) => self.emit(Instr::LoadInt { dst, value: *value }, span),
            ExprKind::Float(value) => self.emit(Instr::LoadFloat { dst, value: *value }, span),
            ExprKind::Bool(value) => self.emit(Instr::LoadBool { dst, value: *value }, span),
            ExprKind::None => self.emit(Instr::LoadNone { dst }, span),
            ExprKind::Text(text) => {
                let index = self.text(text);
                self.emit(Instr::LoadText { dst, index }, span);
            }
            ExprKind::Local(local) => {
                let src = register(*local);
                if src != dst {
                    self.emit(Instr::Move { dst, src }, span);
                }
            }
            ExprKind::Let { local, value, body } => {
                self.expr_into(value, register(*local));
                self.expr_into(body, dst);
            }
            ExprKind::Unary { op, operand } => {
                let a = self.operand(operand);
                let instr = match op {
                    UnaryOp::NegInt => Instr::NegInt { dst, a },
                    UnaryOp::NegFloat => Instr::NegFloat { dst, a },
                    UnaryOp::Not => Instr::Not { dst, a },
                };
                self.emit(instr, span);
            }
            ExprKind::Binary { op: op @ (BinaryOp::EqNone | BinaryOp::NeNone), lhs, rhs } => {
                // `none` equals `none`; the operands are still evaluated for their effects.
                self.operand(lhs);
                self.operand(rhs);
                self.emit(Instr::LoadBool { dst, value: *op == BinaryOp::EqNone }, span);
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let a = self.operand(lhs);
                let b = self.operand(rhs);
                self.emit(binary_instr(*op, dst, a, b), span);
            }
            ExprKind::And { lhs, rhs } | ExprKind::Or { lhs, rhs } => {
                self.expr_into(lhs, dst);
                let jump = match expr.kind {
                    ExprKind::And { .. } => Instr::JumpIfFalse { cond: dst, target: 0 },
                    _ => Instr::JumpIfTrue { cond: dst, target: 0 },
                };
                let at = self.code.len();
                self.emit(jump, span);
                self.expr_into(rhs, dst);
                self.patch(at);
            }
            ExprKind::Convert { conversion, value } => {
                let a = self.operand(value);
                let instr = match conversion {
                    Conversion::IntToFloat => Instr::IntToFloat { dst, a },
                    Conversion::FloatToInt => Instr::FloatToInt { dst, a },
                    Conversion::ToText => Instr::ToText { dst, a },
                };
                self.emit(instr, span);
            }
            ExprKind::Concat(parts) => {
                let start = self.next_temp;
                for _ in parts {
                    self.temp();
                }
                for (offset, part) in parts.iter().enumerate() {
                    self.expr_into(part, start + offset as u32);
                }
                self.emit(Instr::Concat { dst, start, count: parts.len() as u32 }, span);
            }
            ExprKind::CallBuiltin { builtin: Builtin::Show, args } => {
                let src = self.operand(&args[0]);
                self.emit(Instr::Show { src }, span);
                self.emit(Instr::LoadNone { dst }, span);
            }
        }
        self.next_temp = mark;
    }

    /// A register holding the value of `expr`: the register of a local, or a new
    /// temporary, which stays reserved until the caller releases its temporaries.
    fn operand(&mut self, expr: &ir::Expr) -> Reg {
        if let ExprKind::Local(local) = expr.kind {
            return register(local);
        }
        let temp = self.temp();
        self.expr_into(expr, temp);
        temp
    }

    fn temp(&mut self) -> Reg {
        let reg = self.next_temp;
        self.next_temp += 1;
        self.registers = self.registers.max(self.next_temp);
        reg
    }

    fn text(&mut self, text: &str) -> u32 {
        if let Some(&index) = self.text_indices.get(text) {
            return index;
        }
        let index = self.texts.len() as u32;
        self.texts.push(Rc::from(text));
        self.text_indices.insert(text.to_string(), index);
        index
    }

    fn emit(&mut self, instr: Instr, span: Option<Span>) {
        self.code.push(instr);
        self.spans.push(span);
    }

    /// Points the jump at `at` to the next instruction.
    fn patch(&mut self, at: usize) {
        let here = self.code.len() as u32;
        match &mut self.code[at] {
            Instr::Jump { target } | Instr::JumpIfFalse { target, .. } | Instr::JumpIfTrue { target, .. } => {
                *target = here;
            }
            other => unreachable!("not a jump: {other:?}"),
        }
    }
}

fn register(local: ir::LocalId) -> Reg {
    local.0
}

/// Whether compiling `expr` into a register writes that register before it has
/// finished reading the operands.
fn writes_destination_early(expr: &ir::Expr) -> bool {
    match &expr.kind {
        ExprKind::And { .. } | ExprKind::Or { .. } => true,
        ExprKind::Let { body, .. } => writes_destination_early(body),
        _ => false,
    }
}

fn binary_instr(op: BinaryOp, dst: Reg, a: Reg, b: Reg) -> Instr {
    match op {
        BinaryOp::AddInt => Instr::AddInt { dst, a, b },
        BinaryOp::SubInt => Instr::SubInt { dst, a, b },
        BinaryOp::MulInt => Instr::MulInt { dst, a, b },
        BinaryOp::DivInt => Instr::DivInt { dst, a, b },
        BinaryOp::ModInt => Instr::ModInt { dst, a, b },
        BinaryOp::PowInt => Instr::PowInt { dst, a, b },
        BinaryOp::AddFloat => Instr::AddFloat { dst, a, b },
        BinaryOp::SubFloat => Instr::SubFloat { dst, a, b },
        BinaryOp::MulFloat => Instr::MulFloat { dst, a, b },
        BinaryOp::DivFloat => Instr::DivFloat { dst, a, b },
        BinaryOp::PowFloat => Instr::PowFloat { dst, a, b },
        BinaryOp::EqInt => Instr::EqInt { dst, a, b },
        BinaryOp::NeInt => Instr::NeInt { dst, a, b },
        BinaryOp::LtInt => Instr::LtInt { dst, a, b },
        BinaryOp::LeInt => Instr::LeInt { dst, a, b },
        BinaryOp::GtInt => Instr::GtInt { dst, a, b },
        BinaryOp::GeInt => Instr::GeInt { dst, a, b },
        BinaryOp::EqFloat => Instr::EqFloat { dst, a, b },
        BinaryOp::NeFloat => Instr::NeFloat { dst, a, b },
        BinaryOp::LtFloat => Instr::LtFloat { dst, a, b },
        BinaryOp::LeFloat => Instr::LeFloat { dst, a, b },
        BinaryOp::GtFloat => Instr::GtFloat { dst, a, b },
        BinaryOp::GeFloat => Instr::GeFloat { dst, a, b },
        BinaryOp::EqBool => Instr::EqBool { dst, a, b },
        BinaryOp::NeBool => Instr::NeBool { dst, a, b },
        BinaryOp::EqText => Instr::EqText { dst, a, b },
        BinaryOp::NeText => Instr::NeText { dst, a, b },
        BinaryOp::EqNone | BinaryOp::NeNone => unreachable!("compiled to a constant"),
    }
}
