//! Maps (C79): values found by their keys, which are unique, kept in the order of their
//! first insertion, as the elements of a Set (C57). The keys are indexed by hash; the
//! keys of a hash are compared with `==`, or by the machine when a structure defines
//! its equality (§12.5).

use std::collections::HashMap;

use crate::set::hash_of;
use crate::value::Value;

#[derive(Clone, Debug, Default)]
pub struct MapValue {
    entries: Vec<(Value, Value)>,
    index: HashMap<u64, Vec<u32>>,
}

impl MapValue {
    pub fn entries(&self) -> &[(Value, Value)] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The positions of the keys that may equal `key`: those of its hash.
    pub fn candidates(&self, key: &Value) -> Vec<usize> {
        let positions = self.index.get(&hash_of(key)).map(Vec::as_slice).unwrap_or_default();
        positions.iter().map(|&position| position as usize).collect()
    }

    /// The position of the key, compared with the equality of content.
    pub fn position(&self, key: &Value) -> Option<usize> {
        self.candidates(key).into_iter().find(|&position| self.entries[position].0.equals(key))
    }

    pub fn key_at(&self, position: usize) -> &Value {
        &self.entries[position].0
    }

    pub fn value_at(&self, position: usize) -> &Value {
        &self.entries[position].1
    }

    pub fn value_mut(&mut self, position: usize) -> &mut Value {
        &mut self.entries[position].1
    }

    /// Adds a key that is not there yet; gives its position.
    pub fn push_new(&mut self, key: Value, value: Value) -> usize {
        let position = self.entries.len();
        self.index.entry(hash_of(&key)).or_default().push(position as u32);
        self.entries.push((key, value));
        position
    }

    /// Removes the entry at `position`; the others keep their order.
    pub fn remove_at(&mut self, position: usize) {
        self.entries.remove(position);
        self.index.clear();
        for (position, (key, _)) in self.entries.iter().enumerate() {
            self.index.entry(hash_of(key)).or_default().push(position as u32);
        }
    }

    /// The same keys, with equal values (§9.4).
    pub fn equals(&self, other: &MapValue) -> bool {
        self.len() == other.len()
            && self.entries.iter().all(|(key, value)| {
                other.position(key).is_some_and(|position| other.value_at(position).equals(value))
            })
    }
}
