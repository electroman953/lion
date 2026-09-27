//! Evaluation at compile time of expressions written with constants: the default
//! values of fields (C48), and the conditions of a structure built from constants,
//! which then needs no check at run time (§12.3, D39).
//!
//! The operations are those of `lion_runtime`, as in both execution modes (§22.2).
//! Anything else (a call, a variable of the program...) is not evaluated: the answer
//! is then `None`, and the check waits for the run.

use std::collections::HashMap;

use lion_ir::{self as ir, BinaryOp, Conversion, EnumRef, StructRef, UnaryOp};
use lion_runtime::format::{format_float, quote_text};
use lion_runtime::ops;

/// A value known at compile time.
#[derive(Clone, Debug)]
pub(crate) enum Const {
    Int(i64),
    Float(f64),
    Bool(bool),
    Text(String),
    None,
    List(Vec<Const>),
    Struct(StructRef, Vec<Const>),
    Enum(EnumRef, u32),
    /// Without repetitions, in the order of their first appearance.
    Set(Vec<Const>),
    Tuple(Vec<Const>),
}

impl Const {
    /// Equality of content, as `==` (§9.4): NaN is not equal to itself.
    fn equals(&self, other: &Const) -> bool {
        match (self, other) {
            (Const::Int(a), Const::Int(b)) => a == b,
            (Const::Float(a), Const::Float(b)) => a == b,
            (Const::Bool(a), Const::Bool(b)) => a == b,
            (Const::Text(a), Const::Text(b)) => a == b,
            (Const::None, Const::None) => true,
            (Const::Enum(a, x), Const::Enum(b, y)) => a == b && x == y,
            (Const::Set(a), Const::Set(b)) => {
                a.len() == b.len() && a.iter().all(|x| b.iter().any(|y| x.equals(y)))
            }
            (Const::Tuple(a), Const::Tuple(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.equals(y))
            }
            (Const::List(a), Const::List(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.equals(y))
            }
            (Const::Struct(a, x), Const::Struct(b, y)) => a == b && x.iter().zip(y).all(|(x, y)| x.equals(y)),
            _ => false,
        }
    }

    /// The value as a Lion literal, as the `literal` conversion writes it at run time.
    pub(crate) fn literal(&self, fields: &dyn Fn(StructRef) -> Vec<String>) -> String {
        match self {
            Const::Int(value) => value.to_string(),
            Const::Float(value) => format_float(*value),
            Const::Bool(value) => value.to_string(),
            Const::Text(text) => quote_text(text),
            Const::None => "none".to_string(),
            Const::Enum(enumeration, value) => enumeration.values()[*value as usize].clone(),
            Const::Set(elements) => {
                let elements: Vec<String> = elements.iter().map(|element| element.literal(fields)).collect();
                format!("{{{}}}", elements.join(", "))
            }
            Const::Tuple(elements) => {
                let elements: Vec<String> = elements.iter().map(|element| element.literal(fields)).collect();
                match elements.as_slice() {
                    [single] => format!("({single},)"),
                    _ => format!("({})", elements.join(", ")),
                }
            }
            Const::List(elements) => {
                let elements: Vec<String> = elements.iter().map(|element| element.literal(fields)).collect();
                format!("[{}]", elements.join(", "))
            }
            Const::Struct(structure, values) => {
                let names = fields(*structure);
                let values: Vec<String> = names
                    .iter()
                    .zip(values)
                    .map(|(name, value)| format!("{name}: {}", value.literal(fields)))
                    .collect();
                format!("{}({})", structure.name(), values.join(", "))
            }
        }
    }
}

/// The values of locals during an evaluation.
pub(crate) type Env = HashMap<ir::LocalId, Const>;

/// The value of `expr`, or `None` when it cannot be known before the run, or when
/// computing it is a bug (the run will report it).
pub(crate) fn eval(expr: &ir::Expr, env: &mut Env) -> Option<Const> {
    Some(match &expr.kind {
        ir::ExprKind::Int(value) => Const::Int(*value),
        ir::ExprKind::Float(value) => Const::Float(*value),
        ir::ExprKind::Bool(value) => Const::Bool(*value),
        ir::ExprKind::Text(text) => Const::Text(text.clone()),
        ir::ExprKind::None => Const::None,
        ir::ExprKind::Enum { enumeration, value } => Const::Enum(*enumeration, *value),
        ir::ExprKind::Local(local) => env.get(local)?.clone(),
        ir::ExprKind::Let { local, value, body } => {
            let value = eval(value, env)?;
            env.insert(*local, value);
            eval(body, env)?
        }
        ir::ExprKind::Unary { op, operand } => match (op, eval(operand, env)?) {
            (UnaryOp::NegInt, Const::Int(value)) => Const::Int(ops::int_neg(value).ok()?),
            (UnaryOp::NegFloat, Const::Float(value)) => Const::Float(-value),
            (UnaryOp::Not, Const::Bool(value)) => Const::Bool(!value),
            _ => return None,
        },
        ir::ExprKind::Binary { op, lhs, rhs } => binary(*op, eval(lhs, env)?, eval(rhs, env)?)?,
        ir::ExprKind::And { lhs, rhs } => match eval(lhs, env)? {
            Const::Bool(false) => Const::Bool(false),
            Const::Bool(true) => eval(rhs, env)?,
            _ => return None,
        },
        ir::ExprKind::Or { lhs, rhs } => match eval(lhs, env)? {
            Const::Bool(true) => Const::Bool(true),
            Const::Bool(false) => eval(rhs, env)?,
            _ => return None,
        },
        ir::ExprKind::Convert { conversion, value } => match (conversion, eval(value, env)?) {
            (Conversion::IntToFloat, Const::Int(value)) => Const::Float(ops::int_to_float(value).0),
            (Conversion::FloatToInt, Const::Float(value)) => Const::Int(ops::float_to_int(value).ok()?),
            (Conversion::ToText, Const::Int(value)) => Const::Text(value.to_string()),
            (Conversion::ToText, Const::Float(value)) => Const::Text(format_float(value)),
            (Conversion::ToText, Const::Text(text)) => Const::Text(text),
            (Conversion::ToText, Const::Bool(value)) => Const::Text(value.to_string()),
            (Conversion::ToText | Conversion::Literal, Const::Enum(enumeration, value)) => {
                Const::Text(enumeration.values()[value as usize].clone())
            }
            (Conversion::EnumPosition, Const::Enum(_, value)) => Const::Int(i64::from(value)),
            _ => return None,
        },
        ir::ExprKind::If { cond, then, otherwise } => match eval(cond, env)? {
            Const::Bool(true) => eval(then, env)?,
            Const::Bool(false) => eval(otherwise, env)?,
            _ => return None,
        },
        ir::ExprKind::List(elements) => {
            Const::List(elements.iter().map(|element| eval(element, env)).collect::<Option<_>>()?)
        }
        ir::ExprKind::Tuple(elements) => {
            Const::Tuple(elements.iter().map(|element| eval(element, env)).collect::<Option<_>>()?)
        }
        ir::ExprKind::Set(elements) => {
            let mut set: Vec<Const> = Vec::new();
            for element in elements {
                let value = eval(element, env)?;
                // NaN in a Set is a bug, which the run reports (§8.2).
                if !value.equals(&value) {
                    return None;
                }
                if !set.iter().any(|known| known.equals(&value)) {
                    set.push(value);
                }
            }
            Const::Set(set)
        }
        ir::ExprKind::Concat(parts) => {
            let mut text = String::new();
            for part in parts {
                let Const::Text(part) = eval(part, env)? else { return None };
                text.push_str(&part);
            }
            Const::Text(text)
        }
        ir::ExprKind::Property { object, property } => match (property, eval(object, env)?) {
            (ir::Property::Size, Const::List(elements) | Const::Set(elements)) => {
                Const::Int(elements.len() as i64)
            }
            (ir::Property::Size, Const::Text(text)) => Const::Int(text.chars().count() as i64),
            (ir::Property::First, Const::List(elements)) => elements.first()?.clone(),
            (ir::Property::Last, Const::List(elements)) => elements.last()?.clone(),
            _ => return None,
        },
        ir::ExprKind::Struct { structure, fields } => {
            Const::Struct(*structure, fields.iter().map(|field| eval(field, env)).collect::<Option<_>>()?)
        }
        ir::ExprKind::Field { object, field } => match eval(object, env)? {
            Const::Struct(_, mut values) => values.swap_remove(*field as usize),
            _ => return None,
        },
        _ => return None,
    })
}

fn binary(op: BinaryOp, lhs: Const, rhs: Const) -> Option<Const> {
    use BinaryOp::*;
    use Const::{Bool, Float, Int, Text};
    Some(match (op, lhs, rhs) {
        (AddInt, Int(a), Int(b)) => Int(ops::int_add(a, b).ok()?),
        (SubInt, Int(a), Int(b)) => Int(ops::int_sub(a, b).ok()?),
        (MulInt, Int(a), Int(b)) => Int(ops::int_mul(a, b).ok()?),
        (DivInt, Int(a), Int(b)) => Int(ops::int_div(a, b).ok()?),
        (ModInt, Int(a), Int(b)) => Int(ops::int_mod(a, b).ok()?),
        (PowInt, Int(a), Int(b)) => Int(ops::int_pow(a, b).ok()?),
        (AddFloat, Float(a), Float(b)) => Float(a + b),
        (SubFloat, Float(a), Float(b)) => Float(a - b),
        (MulFloat, Float(a), Float(b)) => Float(a * b),
        (DivFloat, Float(a), Float(b)) => Float(a / b),
        (PowFloat, Float(a), Float(b)) => Float(ops::float_pow(a, b)),
        (EqInt, Int(a), Int(b)) => Bool(a == b),
        (NeInt, Int(a), Int(b)) => Bool(a != b),
        (LtInt, Int(a), Int(b)) => Bool(a < b),
        (LeInt, Int(a), Int(b)) => Bool(a <= b),
        (GtInt, Int(a), Int(b)) => Bool(a > b),
        (GeInt, Int(a), Int(b)) => Bool(a >= b),
        (EqFloat, Float(a), Float(b)) => Bool(a == b),
        (NeFloat, Float(a), Float(b)) => Bool(a != b),
        (LtFloat, Float(a), Float(b)) => Bool(a < b),
        (LeFloat, Float(a), Float(b)) => Bool(a <= b),
        (GtFloat, Float(a), Float(b)) => Bool(a > b),
        (GeFloat, Float(a), Float(b)) => Bool(a >= b),
        (EqBool, Bool(a), Bool(b)) => Bool(a == b),
        (NeBool, Bool(a), Bool(b)) => Bool(a != b),
        (EqText, Text(a), Text(b)) => Bool(a == b),
        (NeText, Text(a), Text(b)) => Bool(a != b),
        (EqNone, _, _) => Bool(true),
        (NeNone, _, _) => Bool(false),
        (EqValue, a, b) => Bool(a.equals(&b)),
        (NeValue, a, b) => Bool(!a.equals(&b)),
        (InList | InSet, value, Const::List(elements) | Const::Set(elements)) => {
            Bool(elements.iter().any(|element| element.equals(&value)))
        }
        _ => return None,
    })
}
