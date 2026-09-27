//! Sets (spec §16.1): values without repetition, compared with `==`, kept in the order
//! of their first insertion so that a program shows the same thing at each run (C57).

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::value::Value;

/// The elements, in the order of their first insertion, and their positions by hash.
/// Equal values have equal hashes; the values of a hash are compared with `==`, or with
/// the `equals` of their structure, which the machine calls (§12.5).
#[derive(Clone, Debug, Default)]
pub struct SetValue {
    items: Vec<Value>,
    index: HashMap<u64, Vec<u32>>,
}

/// A value is not inserted in a Set when it holds NaN, which is not equal to itself (§8.2).
#[derive(Debug)]
pub struct NanInSet;

impl SetValue {
    pub fn items(&self) -> &[Value] {
        &self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The elements that may equal `value`: those of its hash.
    pub fn candidates<'s>(&'s self, value: &Value) -> impl Iterator<Item = &'s Value> + 's {
        let positions = self.index.get(&hash_of(value)).map(Vec::as_slice).unwrap_or_default();
        positions.iter().map(|&position| &self.items[position as usize])
    }

    /// Whether an element is equal to `value`, with the equality of content.
    pub fn contains(&self, value: &Value) -> bool {
        !holds_nan(value) && self.candidates(value).any(|element| element.equals(value))
    }

    /// Adds a value that is not NaN and that no element equals.
    pub fn push_new(&mut self, value: Value) {
        self.index.entry(hash_of(&value)).or_default().push(self.items.len() as u32);
        self.items.push(value);
    }

    /// Adds the value, unless an equal one is there already.
    pub fn insert(&mut self, value: Value) -> Result<(), NanInSet> {
        if holds_nan(&value) {
            return Err(NanInSet);
        }
        if !self.contains(&value) {
            self.push_new(value);
        }
        Ok(())
    }

    pub fn from_values(values: impl IntoIterator<Item = Value>) -> Result<SetValue, NanInSet> {
        let mut set = SetValue::default();
        for value in values {
            set.insert(value)?;
        }
        Ok(set)
    }

    /// Two Sets are equal when they have the same elements, in any order (§9.4).
    pub fn equals(&self, other: &SetValue) -> bool {
        self.len() == other.len() && self.items.iter().all(|value| other.contains(value))
    }

    pub fn union(&self, other: &SetValue) -> SetValue {
        let mut result = self.clone();
        for value in &other.items {
            result.insert(value.clone()).expect("the elements of a Set are not NaN");
        }
        result
    }

    pub fn inter(&self, other: &SetValue) -> SetValue {
        let kept = self.items.iter().filter(|value| other.contains(value)).cloned();
        SetValue::from_values(kept).expect("the elements of a Set are not NaN")
    }

    pub fn minus(&self, other: &SetValue) -> SetValue {
        let kept = self.items.iter().filter(|value| !other.contains(value)).cloned();
        SetValue::from_values(kept).expect("the elements of a Set are not NaN")
    }

    pub fn subset(&self, other: &SetValue) -> bool {
        self.items.iter().all(|value| other.contains(value))
    }
}

pub(crate) fn hash_of(value: &Value) -> u64 {
    let mut hasher = DefaultHasher::new();
    hash_value(value, &mut hasher);
    hasher.finish()
}

/// Whether the value is NaN, or contains one.
pub fn holds_nan(value: &Value) -> bool {
    match value {
        Value::Float(value) => value.is_nan(),
        Value::List(elements) | Value::Tuple(elements) => elements.iter().any(holds_nan),
        Value::Struct(record) => record.fields.iter().any(holds_nan),
        Value::Set(set) => set.items.iter().any(holds_nan),
        Value::Map(map) => map.entries().iter().any(|(key, value)| holds_nan(key) || holds_nan(value)),
        _ => false,
    }
}

/// A hash that agrees with `==`: equal values have equal hashes.
fn hash_value<H: Hasher>(value: &Value, state: &mut H) {
    match value {
        Value::None => 0u8.hash(state),
        Value::Bool(value) => (1u8, value).hash(state),
        Value::Int(value) => (2u8, value).hash(state),
        // 0.0 and -0.0 are equal.
        Value::Float(value) => (3u8, if *value == 0.0 { 0 } else { value.to_bits() }).hash(state),
        Value::Text(text) => (4u8, text.as_str()).hash(state),
        Value::Rational(value) => (11u8, value[0], value[1]).hash(state),
        Value::Error(message) => (5u8, message.as_str()).hash(state),
        Value::List(elements) | Value::Tuple(elements) => {
            (6u8, elements.len()).hash(state);
            for element in elements.iter() {
                hash_value(element, state);
            }
        }
        Value::Range(bounds) => (7u8, bounds[0], bounds[1]).hash(state),
        // A structure with its own equality: only its type counts (§12.5).
        Value::Struct(record) if record.layout.equals.is_some() => (8u8, record.layout.index).hash(state),
        Value::Struct(record) => {
            (8u8, record.layout.index).hash(state);
            for field in &record.fields {
                hash_value(field, state);
            }
        }
        Value::Enum(enumeration, position) => (9u8, enumeration.index, position).hash(state),
        // The order of the elements does not count.
        Value::Set(set) => {
            let combined = set.items.iter().fold(0u64, |total, element| {
                let mut hasher = DefaultHasher::new();
                hash_value(element, &mut hasher);
                total.wrapping_add(hasher.finish())
            });
            (10u8, set.len(), combined).hash(state);
        }
        // The order of the entries does not count.
        Value::Map(map) => {
            let combined = map.entries().iter().fold(0u64, |total, (key, value)| {
                let mut hasher = DefaultHasher::new();
                hash_value(key, &mut hasher);
                hash_value(value, &mut hasher);
                total.wrapping_add(hasher.finish())
            });
            (12u8, map.len(), combined).hash(state);
        }
        Value::Function(_) | Value::Cell(_) | Value::Task(_) | Value::Ref(_) => {
            unreachable!("a function, a cell or a reference does not go in a Set")
        }
    }
}
