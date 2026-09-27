//! Execution tests. The front end is reproduced here with the IR built by hand, so
//! these tests do not depend on the parser and the checker.

use lion_diagnostics::{SourceMap, Span};
use lion_ir::{self as ir, BinaryOp, Builtin, Conversion, ExprKind, Type};
use lion_runtime::BugKind;

use crate::{AlertKind, Trap, compile, run};

struct Builder {
    span: Span,
    locals: Vec<ir::Local>,
}

impl Builder {
    fn new() -> Builder {
        let mut map = SourceMap::new();
        let id = map.add("t.lion", "");
        Builder { span: Span::new(id, 0, 0), locals: Vec::new() }
    }

    fn expr(&self, kind: ExprKind, ty: Type) -> ir::Expr {
        ir::Expr { kind, ty, span: self.span }
    }

    fn int(&self, value: i64) -> ir::Expr {
        self.expr(ExprKind::Int(value), Type::Int)
    }

    fn float(&self, value: f64) -> ir::Expr {
        self.expr(ExprKind::Float(value), Type::Float)
    }

    fn binary(&self, op: BinaryOp, lhs: ir::Expr, rhs: ir::Expr, ty: Type) -> ir::Expr {
        self.expr(ExprKind::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) }, ty)
    }

    fn local(&mut self, name: &str, ty: Type) -> ir::LocalId {
        self.locals.push(ir::Local {
            name: name.to_string(),
            ty,
            mutable: true,
            temporary: false,
            by_reference: false,
            boxed: false,
            captured: false,
            synced: false,
            span: self.span,
        });
        ir::LocalId(self.locals.len() as u32 - 1)
    }

    fn read(&self, local: ir::LocalId) -> ir::Expr {
        self.expr(ExprKind::Local(local), self.locals[local.index()].ty)
    }

    fn show(&self, value: ir::Expr) -> ir::Stmt {
        ir::Stmt::Expr(
            self.expr(ExprKind::CallBuiltin { builtin: Builtin::Show, args: vec![value] }, Type::None),
        )
    }

    fn run(self, body: Vec<ir::Stmt>) -> (String, Vec<AlertKind>, Result<(), Trap>) {
        let script = ir::Function {
            name: "script".to_string(),
            params: 0,
            captures: 0,
            defaults: Vec::new(),
            ret: Type::None,
            locals: self.locals,
            body,
            span: None,
        };
        let program = ir::Program {
            functions: vec![script],
            structs: Vec::new(),
            enums: Vec::new(),
            module_inits: vec![None],
            tests: Vec::new(),
            declarations: Vec::new(),
            foreign: Vec::new(),
            main: ir::FunctionId(0),
        };
        let chunk = compile(&program);
        let mut out = Vec::new();
        let mut alerts = Vec::new();
        let result = run(&chunk, &mut out, &mut std::io::empty(), &mut |alert| alerts.push(alert.kind));
        (String::from_utf8(out).unwrap(), alerts, result)
    }
}

#[test]
fn adds_and_shows() {
    let mut b = Builder::new();
    let x = b.local("x", Type::Int);
    let y = b.local("y", Type::Int);
    let body = vec![
        ir::Stmt::Assign { place: ir::Place::Local(x), value: b.int(10) },
        ir::Stmt::Assign { place: ir::Place::Local(y), value: b.int(20) },
        b.show(b.binary(BinaryOp::AddInt, b.read(x), b.read(y), Type::Int)),
    ];
    let (out, alerts, result) = b.run(body);
    assert!(result.is_ok());
    assert!(alerts.is_empty());
    assert_eq!(out, "30\n");
}

#[test]
fn overflow_is_a_bug() {
    let b = Builder::new();
    let body = vec![b.show(b.binary(BinaryOp::AddInt, b.int(i64::MAX), b.int(1), Type::Int))];
    let (out, _, result) = b.run(body);
    assert_eq!(out, "");
    assert!(matches!(result, Err(Trap::Bug { kind: BugKind::IntOverflow { .. }, .. })));
}

#[test]
fn special_floats_raise_an_alert_once_per_location() {
    let b = Builder::new();
    let division = b.binary(BinaryOp::DivFloat, b.float(1.0), b.float(0.0), Type::Float);
    let body = vec![b.show(division.clone()), b.show(division)];
    let (out, alerts, result) = b.run(body);
    assert!(result.is_ok());
    assert_eq!(out, "Infinity\nInfinity\n");
    // Two distinct instructions: one alert each.
    assert_eq!(alerts.len(), 2);
}

#[test]
fn infinity_from_infinity_is_not_new() {
    let b = Builder::new();
    let inf = b.binary(BinaryOp::DivFloat, b.float(1.0), b.float(0.0), Type::Float);
    let body = vec![b.show(b.binary(BinaryOp::AddFloat, inf, b.float(1.0), Type::Float))];
    let (_, alerts, _) = b.run(body);
    assert_eq!(alerts.len(), 1);
}

#[test]
fn precision_loss_alert() {
    let b = Builder::new();
    let value = (1i64 << 53) + 1;
    let convert = b.expr(
        ExprKind::Convert { conversion: Conversion::IntToFloat, value: Box::new(b.int(value)) },
        Type::Float,
    );
    let body = vec![b.show(convert)];
    let (out, alerts, _) = b.run(body);
    assert_eq!(out, "9007199254740992.0\n");
    assert!(matches!(alerts[..], [AlertKind::PrecisionLoss { value: 9007199254740993, .. }]));
}

#[test]
fn and_or_short_circuit_and_assignment_reads_old_value() {
    // x = false; x = true and x  -> false (reads the old x after `true`)
    let mut b = Builder::new();
    let x = b.local("x", Type::Bool);
    let value = b.expr(
        ExprKind::And { lhs: Box::new(b.expr(ExprKind::Bool(true), Type::Bool)), rhs: Box::new(b.read(x)) },
        Type::Bool,
    );
    let body = vec![
        ir::Stmt::Assign { place: ir::Place::Local(x), value: b.expr(ExprKind::Bool(false), Type::Bool) },
        ir::Stmt::Assign { place: ir::Place::Local(x), value },
        b.show(b.read(x)),
    ];
    let (out, _, result) = b.run(body);
    assert!(result.is_ok());
    assert_eq!(out, "false\n");
}

#[test]
fn concat_and_to_text() {
    let b = Builder::new();
    let parts = vec![
        b.expr(ExprKind::Text("pi = ".to_string()), Type::Text),
        b.expr(
            ExprKind::Convert { conversion: Conversion::ToText, value: Box::new(b.float(3.0)) },
            Type::Text,
        ),
        b.expr(ExprKind::Text("!".to_string()), Type::Text),
    ];
    let body = vec![b.show(b.expr(ExprKind::Concat(parts), Type::Text))];
    let (out, _, _) = b.run(body);
    assert_eq!(out, "pi = 3.0!\n");
}
