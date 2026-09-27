use std::sync::atomic::{Ordering, fence};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use lion_runtime::format::{format_float, format_rational, quote_text};

use crate::bytecode::{EnumLayout, Layout};
use crate::map::MapValue;
use crate::set::SetValue;

/// A value in a register of the virtual machine.
#[derive(Clone, Debug, Default)]
pub enum Value {
    #[default]
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    /// An exact fraction, simplified (§8.3); boxed, so that a value stays small.
    Rational(Arc<[i64; 2]>),
    Text(Arc<String>),
    /// An Error made by `error(...)` or by a failed conversion: its message (§18).
    Error(Arc<String>),
    /// A List, copied only when it is changed while shared (§17.1).
    List(Arc<Vec<Value>>),
    /// `start..end`: only the bounds are stored (§16.3, D46).
    Range(Arc<[i64; 2]>),
    /// A value of a structure, copied only when it is changed while shared (§12, §17.1).
    Struct(Arc<Record>),
    /// A value of an enumeration, by its position (§13.1).
    Enum(Arc<EnumLayout>, u32),
    /// A Set, copied only when it is changed while shared (§16.1).
    Set(Arc<SetValue>),
    /// A Map, copied only when it is changed while shared (C79).
    Map(Arc<MapValue>),
    /// A tuple (§4.5).
    Tuple(Arc<Vec<Value>>),
    /// A task: its result, or the thread that computes it (§19.1, C85).
    Task(Arc<TaskCell>),
    /// A function value, with the values it captured (§11).
    Function(Arc<Closure>),
    /// A variable shared by a function and the code around it (§11.5).
    Cell(Arc<Mutex<Value>>),
    /// A reference to a register of the stack, held by a `var` parameter (§11.2).
    Ref(u32),
}

/// A function value: the function, and the values of the variables it captured.
#[derive(Debug)]
pub struct Closure {
    pub function: u32,
    pub name: Arc<str>,
    /// The first arguments, given by a partial application (§11.3).
    pub bound: Vec<Value>,
    pub captures: Vec<Value>,
}

/// The fields of a value of a structure.
#[derive(Clone, Debug)]
pub struct Record {
    pub layout: Arc<Layout>,
    pub fields: Vec<Value>,
}

impl Value {
    /// The text of the value, as `show` and interpolation write it.
    pub fn to_text(&self) -> String {
        match self {
            Value::None => "none".to_string(),
            Value::Bool(value) => value.to_string(),
            Value::Int(value) => value.to_string(),
            Value::Float(value) => format_float(*value),
            Value::Rational(value) => format_rational(**value),
            Value::Text(text) => text.to_string(),
            Value::Range(bounds) => format!("{}..{}", bounds[0], bounds[1]),
            Value::Error(message) => format!("error({})", quote_text(message)),
            Value::List(elements) => {
                let elements: Vec<String> = elements.iter().map(Value::literal).collect();
                format!("[{}]", elements.join(", "))
            }
            // Written as it is built, with the names of the fields (C51).
            Value::Struct(record) => {
                let fields: Vec<String> = record
                    .layout
                    .fields
                    .iter()
                    .zip(&record.fields)
                    .map(|(name, value)| format!("{name}: {}", value.literal()))
                    .collect();
                format!("{}({})", record.layout.name, fields.join(", "))
            }
            Value::Enum(enumeration, value) => enumeration.values[*value as usize].clone(),
            Value::Set(set) => {
                let elements: Vec<String> = set.items().iter().map(Value::literal).collect();
                format!("{{{}}}", elements.join(", "))
            }
            // `{"a": 1, "b": 2}`; the empty Map is `{:}`, the empty Set being `{}` (C79).
            Value::Map(map) if map.is_empty() => "{:}".to_string(),
            Value::Map(map) => {
                let entries: Vec<String> = map
                    .entries()
                    .iter()
                    .map(|(key, value)| format!("{}: {}", key.literal(), value.literal()))
                    .collect();
                format!("{{{}}}", entries.join(", "))
            }
            Value::Tuple(elements) => {
                let elements: Vec<String> = elements.iter().map(Value::literal).collect();
                match elements.as_slice() {
                    [single] => format!("({single},)"),
                    _ => format!("({})", elements.join(", ")),
                }
            }
            Value::Function(closure) => format!("<fun {}>", closure.name),
            Value::Task(task) => match task.result() {
                Some(result) => format!("<task: {}>", result.literal()),
                None => "<task>".to_string(),
            },
            Value::Cell(cell) => lock(cell).to_text(),
            Value::Ref(_) => "<reference>".to_string(),
        }
    }

    /// The value as a literal: inside a shown collection, a Text is written between
    /// quotes: `["Léa", "Tom"]` (C24).
    pub fn literal(&self) -> String {
        match self {
            Value::Text(text) => quote_text(text),
            other => other.to_text(),
        }
    }

    /// Whether the value keeps memory alive, which a finished frame must release.
    #[inline]
    pub fn holds_memory(&self) -> bool {
        matches!(
            self,
            Value::Text(_)
                | Value::Rational(_)
                | Value::List(_)
                | Value::Error(_)
                | Value::Range(_)
                | Value::Struct(_)
                | Value::Enum(..)
                | Value::Set(_)
                | Value::Map(_)
                | Value::Tuple(_)
                | Value::Function(_)
                | Value::Cell(_)
                | Value::Task(_)
        )
    }

    /// The kind of the value, as one bit, for type tests (§7.1).
    pub fn kind(&self) -> u16 {
        match self {
            Value::Int(_) => kinds::INT,
            Value::Float(_) => kinds::FLOAT,
            Value::Rational(_) => kinds::RATIONAL,
            Value::Bool(_) => kinds::BOOL,
            Value::Text(_) => kinds::TEXT,
            Value::None => kinds::NONE,
            Value::Range(_) => kinds::RANGE,
            Value::List(_) => kinds::LIST,
            Value::Error(_) => kinds::ERROR,
            Value::Struct(_) => kinds::STRUCT,
            Value::Enum(..) => kinds::ENUM,
            Value::Set(_) => kinds::SET,
            Value::Map(_) => kinds::MAP,
            Value::Tuple(_) => kinds::TUPLE,
            Value::Function(_) => kinds::FUN,
            Value::Task(_) => kinds::TASK,
            Value::Cell(_) => 0,
            Value::Ref(_) => 0,
        }
    }

    /// Equality of content (§9.4): Floats follow IEEE 754, so NaN is not equal to itself.
    pub fn equals(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::None, Value::None) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            // Always simplified: equal fractions have equal parts.
            (Value::Rational(a), Value::Rational(b)) => a == b,
            (Value::Text(a), Value::Text(b)) => a == b,
            (Value::Range(a), Value::Range(b)) => a == b,
            (Value::Error(a), Value::Error(b)) => a == b,
            (Value::List(a), Value::List(b)) => {
                a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x.equals(y))
            }
            (Value::Enum(a, x), Value::Enum(b, y)) => a.index == b.index && x == y,
            (Value::Set(a), Value::Set(b)) => a.equals(b),
            (Value::Map(a), Value::Map(b)) => a.equals(b),
            (Value::Tuple(a), Value::Tuple(b)) => {
                a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x.equals(y))
            }
            // Field by field (§12.5).
            (Value::Struct(a), Value::Struct(b)) => {
                a.layout.index == b.layout.index && a.fields.iter().zip(&b.fields).all(|(x, y)| x.equals(y))
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
            Value::Rational(_) => "Rational",
            Value::Text(_) => "Text",
            Value::Range(_) => "Range",
            Value::Error(_) => "Error",
            Value::List(_) => "List",
            Value::Struct(_) => "structure",
            Value::Enum(..) => "enumeration",
            Value::Set(_) => "Set",
            Value::Map(_) => "Map",
            Value::Tuple(_) => "tuple",
            Value::Function(_) => "function",
            Value::Task(_) => "task",
            Value::Cell(_) => "cell",
            Value::Ref(_) => "reference",
        }
    }
}

/// The kinds of values, as bits: a type test checks the kind of a value against the
/// set of kinds of the tested type (§7.1).
pub mod kinds {
    pub const INT: u16 = 1;
    pub const FLOAT: u16 = 1 << 1;
    pub const BOOL: u16 = 1 << 2;
    pub const TEXT: u16 = 1 << 3;
    pub const NONE: u16 = 1 << 4;
    pub const RANGE: u16 = 1 << 5;
    pub const LIST: u16 = 1 << 6;
    pub const ERROR: u16 = 1 << 7;
    pub const STRUCT: u16 = 1 << 8;
    pub const ENUM: u16 = 1 << 9;
    pub const SET: u16 = 1 << 10;
    pub const TUPLE: u16 = 1 << 11;
    pub const FUN: u16 = 1 << 12;
    pub const TASK: u16 = 1 << 13;
    pub const RATIONAL: u16 = 1 << 14;
    pub const MAP: u16 = 1 << 15;
}

/// The value of a cell. A thread that panicked while holding it has already stopped the
/// program: the value is still read.
pub fn lock(cell: &Mutex<Value>) -> MutexGuard<'_, Value> {
    cell.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The value that `shared` points to, ready to be changed: first copied if another value
/// shares it (§17.1). The program never makes `Weak` pointers, so a count of one means
/// that nothing else holds the value: a load tells it, without the atomic exchange of
/// `Arc::make_mut`.
#[inline]
pub fn make_mut<T: Clone>(shared: &mut Arc<T>) -> &mut T {
    if Arc::strong_count(shared) == 1 {
        // What other threads did before they dropped their copies comes before the change.
        fence(Ordering::Acquire);
        // SAFETY: no other `Arc`, and no `Weak`, points to the value, and `shared` is
        // borrowed mutably: nothing else can reach the value while it changes.
        return unsafe { &mut *(Arc::as_ptr(shared) as *mut T) };
    }
    Arc::make_mut(shared)
}

/// A task (§19.1): its result, or the thread that computes it (C85).
#[derive(Debug)]
pub struct TaskCell {
    state: Mutex<TaskState>,
}

#[derive(Debug)]
enum TaskState {
    /// The result is known, and what the task wrote, if anything, is written.
    Done(Value),
    /// A thread computes the task.
    Running(std::thread::JoinHandle<Finished>),
    /// The task ended; what it wrote is not written yet.
    Finished(Finished),
}

/// What a task that ran apart did: what it wrote, then its result, or the trap that
/// stopped it.
#[derive(Debug)]
pub struct Finished {
    pub events: Vec<Event>,
    pub result: Result<Value, Box<crate::machine::Trap>>,
}

/// What a part of the program that runs apart writes, kept to be written later in
/// order: text, and the alerts of the interpreted mode (§22.3).
#[derive(Debug)]
pub enum Event {
    Output(Vec<u8>),
    Alert(crate::machine::Alert),
}

impl TaskCell {
    /// A task whose result is known, and whose output, if any, is written.
    pub fn done(result: Value) -> Arc<TaskCell> {
        Arc::new(TaskCell { state: Mutex::new(TaskState::Done(result)) })
    }

    /// A task that a thread computes.
    pub fn running(thread: std::thread::JoinHandle<Finished>) -> Arc<TaskCell> {
        Arc::new(TaskCell { state: Mutex::new(TaskState::Running(thread)) })
    }

    /// A task that ran apart and ended.
    pub fn finished(finished: Finished) -> Arc<TaskCell> {
        Arc::new(TaskCell { state: Mutex::new(TaskState::Finished(finished)) })
    }

    fn state(&self) -> MutexGuard<'_, TaskState> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        // Waits for the thread, if it still runs.
        if matches!(*state, TaskState::Running(_)) {
            let TaskState::Running(thread) = std::mem::replace(&mut *state, TaskState::Done(Value::None))
            else {
                unreachable!("checked above")
            };
            let finished = thread.join().unwrap_or_else(|_| panic!("the thread of a task panicked"));
            *state = TaskState::Finished(finished);
        }
        state
    }

    /// The first time after the task ended, what it did: its output is then to be
    /// written. Afterwards, `None`: the result is known.
    pub fn take(&self) -> Option<Finished> {
        let mut state = self.state();
        match &*state {
            TaskState::Done(_) => None,
            TaskState::Finished(finished) => {
                let result = match &finished.result {
                    Ok(value) => value.clone(),
                    Err(_) => Value::None,
                };
                let TaskState::Finished(finished) = std::mem::replace(&mut *state, TaskState::Done(result))
                else {
                    unreachable!("matched above")
                };
                Some(finished)
            }
            TaskState::Running(_) => unreachable!("the thread ended"),
        }
    }

    /// The result of the task, when it has one, once it ended; nothing is written.
    pub fn result(&self) -> Option<Value> {
        match &*self.state() {
            TaskState::Done(value) => Some(value.clone()),
            TaskState::Finished(finished) => finished.result.as_ref().ok().cloned(),
            TaskState::Running(_) => unreachable!("the thread ended"),
        }
    }
}
