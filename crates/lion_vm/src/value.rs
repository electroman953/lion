use std::rc::Rc;

use lion_runtime::format::{format_float, quote_text};

/// A value in a register of the virtual machine.
#[derive(Clone, Debug, Default)]
pub enum Value {
    #[default]
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(Rc<String>),
    /// A List, copied only when it is changed while shared (§17.1).
    List(Rc<Vec<Value>>),
    /// `start..end`: only the bounds are stored (§16.3, D46).
    Range(Rc<[i64; 2]>),
    /// A reference to a register of the stack, held by a `var` parameter (§11.2).
    Ref(u32),
}

impl Value {
    /// The text of the value, as `show` and interpolation write it.
    pub fn to_text(&self) -> String {
        match self {
            Value::None => "none".to_string(),
            Value::Bool(value) => value.to_string(),
            Value::Int(value) => value.to_string(),
            Value::Float(value) => format_float(*value),
            Value::Text(text) => text.to_string(),
            Value::Range(bounds) => format!("{}..{}", bounds[0], bounds[1]),
            Value::List(elements) => {
                let elements: Vec<String> = elements.iter().map(Value::to_text_in_collection).collect();
                format!("[{}]", elements.join(", "))
            }
            Value::Ref(_) => "<reference>".to_string(),
        }
    }

    /// Inside a shown collection, a Text is written as a literal: `["Léa", "Tom"]` (C24).
    fn to_text_in_collection(&self) -> String {
        match self {
            Value::Text(text) => quote_text(text),
            other => other.to_text(),
        }
    }

    /// Equality of content (§9.4): Floats follow IEEE 754, so NaN is not equal to itself.
    pub fn equals(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::None, Value::None) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Text(a), Value::Text(b)) => a == b,
            (Value::Range(a), Value::Range(b)) => a == b,
            (Value::List(a), Value::List(b)) => {
                a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x.equals(y))
            }
            _ => false,
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::None => "None",
            Value::Bool(_) => "Bool",
            Value::Int(_) => "Int",
            Value::Float(_) => "Float",
            Value::Text(_) => "Text",
            Value::Range(_) => "Range",
            Value::List(_) => "List",
            Value::Ref(_) => "reference",
        }
    }
}
