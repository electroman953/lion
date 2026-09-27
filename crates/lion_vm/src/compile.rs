//! Translation of the typed IR into bytecode.
//!
//! Every local of a function has its own register; temporaries are allocated above
//! them like a stack and released as soon as the instruction that reads them is
//! emitted. The globals are the registers of the script, whose frame is at the bottom
//! of the stack.

use std::collections::HashMap;
use std::sync::Arc;

use lion_diagnostics::Span;
use lion_ir::{self as ir, BinaryOp, Builtin, Conversion, ExprKind, UnaryOp};

use crate::bytecode::{Chunk, Cmp, EnumLayout, Instr, Layout, Program, Reg, Target};

pub fn compile(program: &ir::Program) -> Program {
    let layouts: Vec<Arc<Layout>> = program
        .structs
        .iter()
        .enumerate()
        .map(|(index, def)| {
            Arc::new(Layout {
                index: index as u32,
                name: def.name.clone(),
                fields: def.fields.iter().map(|(name, _)| name.clone()).collect(),
                equals: def.equals.map(|function| function.0),
            })
        })
        .collect();
    let first_enum = program.structs.len();
    let enums: Vec<Arc<EnumLayout>> = program
        .enums
        .iter()
        .enumerate()
        .map(|(index, enumeration)| {
            Arc::new(EnumLayout {
                index: (first_enum + index) as u32,
                name: enumeration.name(),
                values: enumeration.values(),
            })
        })
        .collect();
    let mut shared = Shared {
        layouts: program.structs.iter().enumerate().map(|(index, def)| (def.id, index as u32)).collect(),
        enums: program
            .enums
            .iter()
            .enumerate()
            .map(|(index, enumeration)| (*enumeration, index as u32))
            .collect(),
        first_enum: first_enum as u32,
        type_sets: Vec::new(),
    };
    let functions = program
        .functions
        .iter()
        .enumerate()
        .map(|(index, function)| compile_function(function, index == program.main.index(), &mut shared))
        .collect();
    let module_inits = program.module_inits.iter().map(|init| init.map(|function| function.0)).collect();
    // Before a test, the declarations of the globals of the script run, and nothing else.
    let main = program.function(program.main);
    let declarations = (!program.tests.is_empty()).then(|| {
        let body = main
            .body
            .iter()
            .zip(&program.declarations)
            .filter(|(_, declaration)| **declaration)
            .map(|(stmt, _)| stmt.clone())
            .collect();
        let script = ir::Function {
            name: "script declarations".to_string(),
            params: 0,
            captures: 0,
            defaults: Vec::new(),
            ret: main.ret,
            locals: main.locals.clone(),
            body,
            span: None,
        };
        compile_function(&script, true, &mut shared)
    });
    let custom_equality = layouts.iter().any(|layout| layout.equals.is_some());
    Program {
        custom_equality,
        foreign: program.foreign.clone(),
        functions,
        layouts,
        enums,
        type_sets: shared.type_sets,
        main: program.main.index(),
        module_inits,
        tests: program.tests.iter().map(|(name, function)| (name.clone(), function.0)).collect(),
        declarations,
    }
}

/// What the functions of a program share while they are compiled.
struct Shared {
    /// The layout index of each structure.
    layouts: HashMap<ir::StructRef, u32>,
    /// The index of each enumeration in `Program::enums`.
    enums: HashMap<ir::EnumRef, u32>,
    /// The type number of the first enumeration.
    first_enum: u32,
    type_sets: Vec<Vec<u32>>,
}

fn compile_function(function: &ir::Function, is_script: bool, shared: &mut Shared) -> Chunk {
    let locals = function.locals.len() as u32;
    let mut compiler = Compiler {
        function,
        shared,
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
    // Where each statement starts, so that the interactive mode runs only the new ones.
    let mut starts = Vec::new();
    for stmt in &function.body {
        starts.push(compiler.code.len() as u32);
        compiler.stmt(stmt);
    }
    compiler.emit(if is_script { Instr::Halt } else { Instr::ReturnNone }, None);
    Chunk {
        starts,
        label: Arc::from(function.name.as_str()),
        params: function.params,
        name: function.name.clone(),
        code: compiler.code,
        spans: compiler.spans,
        texts: compiler.texts,
        registers: compiler.registers,
    }
}

struct Compiler<'f> {
    function: &'f ir::Function,
    shared: &'f mut Shared,
    is_script: bool,
    code: Vec<Instr>,
    spans: Vec<Option<Span>>,
    texts: Vec<Arc<String>>,
    text_indices: HashMap<String, u32>,
    /// The first free temporary register.
    next_temp: Reg,
    /// The number of registers used so far.
    registers: u32,
    /// The loops being compiled, innermost last.
    loops: Vec<Loop>,
}

struct Loop {
    /// The jumps of `continue`, to point at the next turn.
    continues: Vec<usize>,
    /// The jumps of `break`, to point at the end of the loop.
    breaks: Vec<usize>,
}

impl Compiler<'_> {
    fn stmt(&mut self, stmt: &ir::Stmt) {
        let mark = self.next_temp;
        match stmt {
            ir::Stmt::Assign { place, value } => self.assign(*place, value),
            ir::Stmt::Declare { local } if self.boxed(*local) => {
                self.emit(Instr::NewCell { dst: register(*local) }, None);
            }
            ir::Stmt::Declare { .. } => {}
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
                self.loops.push(Loop { continues: Vec::new(), breaks: Vec::new() });
                self.block(body);
                let finished = self.loops.pop().expect("the loop is open");
                for at in finished.continues {
                    self.patch_to(at, head);
                }
                self.emit(Instr::Jump { target: head }, None);
                self.patch(to_end);
                for at in finished.breaks {
                    self.patch(at);
                }
            }
            ir::Stmt::For { var, iterable, body } => match iterable.ty {
                ir::Type::Range => self.for_range(*var, iterable, body),
                _ => self.for_list(*var, iterable, body),
            },
            ir::Stmt::Seq(stmts) => self.block(stmts),
            ir::Stmt::InitModule { module } => {
                let dst = self.temp();
                self.emit(Instr::InitModule { module: *module, dst }, None);
            }
            ir::Stmt::AssignElement { root: ir::Place::Local(local), path, value } if self.boxed(*local) => {
                // The value leaves the cell while it changes, so that it is not copied.
                let cell = register(*local);
                let whole = self.temp();
                let (_, start, depth) = self.path(ir::Place::Local(*local), path);
                let src = self.operand(value);
                self.emit(Instr::TakeCell { dst: whole, cell }, None);
                let target = Target::Register(whole);
                self.emit(Instr::StoreElement { target, indices: start, depth, src }, Some(value.span));
                self.emit(Instr::StoreCell { cell, src: whole }, None);
            }
            ir::Stmt::Add { root: ir::Place::Local(local), path, value } if self.boxed(*local) => {
                let cell = register(*local);
                let whole = self.temp();
                let (_, start, depth) = self.path(ir::Place::Local(*local), path);
                let src = self.operand(value);
                self.emit(Instr::TakeCell { dst: whole, cell }, None);
                let target = Target::Register(whole);
                self.emit(Instr::AddElement { target, indices: start, depth, src }, Some(value.span));
                self.emit(Instr::StoreCell { cell, src: whole }, None);
            }
            ir::Stmt::AssignElement { root, path, value } => {
                let (target, start, depth) = self.path(*root, path);
                let src = self.operand(value);
                self.emit(Instr::StoreElement { target, indices: start, depth, src }, Some(value.span));
            }
            ir::Stmt::Add { root, path, value } => {
                let (target, start, depth) = self.path(*root, path);
                let src = self.operand(value);
                self.emit(Instr::AddElement { target, indices: start, depth, src }, Some(value.span));
            }
            ir::Stmt::Remove { root: ir::Place::Local(local), path, key } if self.boxed(*local) => {
                let cell = register(*local);
                let whole = self.temp();
                let (_, start, depth) = self.path(ir::Place::Local(*local), path);
                let src = self.operand(key);
                self.emit(Instr::TakeCell { dst: whole, cell }, None);
                let target = Target::Register(whole);
                self.emit(Instr::RemoveElement { target, indices: start, depth, src }, Some(key.span));
                self.emit(Instr::StoreCell { cell, src: whole }, None);
            }
            ir::Stmt::Remove { root, path, key } => {
                let (target, start, depth) = self.path(*root, path);
                let src = self.operand(key);
                self.emit(Instr::RemoveElement { target, indices: start, depth, src }, Some(key.span));
            }
            ir::Stmt::Break => {
                let at = self.jump();
                self.loops.last_mut().expect("`break` is inside a loop").breaks.push(at);
            }
            ir::Stmt::Continue => {
                let at = self.jump();
                self.loops.last_mut().expect("`continue` is inside a loop").continues.push(at);
            }
            ir::Stmt::Return(Some(value)) => self.return_value(value),
            ir::Stmt::Return(None) => {
                self.emit(if self.is_script { Instr::Halt } else { Instr::ReturnNone }, None);
            }
        }
        self.next_temp = mark;
    }

    fn assign(&mut self, place: ir::Place, value: &ir::Expr) {
        let span = Some(value.span);
        match place {
            // A variable shared with a nested function lives in a cell (§11.5).
            ir::Place::Local(local) if self.boxed(local) => {
                let src = self.operand(value);
                self.emit(Instr::StoreCell { cell: register(local), src }, span);
            }
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

    /// `return value`; `return if c then a else b` returns from each branch, without
    /// going through a register.
    fn return_value(&mut self, value: &ir::Expr) {
        if let ExprKind::If { cond, then, otherwise } = &value.kind {
            let to_otherwise = self.jump_unless(cond);
            self.return_value(then);
            self.patch(to_otherwise);
            self.return_value(otherwise);
            return;
        }
        let mark = self.next_temp;
        let src = self.operand(value);
        self.emit(Instr::Return { src }, Some(value.span));
        self.next_temp = mark;
    }

    /// `for var in a..b`: the Range and a counter live in temporaries during the loop.
    fn for_range(&mut self, var: ir::LocalId, iterable: &ir::Expr, body: &[ir::Stmt]) {
        let span = Some(iterable.span);
        let range = self.temp();
        self.expr_into(iterable, range);
        let counter = self.temp();
        let start = self.code.len();
        self.emit(Instr::ForRange { range, counter, target: 0 }, span);
        let head = self.code.len() as u32;
        self.emit(Instr::Move { dst: register(var), src: counter }, span);
        self.loops.push(Loop { continues: Vec::new(), breaks: Vec::new() });
        self.block(body);
        let finished = self.loops.pop().expect("the loop is open");
        let step = self.code.len() as u32;
        for at in finished.continues {
            self.patch_to(at, step);
        }
        self.emit(Instr::NextRange { range, counter, target: head }, span);
        self.patch(start);
        for at in finished.breaks {
            self.patch(at);
        }
    }

    /// `for var in list`: the list and a position live in temporaries during the loop.
    fn for_list(&mut self, var: ir::LocalId, iterable: &ir::Expr, body: &[ir::Stmt]) {
        let span = Some(iterable.span);
        let list = self.temp();
        self.expr_into(iterable, list);
        let counter = self.temp();
        let start = self.code.len();
        self.emit(Instr::ForList { list, counter, target: 0 }, span);
        let head = self.code.len() as u32;
        self.emit(Instr::ElementAt { dst: register(var), list, counter }, span);
        self.loops.push(Loop { continues: Vec::new(), breaks: Vec::new() });
        self.block(body);
        let finished = self.loops.pop().expect("the loop is open");
        let step = self.code.len() as u32;
        for at in finished.continues {
            self.patch_to(at, step);
        }
        self.emit(Instr::NextList { list, counter, target: head }, span);
        self.patch(start);
        for at in finished.breaks {
            self.patch(at);
        }
    }

    /// The variable of a change in place, and its steps evaluated into consecutive
    /// registers: the indices, and the positions of the fields.
    fn path(&mut self, root: ir::Place, path: &[ir::Step]) -> (Target, Reg, u32) {
        let target = match root {
            ir::Place::Local(local) if self.by_reference(local) => Target::Reference(register(local)),
            ir::Place::Local(local) => Target::Register(register(local)),
            ir::Place::Global(global) => Target::Global(global.0),
        };
        let start = self.next_temp;
        for _ in path {
            self.temp();
        }
        for (offset, step) in path.iter().enumerate() {
            let dst = start + offset as u32;
            match step {
                ir::Step::Index(index) => self.expr_into(index, dst),
                ir::Step::Field(field) => {
                    self.emit(Instr::LoadInt { dst, value: i64::from(*field) }, None);
                }
            }
        }
        (target, start, path.len() as u32)
    }

    fn block(&mut self, stmts: &[ir::Stmt]) {
        for stmt in stmts {
            self.stmt(stmt);
        }
    }

    /// Evaluates a condition and jumps when it is false; returns the jump to patch.
    fn jump_unless(&mut self, cond: &ir::Expr) -> usize {
        let mark = self.next_temp;
        // A comparison of Ints jumps directly, without a Bool in a register.
        let instr = match &cond.kind {
            ExprKind::Binary { op, lhs, rhs } if int_comparison(*op).is_some() => {
                let cmp = int_comparison(*op).expect("checked");
                let a = self.operand_before(lhs, rhs);
                match small_int(rhs) {
                    Some(imm) => Instr::JumpUnlessIntImm { cmp, a, imm, target: 0 },
                    None => {
                        let b = self.operand(rhs);
                        Instr::JumpUnlessInt { cmp, a, b, target: 0 }
                    }
                }
            }
            _ => {
                let reg = self.operand(cond);
                Instr::JumpIfFalse { cond: reg, target: 0 }
            }
        };
        let at = self.code.len();
        self.emit(instr, Some(cond.span));
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
            ExprKind::Local(local) if self.boxed(*local) => {
                self.emit(Instr::LoadCell { dst, cell: register(*local) }, span);
            }
            ExprKind::Task(value) => {
                let src = self.operand(value);
                self.emit(Instr::MakeTask { dst, src }, span);
            }
            ExprKind::Wait(value) => {
                let src = self.operand(value);
                self.emit(Instr::Wait { dst, src }, span);
            }
            ExprKind::Cell(local) => {
                let src = register(*local);
                if src != dst {
                    self.emit(Instr::Move { dst, src }, span);
                }
            }
            ExprKind::Closure { function, captures } => {
                let start = self.next_temp;
                for _ in captures {
                    self.temp();
                }
                for (offset, capture) in captures.iter().enumerate() {
                    self.expr_into(capture, start + offset as u32);
                }
                let count = captures.len() as u32;
                self.emit(Instr::MakeClosure { dst, function: function.0, start, count }, span);
            }
            // Replaced by its value before the program runs; computed here while it is
            // being evaluated (§21.1).
            ExprKind::Compile(value) => self.expr_into(value, dst),
            ExprKind::Partial { callee, args } => {
                let callee_reg = self.temp();
                self.expr_into(callee, callee_reg);
                let start = self.next_temp;
                for _ in args {
                    self.temp();
                }
                for (offset, arg) in args.iter().enumerate() {
                    self.expr_into(arg, start + offset as u32);
                }
                let count = args.len() as u32;
                self.emit(Instr::Bind { dst, callee: callee_reg, start, count }, span);
            }
            ExprKind::CallValue { callee, args } => {
                let callee_reg = self.temp();
                self.expr_into(callee, callee_reg);
                let start = self.next_temp;
                for _ in args {
                    self.temp();
                }
                for (offset, arg) in args.iter().enumerate() {
                    self.expr_into(arg, start + offset as u32);
                }
                let count = args.len() as u32;
                self.emit(Instr::CallValue { dst, callee: callee_reg, args: start, count }, span);
            }
            ExprKind::Local(local) => {
                let src = register(*local);
                if src != dst {
                    self.emit(Instr::Move { dst, src }, span);
                }
            }
            ExprKind::Global(global) => self.emit(Instr::LoadGlobal { dst, global: global.0 }, span),
            ExprKind::Call { function, args } => {
                // A variable in a cell goes to a `var` parameter through a copy, stored
                // back after the call (C50). The copy lies below the frame of the call.
                let copies: Vec<Option<Reg>> = args
                    .iter()
                    .map(|arg| match arg {
                        ir::Arg::Reference(ir::Place::Local(local))
                            if self.boxed(*local) && !self.by_reference(*local) =>
                        {
                            Some(self.temp())
                        }
                        _ => None,
                    })
                    .collect();
                let mut write_backs = Vec::new();
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
                        ir::Arg::Reference(ir::Place::Local(local)) if self.boxed(*local) => {
                            let copy = copies[offset].expect("allocated above");
                            self.emit(Instr::LoadCell { dst: copy, cell: register(*local) }, span);
                            self.emit(Instr::RefLocal { dst: at, src: copy }, span);
                            write_backs.push((copy, register(*local)));
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
                for (copy, cell) in write_backs {
                    self.emit(Instr::StoreCell { cell, src: copy }, None);
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
                    UnaryOp::NegRational => Instr::NegRational { dst, a },
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
            ExprKind::Binary { op, lhs, rhs }
                if small_int(rhs).and_then(|imm| immediate_instr(*op, dst, 0, imm)).is_some() =>
            {
                let imm = small_int(rhs).expect("checked");
                let a = self.operand(lhs);
                self.emit(immediate_instr(*op, dst, a, imm).expect("checked"), span);
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
                    Conversion::IntToRational => Instr::IntToRational { dst, a },
                    Conversion::RationalToFloat => Instr::RationalToFloat { dst, a },
                    Conversion::ToText => Instr::ToText { dst, a },
                    Conversion::TextToInt => Instr::TextToInt { dst, a },
                    Conversion::TextToFloat => Instr::TextToFloat { dst, a },
                    Conversion::Literal => Instr::Literal { dst, a },
                    Conversion::EnumPosition => Instr::EnumPosition { dst, a },
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
            ExprKind::Range { start, end } => {
                let a = self.operand_before(start, end);
                let b = self.operand(end);
                self.emit(Instr::MakeRange { dst, a, b }, span);
            }
            ExprKind::List(elements) => {
                let start = self.next_temp;
                for _ in elements {
                    self.temp();
                }
                for (offset, element) in elements.iter().enumerate() {
                    self.expr_into(element, start + offset as u32);
                }
                self.emit(Instr::MakeList { dst, start, count: elements.len() as u32 }, span);
            }
            ExprKind::Map(entries) => {
                let start = self.next_temp;
                for _ in 0..entries.len() * 2 {
                    self.temp();
                }
                for (offset, (key, value)) in entries.iter().enumerate() {
                    self.expr_into(key, start + 2 * offset as u32);
                    self.expr_into(value, start + 2 * offset as u32 + 1);
                }
                self.emit(Instr::MakeMap { dst, start, count: entries.len() as u32 }, span);
            }
            ExprKind::CallBuiltin { builtin: Builtin::MapGet, args } => {
                let map = self.operand_before(&args[0], &args[1]);
                let key = self.operand(&args[1]);
                self.emit(Instr::MapGet { dst, map, key }, span);
            }
            ExprKind::Set(elements) | ExprKind::Tuple(elements) => {
                let start = self.next_temp;
                for _ in elements {
                    self.temp();
                }
                for (offset, element) in elements.iter().enumerate() {
                    self.expr_into(element, start + offset as u32);
                }
                let count = elements.len() as u32;
                let instr = match expr.kind {
                    ExprKind::Set(_) => Instr::MakeSet { dst, start, count },
                    _ => Instr::MakeTuple { dst, start, count },
                };
                self.emit(instr, span);
            }
            ExprKind::Index { object, index } => {
                let object = self.operand_before(object, index);
                let index = self.operand(index);
                self.emit(Instr::GetIndex { dst, object, index }, span);
            }
            ExprKind::Slice { object, range } => {
                let object = self.operand_before(object, range);
                let range = self.operand(range);
                self.emit(Instr::GetSlice { dst, object, range }, span);
            }
            ExprKind::Property { object, property } => {
                let object = self.operand(object);
                let instr = match property {
                    ir::Property::Size => Instr::GetSize { dst, object },
                    ir::Property::First => Instr::GetFirst { dst, list: object },
                    ir::Property::Last => Instr::GetLast { dst, list: object },
                };
                self.emit(instr, span);
            }
            ExprKind::TypeTest { value, ty } => {
                let src = self.operand(value);
                let instr = self.type_test(value.ty, *ty, dst, src);
                self.emit(instr, span);
            }
            ExprKind::Try(value) => {
                let src = self.operand(value);
                // The errors of the program are structures or enumerations (§18.2).
                let result = expr.ty.members();
                let errors: Vec<u32> = self
                    .named_types(value.ty)
                    .into_iter()
                    .zip(
                        value
                            .ty
                            .members()
                            .into_iter()
                            .filter(|member| matches!(member, ir::Type::Struct(_) | ir::Type::Enum(_))),
                    )
                    .filter(|(_, member)| !result.contains(member))
                    .map(|((_, number), _)| number)
                    .collect();
                let errors = if errors.is_empty() { u32::MAX } else { self.type_set(errors) };
                self.emit(Instr::Try { dst, src, errors }, span);
            }
            ExprKind::Block { stmts, value } => {
                self.block(stmts);
                self.expr_into(value, dst);
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
            ExprKind::CallBuiltin { builtin: Builtin::Error, args } => {
                let a = self.operand(&args[0]);
                self.emit(Instr::NewError { dst, a }, span);
            }
            ExprKind::CallBuiltin { builtin: Builtin::Message, args } => {
                let a = self.operand(&args[0]);
                self.emit(Instr::ErrorMessage { dst, a }, span);
            }
            ExprKind::CallBuiltin {
                builtin:
                    builtin @ (Builtin::Ask
                    | Builtin::ExpectFailed
                    | Builtin::Fail
                    | Builtin::Exit
                    | Builtin::Reverse
                    | Builtin::Floor
                    | Builtin::Ceil
                    | Builtin::Round
                    | Builtin::Isqrt),
                args,
            } => {
                let a = self.operand(&args[0]);
                let instr = match builtin {
                    Builtin::Ask => Instr::Ask { dst, prompt: a },
                    Builtin::ExpectFailed => Instr::ExpectFailed { message: a },
                    Builtin::Fail => Instr::Fail { message: a },
                    Builtin::Exit => Instr::Exit { code: a },
                    Builtin::Reverse => Instr::Reverse { dst, a },
                    Builtin::Floor => Instr::Floor { dst, a },
                    Builtin::Ceil => Instr::Ceil { dst, a },
                    Builtin::Round => Instr::Round { dst, a },
                    _ => Instr::Isqrt { dst, a },
                };
                self.emit(instr, span);
            }
            ExprKind::CallBuiltin { builtin: Builtin::Native(native), args } => {
                let start = self.next_temp;
                for _ in args {
                    self.temp();
                }
                for (offset, arg) in args.iter().enumerate() {
                    self.expr_into(arg, start + offset as u32);
                }
                let count = args.len() as u32;
                self.emit(Instr::Native { dst, native: *native, start, count }, span);
            }
            ExprKind::CallBuiltin { builtin: Builtin::Foreign { index, .. }, args } => {
                let start = self.next_temp;
                for _ in args {
                    self.temp();
                }
                for (offset, arg) in args.iter().enumerate() {
                    self.expr_into(arg, start + offset as u32);
                }
                let count = args.len() as u32;
                self.emit(Instr::CallForeign { dst, index: *index, start, count }, span);
            }
            ExprKind::CallBuiltin { builtin: Builtin::Broken, args } => {
                let name = self.operand_before(&args[0], &args[1]);
                let detail = self.operand(&args[1]);
                self.emit(Instr::Broken { name, detail }, span);
                self.emit(Instr::LoadNone { dst }, span);
            }
            ExprKind::Struct { structure, fields } => {
                let start = self.next_temp;
                for _ in fields {
                    self.temp();
                }
                for (offset, field) in fields.iter().enumerate() {
                    self.expr_into(field, start + offset as u32);
                }
                let layout = self.shared.layouts[structure];
                self.emit(Instr::MakeStruct { dst, layout, start, count: fields.len() as u32 }, span);
            }
            ExprKind::Enum { enumeration, value } => {
                let enumeration = self.shared.enums[enumeration];
                self.emit(Instr::LoadEnum { dst, enumeration, value: *value }, span);
            }
            ExprKind::Field { object, field } => {
                let object = self.operand(object);
                self.emit(Instr::GetField { dst, object, field: *field }, span);
            }
            ExprKind::CallBuiltin { builtin: Builtin::Sum, args } => {
                let values = self.operand(&args[0]);
                let instr = match expr.ty {
                    ir::Type::Float => Instr::SumFloat { dst, values },
                    ir::Type::Rational => Instr::SumRational { dst, values },
                    _ => Instr::SumInt { dst, values },
                };
                self.emit(instr, span);
            }
        }
        self.next_temp = mark;
    }

    /// The test that a value of type `whole` is of type `part`. A value carries its kind;
    /// a structure or an enumeration also carries its type, needed when `whole` has
    /// several of them (§7.1).
    fn type_test(&mut self, whole: ir::Type, part: ir::Type, dst: Reg, src: Reg) -> Instr {
        use crate::value::kinds;
        let kinds = kinds_of(part);
        let (tested, all) = (self.named_types(part), self.named_types(whole));
        let needed = |kind: u16| {
            kinds & kind != 0
                && tested.iter().filter(|(k, _)| *k == kind).count()
                    < all.iter().filter(|(k, _)| *k == kind).count()
        };
        if !needed(kinds::STRUCT) && !needed(kinds::ENUM) {
            return Instr::TypeTest { dst, src, kinds };
        }
        let set = self.type_set(tested.iter().map(|(_, number)| *number).collect());
        Instr::TypeTestNamed { dst, src, kinds: kinds & !(kinds::STRUCT | kinds::ENUM), set }
    }

    /// The index of a set of type numbers in `Program::type_sets`.
    fn type_set(&mut self, mut set: Vec<u32>) -> u32 {
        set.sort_unstable();
        match self.shared.type_sets.iter().position(|known| *known == set) {
            Some(index) => index as u32,
            None => {
                self.shared.type_sets.push(set);
                self.shared.type_sets.len() as u32 - 1
            }
        }
    }

    /// The structures and enumerations among the members of `ty`: their kind and number.
    fn named_types(&self, ty: ir::Type) -> Vec<(u16, u32)> {
        use crate::value::kinds;
        ty.members()
            .into_iter()
            .filter_map(|member| match member {
                ir::Type::Struct(structure) => Some((kinds::STRUCT, self.shared.layouts[&structure])),
                ir::Type::Enum(enumeration) => {
                    Some((kinds::ENUM, self.shared.first_enum + self.shared.enums[&enumeration]))
                }
                _ => None,
            })
            .collect()
    }

    /// A register holding the value of `expr`: the register of a local, or a new
    /// temporary, which stays reserved until the caller releases its temporaries.
    fn operand(&mut self, expr: &ir::Expr) -> Reg {
        if let ExprKind::Local(local) = expr.kind
            && !self.by_reference(local)
            && !self.boxed(local)
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

    /// A variable shared with a nested function, whose register holds a cell (§11.5).
    fn boxed(&self, local: ir::LocalId) -> bool {
        self.function.local(local).boxed
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
        self.texts.push(Arc::new(text.to_string()));
        self.text_indices.insert(text.to_string(), index);
        index
    }

    fn emit(&mut self, instr: Instr, span: Option<Span>) {
        self.code.push(instr);
        self.spans.push(span);
    }

    /// Points the jump at `at` to the next instruction.
    fn patch(&mut self, at: usize) {
        self.patch_to(at, self.code.len() as u32);
    }

    fn patch_to(&mut self, at: usize, destination: u32) {
        match &mut self.code[at] {
            Instr::Jump { target }
            | Instr::JumpIfFalse { target, .. }
            | Instr::JumpIfTrue { target, .. }
            | Instr::JumpIfArgs { target, .. }
            | Instr::JumpUnlessInt { target, .. }
            | Instr::JumpUnlessIntImm { target, .. }
            | Instr::ForRange { target, .. }
            | Instr::ForList { target, .. } => *target = destination,
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
        ExprKind::Range { start, end } => calls_function(start) || calls_function(end),
        // A comprehension may call functions from its statements.
        ExprKind::Block { .. } => true,
        ExprKind::Map(entries) => {
            entries.iter().any(|(key, value)| calls_function(key) || calls_function(value))
        }
        ExprKind::List(elements) | ExprKind::Set(elements) | ExprKind::Tuple(elements) => {
            elements.iter().any(calls_function)
        }
        ExprKind::Index { object, index } => calls_function(object) || calls_function(index),
        ExprKind::Slice { object, range } => calls_function(object) || calls_function(range),
        ExprKind::Property { object, .. } => calls_function(object),
        ExprKind::TypeTest { value, .. } | ExprKind::Try(value) => calls_function(value),
        ExprKind::Concat(parts) => parts.iter().any(calls_function),
        ExprKind::CallBuiltin { args, .. } => args.iter().any(calls_function),
        ExprKind::Struct { fields, .. } => fields.iter().any(calls_function),
        ExprKind::Field { object, .. } => calls_function(object),
        ExprKind::Enum { .. } | ExprKind::Cell(_) => false,
        ExprKind::Closure { captures, .. } => captures.iter().any(calls_function),
        ExprKind::CallValue { .. } => true,
        ExprKind::Task(value) | ExprKind::Wait(value) => calls_function(value),
        ExprKind::Partial { callee, args } => calls_function(callee) || args.iter().any(calls_function),
        ExprKind::Compile(value) => calls_function(value),
    }
}

/// The kinds of values of a type, as the bits of [`crate::value::kinds`].
fn kinds_of(ty: ir::Type) -> u16 {
    use crate::value::kinds;
    ty.members()
        .into_iter()
        .map(|member| match member {
            ir::Type::Int => kinds::INT,
            ir::Type::Float => kinds::FLOAT,
            ir::Type::Rational => kinds::RATIONAL,
            ir::Type::Bool => kinds::BOOL,
            ir::Type::Text => kinds::TEXT,
            ir::Type::None => kinds::NONE,
            ir::Type::Range => kinds::RANGE,
            ir::Type::List(_) => kinds::LIST,
            ir::Type::Set(_) => kinds::SET,
            ir::Type::Map(_) => kinds::MAP,
            ir::Type::Task(_) => kinds::TASK,
            ir::Type::Tuple(_) => kinds::TUPLE,
            ir::Type::Error => kinds::ERROR,
            ir::Type::Struct(_) => kinds::STRUCT,
            ir::Type::Enum(_) => kinds::ENUM,
            // A Domain is the function that tells whether a value belongs to it (C74).
            ir::Type::Fun(_) | ir::Type::Domain(_) => kinds::FUN,
            // A trait without members has no values.
            ir::Type::Trait(_) => 0,
            ir::Type::Var(_) => unreachable!("an instance has no type variables"),
            ir::Type::Union(_) => unreachable!("the members of a union are not unions"),
        })
        .fold(0, |all, kind| all | kind)
}

/// An Int literal that fits in the immediate operand of an instruction.
fn small_int(expr: &ir::Expr) -> Option<i32> {
    match expr.kind {
        ExprKind::Int(value) => i32::try_from(value).ok(),
        _ => None,
    }
}

fn int_comparison(op: BinaryOp) -> Option<Cmp> {
    Some(match op {
        BinaryOp::EqInt => Cmp::Eq,
        BinaryOp::NeInt => Cmp::Ne,
        BinaryOp::LtInt => Cmp::Lt,
        BinaryOp::LeInt => Cmp::Le,
        BinaryOp::GtInt => Cmp::Gt,
        BinaryOp::GeInt => Cmp::Ge,
        _ => return None,
    })
}

/// The instruction for `a op imm`, when there is one. Dividing by a constant 0 keeps
/// the general instruction, which reports the bug.
fn immediate_instr(op: BinaryOp, dst: Reg, a: Reg, imm: i32) -> Option<Instr> {
    Some(match op {
        BinaryOp::AddInt => Instr::AddIntImm { dst, a, imm },
        BinaryOp::SubInt => Instr::SubIntImm { dst, a, imm },
        BinaryOp::MulInt => Instr::MulIntImm { dst, a, imm },
        BinaryOp::DivInt if imm != 0 => Instr::DivIntImm { dst, a, imm },
        BinaryOp::ModInt if imm != 0 => Instr::ModIntImm { dst, a, imm },
        _ => return None,
    })
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
        BinaryOp::Over => Instr::MakeRational { dst, a, b },
        BinaryOp::AddRational => Instr::AddRational { dst, a, b },
        BinaryOp::SubRational => Instr::SubRational { dst, a, b },
        BinaryOp::MulRational => Instr::MulRational { dst, a, b },
        BinaryOp::DivRational => Instr::DivRational { dst, a, b },
        BinaryOp::PowRational => Instr::PowRational { dst, a, b },
        BinaryOp::EqRational => Instr::CmpRational { dst, cmp: Cmp::Eq, a, b },
        BinaryOp::NeRational => Instr::CmpRational { dst, cmp: Cmp::Ne, a, b },
        BinaryOp::LtRational => Instr::CmpRational { dst, cmp: Cmp::Lt, a, b },
        BinaryOp::LeRational => Instr::CmpRational { dst, cmp: Cmp::Le, a, b },
        BinaryOp::GtRational => Instr::CmpRational { dst, cmp: Cmp::Gt, a, b },
        BinaryOp::GeRational => Instr::CmpRational { dst, cmp: Cmp::Ge, a, b },
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
        BinaryOp::InRange => Instr::InRange { dst, a, b },
        BinaryOp::InList => Instr::InList { dst, a, b },
        BinaryOp::InSet => Instr::InSet { dst, a, b },
        BinaryOp::InMap => Instr::InMap { dst, a, b },
        BinaryOp::SetUnion => Instr::SetUnion { dst, a, b },
        BinaryOp::SetInter => Instr::SetInter { dst, a, b },
        BinaryOp::SetMinus => Instr::SetMinus { dst, a, b },
        BinaryOp::Subset => Instr::Subset { dst, a, b },
        BinaryOp::EqValue => Instr::EqValue { dst, a, b },
        BinaryOp::NeValue => Instr::NeValue { dst, a, b },
        BinaryOp::EqNone | BinaryOp::NeNone => unreachable!("compiled to a constant"),
    }
}
