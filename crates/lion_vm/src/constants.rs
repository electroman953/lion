//! `compile expr` (spec §21.1, D21): the values computed while the program is compiled.
//! Each expression runs in a function of its own, which has the locals of the function
//! around it, on a machine of its own; its value then replaces the expression, written
//! as literals (C77). The checker made sure that it reads nothing from outside and
//! changes nothing.

use lion_diagnostics::{Diagnostic, Severity, Span};
use lion_ir::{self as ir, Type};

use crate::value::Value;

/// Computes the `compile` expressions of the program and puts their values in their
/// place; a bug met on the way is an error of the program.
pub fn evaluate_compile(program: &mut ir::Program) -> Result<(), Vec<Diagnostic>> {
    // In the order in which they are replaced: the inner expressions first.
    let mut found: Vec<(usize, ir::Expr)> = Vec::new();
    for (index, function) in program.functions.iter().enumerate() {
        let mut body = function.body.clone();
        let mut defaults: Vec<ir::Expr> = function.defaults.iter().map(|(_, value)| value.clone()).collect();
        let mut collect = |expr: &mut ir::Expr| {
            if let ir::ExprKind::Compile(value) = &expr.kind {
                found.push((index, (**value).clone()));
            }
        };
        ir::visit::exprs_in_stmts_mut(&mut body, &mut collect);
        for default in &mut defaults {
            ir::visit::exprs_in_mut(default, &mut collect);
        }
    }
    if found.is_empty() {
        return Ok(());
    }
    let first = program.functions.len();
    for (index, value) in &found {
        let around = &program.functions[*index];
        program.functions.push(ir::Function {
            name: "compile".to_string(),
            params: 0,
            captures: 0,
            defaults: Vec::new(),
            ret: value.ty,
            locals: around.locals.clone(),
            body: vec![ir::Stmt::Return(Some(value.clone()))],
            span: around.span,
        });
    }
    let bytecode = crate::compile(program);
    program.functions.truncate(first);
    let mut values = Vec::new();
    let mut errors = Vec::new();
    for (offset, (_, value)) in found.iter().enumerate() {
        match crate::machine::evaluate(&bytecode, first + offset) {
            Ok(result) => values.push(literal(&result, value.ty, value.span, program)),
            Err(trap) => errors.push(compile_error(trap.to_diagnostic(), value.span)),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let mut values = values.into_iter();
    let mut replace = |expr: &mut ir::Expr| {
        if let ir::ExprKind::Compile(_) = expr.kind {
            *expr = values.next().expect("one value per `compile`");
        }
    };
    for function in &mut program.functions {
        ir::visit::exprs_in_stmts_mut(&mut function.body, &mut replace);
        for (_, default) in &mut function.defaults {
            ir::visit::exprs_in_mut(default, &mut replace);
        }
    }
    Ok(())
}

/// A bug met while computing `compile`: an error of the program, which cannot run.
fn compile_error(bug: Diagnostic, span: Span) -> Diagnostic {
    let mut error = Diagnostic { severity: Severity::Error, ..bug };
    error.message = format!("`compile` stops with a bug: {}", error.message);
    if error.labels.is_empty() {
        error = error.with_primary(span, "");
    }
    error.with_note("`compile` computes its value while the program is compiled (§21.1)")
}

/// The value as an expression of literals, of type `ty`.
fn literal(value: &Value, ty: Type, span: Span, program: &ir::Program) -> ir::Expr {
    let typed = |kind: ir::ExprKind, ty: Type| ir::Expr { kind, ty, span };
    let member = |pick: fn(&Type) -> bool| ty.members().into_iter().find(pick).unwrap_or(ty);
    let kind = match value {
        Value::None => ir::ExprKind::None,
        Value::Bool(value) => ir::ExprKind::Bool(*value),
        Value::Int(value) => ir::ExprKind::Int(*value),
        Value::Float(value) => ir::ExprKind::Float(*value),
        Value::Rational(value) => {
            let part = |value: i64| Box::new(typed(ir::ExprKind::Int(value), Type::Int));
            ir::ExprKind::Binary { op: ir::BinaryOp::Over, lhs: part(value[0]), rhs: part(value[1]) }
        }
        Value::Text(text) => ir::ExprKind::Text(text.to_string()),
        Value::Error(message) => ir::ExprKind::CallBuiltin {
            builtin: ir::Builtin::Error,
            args: vec![typed(ir::ExprKind::Text(message.to_string()), Type::Text)],
        },
        Value::Range(bounds) => ir::ExprKind::Range {
            start: Box::new(typed(ir::ExprKind::Int(bounds[0]), Type::Int)),
            end: Box::new(typed(ir::ExprKind::Int(bounds[1]), Type::Int)),
        },
        Value::List(elements) => {
            let list = member(|ty| matches!(ty, Type::List(_)));
            let element = list.element().unwrap_or(Type::None);
            ir::ExprKind::List(elements.iter().map(|value| literal(value, element, span, program)).collect())
        }
        Value::Set(set) => {
            let set_type = member(|ty| matches!(ty, Type::Set(_)));
            let element = set_type.element().unwrap_or(Type::None);
            ir::ExprKind::Set(
                set.items().iter().map(|value| literal(value, element, span, program)).collect(),
            )
        }
        Value::Tuple(elements) => {
            let types = match member(|ty| matches!(ty, Type::Tuple(_))) {
                Type::Tuple(tuple) => tuple.elements(),
                _ => Vec::new(),
            };
            let values = elements
                .iter()
                .enumerate()
                .map(|(position, value)| {
                    literal(value, types.get(position).copied().unwrap_or(Type::None), span, program)
                })
                .collect();
            ir::ExprKind::Tuple(values)
        }
        Value::Struct(record) => {
            let def = &program.structs[record.layout.index as usize];
            let fields = record
                .fields
                .iter()
                .zip(&def.fields)
                .map(|(value, (_, ty))| literal(value, *ty, span, program))
                .collect();
            ir::ExprKind::Struct { structure: def.id, fields }
        }
        Value::Enum(layout, position) => {
            let enumeration = program.enums[layout.index as usize - program.structs.len()];
            ir::ExprKind::Enum { enumeration, value: *position }
        }
        Value::Function(_) | Value::Cell(_) | Value::Task(_) | Value::Ref(_) => {
            unreachable!("the checker allows only values written with literals in `compile`")
        }
    };
    typed(kind, ty)
}
