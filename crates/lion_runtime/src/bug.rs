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

#[derive(Clone, Debug, PartialEq)]
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
    /// NaN, or a value that holds NaN, added to a Set (§8.2, §16).
    NanInSet,
    /// `isqrt(n)` with a negative `n` (§23).
    NegativeSquareRoot { value: i64 },
    /// `exit(code)` with a code that the system cannot give back (§20.1).
    InvalidExitCode { code: i64 },
    /// A function of the standard library called with a value it cannot take (§23).
    InvalidArgument { message: String, details: String },
    /// A value of a structure no longer satisfies its invariants after a change (§12.3,
    /// D40). `detail` names the condition and the values of its fields.
    BrokenInvariant { structure: String, detail: String },
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
            BugKind::NanInSet => "NaN cannot go in a Set".to_string(),
            BugKind::NegativeSquareRoot { .. } => "square root of a negative number".to_string(),
            BugKind::InvalidExitCode { .. } => "invalid exit code".to_string(),
            BugKind::InvalidArgument { message, .. } => message.clone(),
            BugKind::BrokenInvariant { structure, .. } => {
                format!("this change breaks an invariant of {structure}")
            }
        }
    }

    /// The values involved (spec §18.4: a bug shows the values in question).
    pub fn details(&self) -> String {
        match *self {
            BugKind::BrokenInvariant { ref detail, .. } => detail.clone(),
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
            BugKind::NanInSet => {
                "NaN is not equal to itself, so a Set could not tell whether it holds it (§8.2)".to_string()
            }
            BugKind::NegativeSquareRoot { value } => format!("isqrt({value}) has no Int value"),
            BugKind::InvalidExitCode { code } => format!("{code} is not in 0..255"),
            BugKind::InvalidArgument { ref details, .. } => details.clone(),
        }
    }

    /// A suggestion to fix the program (spec §18.4, D18).
    pub fn help(&self) -> String {
        match self {
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
            BugKind::NanInSet => "check the values first: `x == x` is false only for NaN".to_string(),
            BugKind::NegativeSquareRoot { .. } => "check that the value is not negative first".to_string(),
            BugKind::InvalidExitCode { .. } => {
                "an exit code is between 0 (success) and 255".to_string()
            }
            BugKind::InvalidArgument { .. } => "check the value before the call".to_string(),
            BugKind::BrokenInvariant { .. } => {
                "a value must satisfy its conditions after each change, and when the outermost `var self` method on it returns: check the values before changing them (§12.3)".to_string()
            }
        }
    }
}
