//! Translation of the typed IR into bytecode.
//!
//! Every local of a function has its own register; temporaries are allocated above
//! them like a stack and released as soon as the instruction that reads them is
//! emitted. The globals are the registers of the script, whose frame is at the bottom
//! of the stack.

use std::collections::HashMap;
use std::rc::Rc;

use lion_diagnostics::Span;
use lion_ir::{self as ir, BinaryOp, Builtin, Conversion, ExprKind, UnaryOp};

use crate::bytecode::{Chunk, Instr, Program, Reg};

pub fn compile(program: &ir::Program) -> Program {
    let functions = program
        .functions
        .iter()
        .enumerate()
        .map(|(index, function)| compile_function(function, index == program.main.index()))
        .collect();
    Program { functions, main: program.main.index() }
}

fn compile_function(function: &ir::Function, is_script: bool) -> Chunk {
    let locals = function.locals.len() as u32;
    let mut compiler = Compiler {
        function,
        is_script,
        code: Vec::new(),
        spans: Vec::new(),
        texts: Vec::new(),
        text_indices: HashMap::new(),
        next_temp: locals,
        registers: locals,
        loops: Vec::new(),
    };
    // Omitted arguments take their default value, in the order of the parameters (§11.2).
    for (index, value) in &function.defaults {
        let skip = compiler.code.len();
        compiler.emit(Instr::JumpIfArgs { count: index + 1, target: 0 }, None);
        compiler.expr_into(value, *index);
        compiler.patch(skip);
    }
    compiler.block(&function.body);
    compiler.emit(if is_script { Instr::Halt } else { Instr::ReturnNone }, None);
    Chunk {
        name: function.name.clone(),
        code: compiler.code,
        spans: compiler.spans,
        texts: compiler.texts,
        registers: compiler.registers,
    }
}

struct Compiler<'f> {
    function: &'f ir::Function,
    is_script: bool,
    code: Vec<Instr>,
    spans: Vec<Option<Span>>,
    texts: Vec<Rc<String>>,
    text_indices: HashMap<String, u32>,
    /// The first free temporary register.
    next_temp: Reg,
    /// The number of registers used so far.
    registers: u32,
    /// The loops being compiled, innermost last.
    loops: Vec<Loop>,
}

struct Loop {
    /// Where each turn starts, with the test of the condition.
    head: u32,
    /// The jumps of `break`, to point at the end of the loop.
    breaks: Vec<usize>,
}

impl Compiler<'_> {
    fn stmt(&mut self, stmt: &ir::Stmt) {
        let mark = self.next_temp;
        match stmt {
            ir::Stmt::Assign { place, value } => self.assign(*place, value),
            ir::Stmt::Expr(expr) => self.effect(expr),
            ir::Stmt::If { cond, then, otherwise } => {
                let to_otherwise = self.jump_unless(cond);
                self.block(then);
                if otherwise.is_empty() {
                    self.patch(to_otherwise);
                } else {
                    let to_end = self.jump();
                    self.patch(to_otherwise);
                    self.block(otherwise);
                    self.patch(to_end);
                }
            }
            ir::Stmt::While { cond, body } => {
                let head = self.code.len() as u32;
                let to_end = self.jump_unless(cond);
                self.loops.push(Loop { head, breaks: Vec::new() });
                self.block(body);
                self.emit(Instr::Jump { target: head }, None);
                let finished = self.loops.pop().expect("the loop is open");
                self.patch(to_end);
                for at in finished.breaks {
                    self.patch(at);
                }
            }
            ir::Stmt::Break => {
                let at = self.jump();
                self.loops.last_mut().expect("`break` is inside a loop").breaks.push(at);
            }
            ir::Stmt::Continue => {
                let head = self.loops.last().expect("`continue` is inside a loop").head;
                self.emit(Instr::Jump { target: head }, None);
            }
            ir::Stmt::Return(Some(value)) => {
                let src = self.operand(value);
                self.emit(Instr::Return { src }, Some(value.span));
            }
            ir::Stmt::Return(None) => {
                self.emit(if self.is_script { Instr::Halt } else { Instr::ReturnNone }, None);
            }
        }
        self.next_temp = mark;
    }

    fn assign(&mut self, place: ir::Place, value: &ir::Expr) {
        let span = Some(value.span);
        match place {
            ir::Place::Local(local) if !self.by_reference(local) => {
                let dst = register(local);
                if writes_destination_early(value) {
                    // `x = y and x` must read the old `x` after `y` is computed.
                    let temp = self.temp();
                    self.expr_into(value, temp);
                    self.emit(Instr::Move { dst, src: temp }, span);
                } else {
                    self.expr_into(value, dst);
                }
            }
            ir::Place::Local(local) => {
                let src = self.operand(value);
                self.emit(Instr::StoreRef { reference: register(local), src }, span);
            }
            ir::Place::Global(global) => {
                let src = self.operand(value);
                self.emit(Instr::StoreGlobal { global: global.0, src }, span);
            }
        }
    }

    fn block(&mut self, stmts: &[ir::Stmt]) {
        for stmt in stmts {
            self.stmt(stmt);
        }
    }

    /// Evaluates a condition and jumps when it is false; returns the jump to patch.
    fn jump_unless(&mut self, cond: &ir::Expr) -> usize {
        let mark = self.next_temp;
        let reg = self.operand(cond);
        let at = self.code.len();
        self.emit(Instr::JumpIfFalse { cond: reg, target: 0 }, Some(cond.span));
        self.next_temp = mark;
        at
    }

    /// An unconditional jump, to patch.
    fn jump(&mut self) -> usize {
        let at = self.code.len();
        self.emit(Instr::Jump { target: 0 }, None);
        at
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
            ExprKind::Local(local) if self.by_reference(*local) => {
                self.emit(Instr::LoadRef { dst, reference: register(*local) }, span);
            }
            ExprKind::Local(local) => {
                let src = register(*local);
                if src != dst {
                    self.emit(Instr::Move { dst, src }, span);
                }
            }
            ExprKind::Global(global) => self.emit(Instr::LoadGlobal { dst, global: global.0 }, span),
            ExprKind::Call { function, args } => {
                // The arguments go to consecutive registers, evaluated left to right (§9.2).
                let start = self.next_temp;
                for _ in args {
                    self.temp();
                }
                for (offset, arg) in args.iter().enumerate() {
                    let at = start + offset as u32;
                    match arg {
                        ir::Arg::Value(value) => self.expr_into(value, at),
                        ir::Arg::Reference(ir::Place::Local(local)) if self.by_reference(*local) => {
                            self.emit(Instr::Move { dst: at, src: register(*local) }, span);
                        }
                        ir::Arg::Reference(ir::Place::Local(local)) => {
                            self.emit(Instr::RefLocal { dst: at, src: register(*local) }, span);
                        }
                        ir::Arg::Reference(ir::Place::Global(global)) => {
                            self.emit(Instr::RefGlobal { dst: at, global: global.0 }, span);
                        }
                    }
                }
                let count = args.len() as u32;
                self.emit(Instr::Call { function: function.0, dst, args: start, count }, span);
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
                self.operand_before(lhs, rhs);
                self.operand(rhs);
                self.emit(Instr::LoadBool { dst, value: *op == BinaryOp::EqNone }, span);
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let a = self.operand_before(lhs, rhs);
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
            ExprKind::If { cond, then, otherwise } => {
                let to_otherwise = self.jump_unless(cond);
                self.expr_into(then, dst);
                let to_end = self.jump();
                self.patch(to_otherwise);
                self.expr_into(otherwise, dst);
                self.patch(to_end);
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
        if let ExprKind::Local(local) = expr.kind
            && !self.by_reference(local)
        {
            return register(local);
        }
        let temp = self.temp();
        self.expr_into(expr, temp);
        temp
    }

    /// Like [`Compiler::operand`], for an operand evaluated before `later`: if `later`
    /// calls a function, which may change the variable, its current value is copied, so
    /// that operands keep the values they had from left to right (§9.2).
    fn operand_before(&mut self, expr: &ir::Expr, later: &ir::Expr) -> Reg {
        if matches!(expr.kind, ExprKind::Local(_)) && calls_function(later) {
            let temp = self.temp();
            self.expr_into(expr, temp);
            return temp;
        }
        self.operand(expr)
    }

    fn by_reference(&self, local: ir::LocalId) -> bool {
        self.function.local(local).by_reference
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
        self.texts.push(Rc::new(text.to_string()));
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
            Instr::Jump { target }
            | Instr::JumpIfFalse { target, .. }
            | Instr::JumpIfTrue { target, .. }
            | Instr::JumpIfArgs { target, .. } => *target = here,
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
        ExprKind::If { then, otherwise, .. } => {
            writes_destination_early(then) || writes_destination_early(otherwise)
        }
        _ => false,
    }
}

/// Whether evaluating `expr` may call a function of the program.
fn calls_function(expr: &ir::Expr) -> bool {
    match &expr.kind {
        ExprKind::Call { .. } => true,
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Text(_)
        | ExprKind::None
        | ExprKind::Local(_)
        | ExprKind::Global(_) => false,
        ExprKind::Let { value, body, .. } => calls_function(value) || calls_function(body),
        ExprKind::Unary { operand, .. } => calls_function(operand),
        ExprKind::Convert { value, .. } => calls_function(value),
        ExprKind::Binary { lhs, rhs, .. } | ExprKind::And { lhs, rhs } | ExprKind::Or { lhs, rhs } => {
            calls_function(lhs) || calls_function(rhs)
        }
        ExprKind::If { cond, then, otherwise } => {
            calls_function(cond) || calls_function(then) || calls_function(otherwise)
        }
        ExprKind::Concat(parts) => parts.iter().any(calls_function),
        ExprKind::CallBuiltin { args, .. } => args.iter().any(calls_function),
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
