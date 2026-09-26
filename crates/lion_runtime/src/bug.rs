//! Runtime bugs: failures that stop the program immediately (spec §18.1).

use crate::format::format_float;

/// The Int operation that failed, to describe it in messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    Power,
    Negate,
}

impl IntOp {
    fn symbol(self) -> &'static str {
        match self {
            IntOp::Add => "+",
            IntOp::Subtract | IntOp::Negate => "-",
            IntOp::Multiply => "*",
            IntOp::Divide => "div",
            IntOp::Modulo => "mod",
            IntOp::Power => "^",
        }
    }

    fn noun(self) -> &'static str {
        match self {
            IntOp::Add => "addition",
            IntOp::Subtract => "subtraction",
            IntOp::Multiply => "multiplication",
            IntOp::Divide | IntOp::Modulo => "division",
            IntOp::Power => "power",
            IntOp::Negate => "negation",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BugKind {
    /// The exact result does not fit in a 64-bit Int (§8.1). `rhs` is `None` for negation.
    IntOverflow { op: IntOp, lhs: i64, rhs: Option<i64> },
    /// `div` or `mod` by zero (§8.1, D62).
    DivisionByZero { op: IntOp, lhs: i64 },
    /// `Int ^ Int` with a negative exponent (§8.4).
    NegativeExponent { base: i64, exponent: i64 },
    /// `x as Int` where `x` is NaN, infinite or outside the Int range (§8.5, D74).
    InvalidFloatToInt { value: f64 },
    /// More than [`MAX_CALL_DEPTH`] calls in progress: usually a recursion that never ends.
    StackOverflow,
    /// `l[i]` with `i` outside `1..l.size` (§16.2, D38).
    IndexOutOfRange { index: i64, size: usize },
    /// `l[a..b]` with a non-empty interval outside `1..l.size` (§16.2, D41).
    SliceOutOfRange { start: i64, end: i64, size: usize },
    /// `l.first` or `l.last` of an empty list (D38).
    EmptyList,
}

/// The number of calls that may be in progress at once, the same in both modes (C13).
pub const MAX_CALL_DEPTH: usize = 100_000;

impl BugKind {
    /// One-line description, used as the title of the bug report.
    pub fn message(&self) -> String {
        match self {
            BugKind::IntOverflow { .. } => "integer overflow".to_string(),
            BugKind::DivisionByZero { .. } => "integer division by zero".to_string(),
            BugKind::NegativeExponent { .. } => "negative exponent in an Int power".to_string(),
            BugKind::InvalidFloatToInt { .. } => "cannot convert this Float to an Int".to_string(),
            BugKind::StackOverflow => "too many nested calls".to_string(),
            BugKind::IndexOutOfRange { .. } => "index out of range".to_string(),
            BugKind::SliceOutOfRange { .. } => "extract out of range".to_string(),
            BugKind::EmptyList => "the list is empty".to_string(),
        }
    }

    /// The values involved (spec §18.4: a bug shows the values in question).
    pub fn details(&self) -> String {
        match *self {
            BugKind::IntOverflow { op, lhs, rhs: Some(rhs) } => {
                format!("{lhs} {} {rhs} exceeds the capacity of an Int (64 bits)", op.symbol())
            }
            BugKind::IntOverflow { lhs, .. } => {
                format!("-({lhs}) exceeds the capacity of an Int (64 bits)")
            }
            BugKind::DivisionByZero { op, lhs } => {
                format!("{lhs} {} 0 has no value", op.symbol())
            }
            BugKind::NegativeExponent { base, exponent } => {
                format!("{base} ^ {exponent} is not an Int")
            }
            BugKind::InvalidFloatToInt { value } if value.is_finite() => {
                format!("{} is outside the range of an Int", format_float(value))
            }
            BugKind::InvalidFloatToInt { value } => {
                format!("{} has no Int value", format_float(value))
            }
            BugKind::StackOverflow => format!("more than {MAX_CALL_DEPTH} calls are in progress"),
            BugKind::IndexOutOfRange { index, size: 0 } => format!("index {index} in an empty sequence"),
            BugKind::IndexOutOfRange { index, size } => {
                format!("index {index} is outside 1..{size}; indices start at 1 (§16.2)")
            }
            BugKind::SliceOutOfRange { start, end, size } => {
                format!("{start}..{end} is not inside 1..{size}")
            }
            BugKind::EmptyList => "an empty list has no first or last element".to_string(),
        }
    }

    /// A suggestion to fix the program (spec §18.4, D18).
    pub fn help(&self) -> String {
        match *self {
            BugKind::IntOverflow { op, .. } => {
                format!("use a Float, or check the value before the {}", op.noun())
            }
            BugKind::DivisionByZero { .. } => {
                "check that the divisor is not zero before dividing".to_string()
            }
            BugKind::NegativeExponent { .. } => {
                "for a fractional result, use a Float base: `(x as Float) ^ n`".to_string()
            }
            BugKind::InvalidFloatToInt { .. } => {
                "check the value before converting it with `as Int`".to_string()
            }
            BugKind::StackOverflow => {
                "check that the recursion reaches a case that does not call again".to_string()
            }
            BugKind::IndexOutOfRange { .. } | BugKind::SliceOutOfRange { .. } => {
                "check the index against `size` first; the last element is at `size`, or `last`".to_string()
            }
            BugKind::EmptyList => "check that `size > 0` first".to_string(),
        }
    }
}
