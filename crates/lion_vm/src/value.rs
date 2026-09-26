use std::rc::Rc;

use lion_runtime::format::format_float;

/// A value in a register of the virtual machine.
#[derive(Clone, Debug, Default)]
pub enum Value {
    #[default]
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(Rc<String>),
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
            Value::Ref(_) => "<reference>".to_string(),
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
            Value::Ref(_) => "reference",
        }
    }
}
