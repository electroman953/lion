//! The operations on values that the two modes share (§22.2): the virtual machine calls
//! them, and so do the programs that `lion build` compiles, through `lion_native`. One
//! implementation gives the same results and the same bugs in both modes.
//!
//! Comparing values may call a function of the program, the `equals` of a structure
//! (§12.5). Each mode calls it its own way: the operations receive a [`Comparer`].

use std::ffi::{CString, c_void};
use std::io::{self, BufRead, Write};
use std::rc::Rc;

use lion_ir::{ForeignFunction, Type};
use lion_runtime::BugKind;
use lion_runtime::ops::{self, RationalOp};

use crate::ffi::{self, CResult, CValue};
use crate::machine::Trap;
use crate::map::MapValue;
use crate::set::{SetValue, holds_nan};
use crate::value::Value;

/// Why an operation stopped: a bug of the program, or a trap met by a function of the
/// program that it called.
#[derive(Debug)]
pub enum Stop {
    Bug(BugKind),
    Fault(Box<Trap>),
}

impl From<BugKind> for Stop {
    fn from(kind: BugKind) -> Stop {
        Stop::Bug(kind)
    }
}

impl From<Box<Trap>> for Stop {
    fn from(fault: Box<Trap>) -> Stop {
        Stop::Fault(fault)
    }
}

/// How values are compared: with the equality of content, and with the `equals` of the
/// structures that define one (§12.5), which the mode calls.
pub trait Comparer {
    /// Whether a structure of the program defines its equality. When none does, the
    /// equality of content is enough, without calling anything.
    fn custom_equality(&self) -> bool;

    /// The result of the function `function` of the program, the `equals` of a
    /// structure, called with two values.
    fn call_equals(&mut self, function: u32, a: Value, b: Value) -> Result<Value, Box<Trap>>;
}

/// Equality of content (§9.4), with the `equals` of the structures that define one
/// (§12.5).
pub fn equal(c: &mut dyn Comparer, a: &Value, b: &Value) -> Result<bool, Box<Trap>> {
    if !c.custom_equality() {
        return Ok(a.equals(b));
    }
    Ok(match (a, b) {
        (Value::Struct(x), Value::Struct(y)) if x.layout.index == y.layout.index => match x.layout.equals {
            Some(function) => match c.call_equals(function, a.clone(), b.clone())? {
                Value::Bool(equal) => equal,
                other => panic!("`equals` gave {} instead of a Bool", other.type_name()),
            },
            None => {
                let (x, y) = (Rc::clone(x), Rc::clone(y));
                for (first, second) in x.fields.iter().zip(&y.fields) {
                    if !equal(c, first, second)? {
                        return Ok(false);
                    }
                }
                true
            }
        },
        (Value::List(x), Value::List(y)) | (Value::Tuple(x), Value::Tuple(y)) => {
            if x.len() != y.len() {
                return Ok(false);
            }
            let (x, y) = (Rc::clone(x), Rc::clone(y));
            for (first, second) in x.iter().zip(y.iter()) {
                if !equal(c, first, second)? {
                    return Ok(false);
                }
            }
            true
        }
        (Value::Set(x), Value::Set(y)) => {
            let (x, y) = (Rc::clone(x), Rc::clone(y));
            x.len() == y.len() && set_subset(c, &x, &y)?
        }
        (Value::Map(x), Value::Map(y)) => {
            let (x, y) = (Rc::clone(x), Rc::clone(y));
            if x.len() != y.len() {
                return Ok(false);
            }
            for (key, value) in x.entries() {
                let Some(position) = map_position(c, &y, key)? else { return Ok(false) };
                if !equal(c, value, y.value_at(position))? {
                    return Ok(false);
                }
            }
            true
        }
        _ => a.equals(b),
    })
}

/// `value in list`, with `==` (§16.2).
pub fn in_list(c: &mut dyn Comparer, value: &Value, list: &[Value]) -> Result<bool, Box<Trap>> {
    if !c.custom_equality() {
        return Ok(list.iter().any(|element| element.equals(value)));
    }
    for element in list {
        if equal(c, element, value)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether an element of the Set equals `value`.
pub fn set_contains(c: &mut dyn Comparer, set: &SetValue, value: &Value) -> Result<bool, Box<Trap>> {
    if !c.custom_equality() {
        return Ok(set.contains(value));
    }
    if holds_nan(value) {
        return Ok(false);
    }
    let candidates: Vec<Value> = set.candidates(value).cloned().collect();
    for candidate in &candidates {
        if equal(c, candidate, value)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Adds the value to the Set, unless an element equals it; NaN is a bug (§8.2).
pub fn set_insert(c: &mut dyn Comparer, set: &mut SetValue, value: Value) -> Result<(), Stop> {
    if holds_nan(&value) {
        return Err(Stop::Bug(BugKind::NanInSet));
    }
    if !set_contains(c, set, &value)? {
        set.push_new(value);
    }
    Ok(())
}

/// `{a, b, c}`: the values without repetition, in the order of their first appearance.
pub fn make_set(c: &mut dyn Comparer, values: Vec<Value>) -> Result<SetValue, Stop> {
    if !c.custom_equality() {
        return SetValue::from_values(values).map_err(|_| Stop::Bug(BugKind::NanInSet));
    }
    let mut set = SetValue::default();
    for value in values {
        set_insert(c, &mut set, value)?;
    }
    Ok(set)
}

/// `a subset b`.
pub fn set_subset(c: &mut dyn Comparer, first: &SetValue, second: &SetValue) -> Result<bool, Box<Trap>> {
    if !c.custom_equality() {
        return Ok(first.subset(second));
    }
    for value in first.items() {
        if !set_contains(c, second, value)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// `a union b`: the elements of `a`, then those of `b` that are not in it (C57).
pub fn set_union(c: &mut dyn Comparer, first: &SetValue, second: &SetValue) -> Result<SetValue, Box<Trap>> {
    if !c.custom_equality() {
        return Ok(first.union(second));
    }
    let mut result = first.clone();
    for value in second.items() {
        if !set_contains(c, &result, value)? {
            result.push_new(value.clone());
        }
    }
    Ok(result)
}

/// `a inter b` when `keep_common`, `a minus b` otherwise: the elements of `a` that are
/// in `b`, or that are not.
pub fn set_filter(
    c: &mut dyn Comparer,
    first: &SetValue,
    second: &SetValue,
    keep_common: bool,
) -> Result<SetValue, Box<Trap>> {
    if !c.custom_equality() {
        return Ok(if keep_common { first.inter(second) } else { first.minus(second) });
    }
    let mut result = SetValue::default();
    for value in first.items() {
        if set_contains(c, second, value)? == keep_common {
            result.push_new(value.clone());
        }
    }
    Ok(result)
}

/// The position of the key in the Map.
pub fn map_position(c: &mut dyn Comparer, map: &MapValue, key: &Value) -> Result<Option<usize>, Box<Trap>> {
    if !c.custom_equality() {
        return Ok(map.position(key));
    }
    for position in map.candidates(key) {
        let candidate = map.key_at(position).clone();
        if equal(c, &candidate, key)? {
            return Ok(Some(position));
        }
    }
    Ok(None)
}

/// A Map with these keys and values, in this order; a later value of a key replaces
/// the earlier one. A key that holds NaN is a bug (C79).
pub fn make_map(c: &mut dyn Comparer, pairs: Vec<(Value, Value)>) -> Result<MapValue, Stop> {
    let mut map = MapValue::default();
    for (key, value) in pairs {
        if holds_nan(&key) {
            return Err(Stop::Bug(BugKind::NanKey));
        }
        match map_position(c, &map, &key)? {
            Some(position) => *map.value_mut(position) = value,
            None => {
                map.push_new(key, value);
            }
        }
    }
    Ok(map)
}

/// `m[k]`: the value of the key; a missing key is a bug (C79).
pub fn map_index(c: &mut dyn Comparer, map: &MapValue, key: &Value) -> Result<Value, Stop> {
    match map_position(c, map, key)? {
        Some(position) => Ok(map.value_at(position).clone()),
        None => Err(Stop::Bug(BugKind::MissingKey { key: key.literal() })),
    }
}

/// `m.get(k)`: the value of the key, or `none` (C79).
pub fn map_get(c: &mut dyn Comparer, map: &MapValue, key: &Value) -> Result<Value, Box<Trap>> {
    Ok(match map_position(c, map, key)? {
        Some(position) => map.value_at(position).clone(),
        None => Value::None,
    })
}

/// `l[path] = value`: replaces the part of `root` that the steps reach; the last step
/// into a Map adds the key when it is missing (C79).
pub fn store_element(
    c: &mut dyn Comparer,
    root: &mut Value,
    steps: &[Value],
    value: Value,
) -> Result<(), Stop> {
    let keys = key_positions(c, root, steps)?;
    *element_mut(root, steps, keys.as_deref(), true)? = value;
    Ok(())
}

/// `l.add(value)`: adds at the end of the List, or to the Set, that the steps reach in
/// `root`. A Set gets the value unless an element equals it.
pub fn add_element(
    c: &mut dyn Comparer,
    root: &mut Value,
    steps: &[Value],
    value: Value,
) -> Result<(), Stop> {
    let keys = key_positions(c, root, steps)?;
    match element_mut(root, steps, keys.as_deref(), false)? {
        Value::List(elements) => Rc::make_mut(elements).push(value),
        Value::Set(set) => {
            if holds_nan(&value) {
                return Err(Stop::Bug(BugKind::NanInSet));
            }
            if !set_contains(c, set, &value)? {
                Rc::make_mut(set).push_new(value);
            }
        }
        other => panic!("expected a List or a Set but found {}", other.type_name()),
    }
    Ok(())
}

/// `m.remove(key)`: removes the key from the Map that the steps reach in `root`, if it
/// is there (C79).
pub fn remove_element(
    c: &mut dyn Comparer,
    root: &mut Value,
    steps: &[Value],
    key: &Value,
) -> Result<(), Stop> {
    let keys = key_positions(c, root, steps)?;
    match element_mut(root, steps, keys.as_deref(), false)? {
        Value::Map(map) => {
            if let Some(position) = map_position(c, map, key)? {
                Rc::make_mut(map).remove_at(position);
            }
        }
        other => panic!("expected a Map but found {}", other.type_name()),
    }
    Ok(())
}

/// The element reached from `root` through the steps, ready to be changed: every value
/// on the way is copied first if it is shared (§17.1). A step is an index of a List,
/// the position of a field (from 0) or a key of a Map. A key is found by position in
/// `keys` when they were found by [`key_positions`], and otherwise with the equality of
/// content. With `create`, the last key is added when it is missing (C79).
pub fn element_mut<'v>(
    root: &'v mut Value,
    steps: &[Value],
    keys: Option<&[Option<usize>]>,
    create: bool,
) -> Result<&'v mut Value, BugKind> {
    let last = steps.len().saturating_sub(1);
    let mut slot = root;
    for (number, step) in steps.iter().enumerate() {
        slot = match (slot, step) {
            (Value::List(elements), Value::Int(step)) => {
                let elements = Rc::make_mut(elements);
                let position = position(*step, elements.len())?;
                &mut elements[position]
            }
            (Value::Struct(record), Value::Int(step)) => &mut Rc::make_mut(record).fields[*step as usize],
            (Value::Map(map), key) => {
                let map = Rc::make_mut(map);
                let found = match keys {
                    Some(keys) => keys[number],
                    None => map.position(key),
                };
                match found {
                    Some(position) => map.value_mut(position),
                    None if create && number == last => {
                        if holds_nan(key) {
                            return Err(BugKind::NanKey);
                        }
                        let position = map.push_new(key.clone(), Value::None);
                        map.value_mut(position)
                    }
                    None => return Err(BugKind::MissingKey { key: key.literal() }),
                }
            }
            (other, _) => {
                panic!("expected a List, a structure or a Map but found {}", other.type_name())
            }
        };
    }
    Ok(slot)
}

/// When a structure defines its equality, the positions of the keys of the Maps on the
/// path, found with the [`Comparer`], which may call `equals` (§12.5); `None` for the
/// other steps, and for a missing key.
pub fn key_positions(
    c: &mut dyn Comparer,
    root: &Value,
    steps: &[Value],
) -> Result<Option<Vec<Option<usize>>>, Box<Trap>> {
    if !c.custom_equality() {
        return Ok(None);
    }
    let mut value = root.clone();
    let mut positions = Vec::new();
    for step in steps {
        let next = match (&value, step) {
            (Value::List(elements), Value::Int(index)) => {
                positions.push(None);
                position(*index, elements.len()).ok().map(|found| elements[found].clone())
            }
            (Value::Struct(record), Value::Int(field)) => {
                positions.push(None);
                record.fields.get(*field as usize).cloned()
            }
            (Value::Map(map), key) => {
                let map = Rc::clone(map);
                let found = map_position(c, &map, key)?;
                positions.push(found);
                found.map(|found| map.value_at(found).clone())
            }
            _ => None,
        };
        match next {
            Some(next) => value = next,
            None => break,
        }
    }
    positions.resize(steps.len(), None);
    Ok(Some(positions))
}

/// `l[i]` or `t[i]`, from 1 (§16.2, D42).
pub fn get_index(object: &Value, index: i64) -> Result<Value, BugKind> {
    match object {
        Value::List(elements) => {
            let position = position(index, elements.len())?;
            Ok(elements[position].clone())
        }
        Value::Text(text) => {
            let size = text.chars().count();
            let position = position(index, size)?;
            let character = text.chars().nth(position).expect("the position is inside the text");
            Ok(Value::Text(Rc::new(character.to_string())))
        }
        other => panic!("expected a List or a Text but found {}", other.type_name()),
    }
}

/// `l[a..b]` or `t[a..b]`, bounds included; an empty interval gives an empty extract
/// (D41, C27).
pub fn get_slice(object: &Value, [start, end]: [i64; 2]) -> Result<Value, BugKind> {
    let size = match object {
        Value::List(elements) => elements.len(),
        Value::Text(text) => text.chars().count(),
        other => panic!("expected a List or a Text but found {}", other.type_name()),
    };
    let (from, to) = if start > end {
        (0, 0)
    } else if start < 1 || end > size as i64 {
        return Err(BugKind::SliceOutOfRange { start, end, size });
    } else {
        (start as usize - 1, end as usize)
    };
    Ok(match object {
        Value::List(elements) => Value::List(Rc::new(elements[from..to].to_vec())),
        Value::Text(text) => Value::Text(Rc::new(text.chars().skip(from).take(to - from).collect())),
        _ => unreachable!("checked above"),
    })
}

/// `x.size`: the elements of a List, Set or Map, the characters of a Text, the
/// integers of a Range.
pub fn size(value: &Value) -> Result<i64, BugKind> {
    Ok(match value {
        Value::List(elements) => elements.len() as i64,
        Value::Set(set) => set.len() as i64,
        Value::Map(map) => map.len() as i64,
        Value::Text(text) => text.chars().count() as i64,
        Value::Range(bounds) => range_size(bounds[0], bounds[1])?,
        other => panic!("expected a List, Text or Range but found {}", other.type_name()),
    })
}

/// `l.first` and `l.last`: an empty List is a bug (D38, C26).
pub fn first(value: &Value) -> Result<Value, BugKind> {
    elements(value).first().cloned().ok_or(BugKind::EmptyList)
}

pub fn last(value: &Value) -> Result<Value, BugKind> {
    elements(value).last().cloned().ok_or(BugKind::EmptyList)
}

/// The elements of a List, or of a Set in their order.
pub fn elements(value: &Value) -> &[Value] {
    match value {
        Value::List(elements) => elements,
        Value::Set(set) => set.items(),
        other => panic!("expected a List or a Set but found {}", other.type_name()),
    }
}

/// The number of values that `for` goes through: elements, or keys of a Map.
pub fn sequence_len(value: &Value) -> usize {
    match value {
        Value::List(elements) => elements.len(),
        Value::Set(set) => set.len(),
        Value::Map(map) => map.len(),
        other => panic!("expected a List, Set or Map but found {}", other.type_name()),
    }
}

/// The value at `position`, from 0, that `for` goes through.
pub fn sequence_at(value: &Value, position: usize) -> Value {
    match value {
        Value::List(elements) => elements[position].clone(),
        Value::Set(set) => set.items()[position].clone(),
        Value::Map(map) => map.key_at(position).clone(),
        other => panic!("expected a List, Set or Map but found {}", other.type_name()),
    }
}

/// `reverse(l)`: a List or a Text in the opposite order (C59).
pub fn reverse(value: &Value) -> Value {
    match value {
        Value::List(elements) => Value::List(Rc::new(elements.iter().rev().cloned().collect())),
        Value::Text(text) => Value::Text(Rc::new(text.chars().rev().collect())),
        other => panic!("expected a List or a Text but found {}", other.type_name()),
    }
}

/// `sum(values)` of a List of Int or of a Range (C31).
pub fn sum_int(values: &Value) -> Result<i64, BugKind> {
    match values {
        Value::Range(bounds) => range_sum(bounds[0], bounds[1]),
        _ => elements(values).iter().try_fold(0i64, |total, element| match element {
            Value::Int(value) => ops::int_add(total, *value),
            other => panic!("expected an Int but found {}", other.type_name()),
        }),
    }
}

/// `sum(values)` of a List of Float, and whether finite values gave an infinity or NaN,
/// which the interpreted mode reports (§22.3).
pub fn sum_float(values: &Value) -> (f64, bool) {
    let mut total = 0.0;
    let mut finite = true;
    for element in elements(values) {
        let Value::Float(value) = element else {
            panic!("expected a Float but found {}", element.type_name())
        };
        finite &= value.is_finite();
        total += value;
    }
    (total, finite && !total.is_finite())
}

/// `sum(values)` of a List of Rational (C72).
pub fn sum_rational(values: &Value) -> Result<[i64; 2], BugKind> {
    elements(values).iter().try_fold([0, 1], |total, element| match element {
        Value::Rational(value) => ops::rational_op(RationalOp::Add, total, **value),
        other => panic!("expected a Rational but found {}", other.type_name()),
    })
}

/// Whether the value is of one of the kinds, or is a structure or an enumeration whose
/// type number is in `named` (§7.1).
pub fn type_test_named(value: &Value, kinds: u16, named: &[u32]) -> bool {
    value.kind() & kinds != 0 || type_number(value).is_some_and(|number| named.contains(&number))
}

/// The type number of a structure or an enumeration.
pub fn type_number(value: &Value) -> Option<u32> {
    match value {
        Value::Struct(record) => Some(record.layout.index),
        Value::Enum(enumeration, _) => Some(enumeration.index),
        _ => None,
    }
}

/// Whether `try` meets an Error in the value: an Error of the standard library, or a
/// structure or an enumeration of the program among `errors` (§18.2, §18.3).
pub fn is_failure(value: &Value, errors: &[u32]) -> bool {
    matches!(value, Value::Error(_)) || type_number(value).is_some_and(|number| errors.contains(&number))
}

/// The message of an Error that stops the script (§18.3, C39).
pub fn failure_message(value: &Value) -> String {
    match value {
        Value::Error(message) => message.to_string(),
        other => other.to_text(),
    }
}

/// `e.message()` of an Error of the standard library (§18.2).
pub fn error_message(value: &Value) -> Value {
    match value {
        Value::Error(message) => Value::Text(Rc::new(message.to_string())),
        other => panic!("expected an Error but found {}", other.type_name()),
    }
}

/// `"12" as Int`: an Int, or an Error that says why (§8.5, D14).
pub fn text_to_int(text: &str) -> Value {
    match ops::parse_int(text) {
        Ok(value) => Value::Int(value),
        Err(message) => Value::Error(Rc::new(message)),
    }
}

/// `"2.5" as Float`: a Float, or an Error that says why (§8.5, D14).
pub fn text_to_float(text: &str) -> Value {
    match ops::parse_float(text) {
        Ok(value) => Value::Float(value),
        Err(message) => Value::Error(Rc::new(message)),
    }
}

/// The exit code of `exit(code)`, from 0 to 255 (C59).
pub fn exit_code(code: i64) -> Result<u8, BugKind> {
    u8::try_from(code).map_err(|_| BugKind::InvalidExitCode { code })
}

/// `ask(prompt)`: the prompt, followed by a space, then a line without its end. At the
/// end of the input, the line is empty (C59).
pub fn ask(out: &mut dyn Write, input: &mut dyn BufRead, prompt: &str) -> io::Result<String> {
    if !prompt.is_empty() {
        write!(out, "{prompt} ")?;
    }
    out.flush()?;
    let mut line = String::new();
    input.read_line(&mut line)?;
    if line.ends_with('\n') {
        line.pop();
        if line.ends_with('\r') {
            line.pop();
        }
    }
    Ok(line)
}

/// Calls the C function of `signature` with `args` (§21.2, C80). `found` keeps the
/// function once it is found.
pub fn call_foreign(
    found: &mut Option<*mut c_void>,
    signature: &ForeignFunction,
    args: &[Value],
) -> Result<Value, BugKind> {
    let function = match *found {
        Some(function) => function,
        None => {
            let function = ffi::find(&signature.library, &signature.name)
                .map_err(|reason| BugKind::ForeignUnavailable { reason })?;
            *found = Some(function);
            function
        }
    };
    let mut values = Vec::new();
    for arg in &args[..signature.params.len()] {
        values.push(match arg {
            Value::Int(value) => CValue::Int(*value),
            Value::Bool(value) => CValue::Int(i64::from(*value)),
            Value::Float(value) => CValue::Float(*value),
            Value::Text(text) => match CString::new(text.as_str()) {
                Ok(text) => CValue::Text(text),
                Err(_) => return Err(BugKind::ForeignText),
            },
            other => panic!("expected an Int, Float, Bool or Text but found {}", other.type_name()),
        });
    }
    Ok(match ffi::call(function, signature, &values) {
        CResult::Int(value) => Value::Int(value),
        CResult::Float(value) => Value::Float(value),
        CResult::Bool(value) => Value::Bool(value),
        CResult::None => Value::None,
        CResult::Text(Some(text)) => Value::Text(Rc::new(text)),
        CResult::Text(None) if signature.ret == Type::Text => return Err(BugKind::ForeignNoText),
        CResult::Text(None) => Value::None,
    })
}

/// The position, from 0, of the 1-based `index` in a sequence of `size` elements.
pub fn position(index: i64, size: usize) -> Result<usize, BugKind> {
    if index < 1 || index > size as i64 {
        return Err(BugKind::IndexOutOfRange { index, size });
    }
    Ok(index as usize - 1)
}

/// The number of integers in `start..end`, which may not fit in an Int.
pub fn range_size(start: i64, end: i64) -> Result<i64, BugKind> {
    if start > end {
        return Ok(0);
    }
    ops::int_add(ops::int_sub(end, start)?, 1)
}

/// The sum of the integers in `start..end`.
pub fn range_sum(start: i64, end: i64) -> Result<i64, BugKind> {
    let mut total = 0i64;
    if start <= end {
        let mut value = start;
        loop {
            total = ops::int_add(total, value)?;
            if value == end {
                break;
            }
            value += 1;
        }
    }
    Ok(total)
}
