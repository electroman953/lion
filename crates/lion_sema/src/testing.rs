//! Tests written in the program (spec §24.1): `test "name": ... ;` blocks, checked with
//! the rest of the file and run only by `lion test`, and `expect`, which reports the
//! values it compared when it fails (D72).

use std::collections::HashMap;
use std::rc::Rc;

use lion_diagnostics::{Diagnostic, Span};
use lion_ir::{self as ir, Type};
use lion_syntax::ast;

use crate::functions::Ret;
use crate::{Checker, ContextKind, typed};

impl<'a> Checker<'a> {
    /// Registers the tests of every file; those of the file that is run are listed in
    /// the program (C68).
    pub(crate) fn register_tests(&mut self) {
        for module in 0..self.modules.len() {
            let ast = self.modules[module].ast;
            let previous = self.enter_module(module);
            for stmt in &ast.stmts {
                let ast::StmtKind::Test { name, decl } = &stmt.kind else { continue };
                if module == 0 && self.tests.iter().any(|(known, _)| known == name) {
                    self.diagnostics.push(
                        Diagnostic::error(format!("two tests are named \"{name}\""))
                            .with_primary(decl.name.span, "")
                            .with_help("give each test its own name"),
                    );
                    continue;
                }
                let index = self.functions.len();
                self.register_nested_function(Rc::new((**decl).clone()), Vec::new(), None);
                self.functions[index].test = Some(name.clone());
                self.resolve_signature(index);
                let instance = self.new_instance(index, Vec::new(), None, Ret::Declared(Type::None));
                self.functions[index].instance = Some(instance);
                if module == 0 {
                    self.tests.push((name.clone(), instance));
                    // A test runs after the declarations of the globals that have a value,
                    // without the other statements of the script (C68).
                    let unassigned = self
                        .tables
                        .globals
                        .values()
                        .filter(|global| global.decl.value.is_none())
                        .filter_map(|global| global.local)
                        .collect();
                    self.script_calls.push(crate::functions::ScriptCall::new(
                        instance,
                        decl.name.span,
                        unassigned,
                    ));
                }
            }
            self.enter_module(previous);
        }
    }

    /// `expect condition` (§24.1): nothing when it holds; otherwise the failure is
    /// recorded, with the values compared, and the test goes on (D72, C68).
    pub(crate) fn expect_stmt(&mut self, condition: &ast::Condition, span: Span) -> Option<ir::Stmt> {
        let in_test = match self.ctx.kind {
            ContextKind::Function(instance) => {
                let function = self.instance_function(instance);
                function.is_some_and(|function| self.functions[function].test.is_some())
            }
            _ => false,
        };
        if !in_test {
            self.diagnostics.push(
                Diagnostic::error("`expect` is written in a test")
                    .with_primary(span, "")
                    .with_help("write it in `test \"name\": ... ;` (§24.1)"),
            );
            return None;
        }
        let text = |text: String| typed(ir::ExprKind::Text(text), Type::Text, span);
        let literal = |value: ir::Expr| {
            let span = value.span;
            typed(
                ir::ExprKind::Convert { conversion: ir::Conversion::Literal, value: Box::new(value) },
                Type::Text,
                span,
            )
        };
        // `expect a == b`: « expected b, got a » (D72); another comparison shows both values.
        if let ast::ExprKind::Compare { first, rest } = &condition.expr.kind
            && let [comparison] = rest.as_slice()
        {
            let lhs = self.expr(first);
            let rhs = self.expr(&comparison.rhs);
            let (lhs, rhs) = (lhs?, rhs?);
            let (left, right) = (self.temporary(lhs.ty, lhs.span), self.temporary(rhs.ty, rhs.span));
            let read = |local: ir::LocalId, value: &ir::Expr| {
                typed(ir::ExprKind::Local(local), value.ty, value.span)
            };
            let (left_value, right_value) = (read(left, &lhs), read(right, &rhs));
            let cond = self.compare_values(
                comparison.op,
                comparison.op_span,
                left_value.clone(),
                right_value.clone(),
                span,
            )?;
            let message = if comparison.op == ast::CompareOp::Eq {
                vec![
                    text("expected ".to_string()),
                    literal(right_value),
                    text(", got ".to_string()),
                    literal(left_value),
                ]
            } else {
                vec![
                    text(format!("expected {}: ", condition.text)),
                    literal(left_value),
                    text(format!(" {} ", comparison.op.as_str())),
                    literal(right_value),
                    text(" is false".to_string()),
                ]
            };
            let fail = self.expect_failure(message, span);
            return Some(ir::Stmt::Seq(vec![
                ir::Stmt::Assign { place: ir::Place::Local(left), value: lhs },
                ir::Stmt::Assign { place: ir::Place::Local(right), value: rhs },
                ir::Stmt::If { cond: not(cond), then: vec![fail], otherwise: Vec::new() },
            ]));
        }
        let cond = self.condition(&condition.expr, "expect")?;
        let fail = self.expect_failure(vec![text(format!("expected {}", condition.text))], span);
        Some(ir::Stmt::If { cond: not(cond), then: vec![fail], otherwise: Vec::new() })
    }

    fn expect_failure(&self, parts: Vec<ir::Expr>, span: Span) -> ir::Stmt {
        let message = typed(ir::ExprKind::Concat(parts), Type::Text, span);
        let kind = ir::ExprKind::CallBuiltin { builtin: ir::Builtin::ExpectFailed, args: vec![message] };
        ir::Stmt::Expr(typed(kind, Type::None, span))
    }

    /// The tests of the file that is run, by name, for the program.
    pub(crate) fn test_list(&self) -> Vec<(String, ir::FunctionId)> {
        let mut names: HashMap<&str, ()> = HashMap::new();
        self.tests
            .iter()
            .filter(|(name, _)| names.insert(name, ()).is_none())
            .map(|(name, instance)| (name.clone(), ir::FunctionId(*instance as u32)))
            .collect()
    }
}

fn not(cond: ir::Expr) -> ir::Expr {
    let span = cond.span;
    typed(ir::ExprKind::Unary { op: ir::UnaryOp::Not, operand: Box::new(cond) }, Type::Bool, span)
}
