//! The typed intermediate representation shared by every backend.
//!
//! The front end (`lion_sema`) builds a [`Program`] only for valid programs. In it,
//! every name is resolved to a [`LocalId`], every expression carries its [`Type`],
//! implicit conversions are explicit [`Conversion`] nodes and every operator is
//! specialised to its operand types. Backends never reject a program and never redo
//! type analysis, which keeps the interpreted and compiled modes in step (§22.2).

mod print;
mod types;

pub use print::print_program;
pub use types::Type;

use lion_diagnostics::Span;

/// A checked script: the top-level statements of the file that is run (§20.1).
pub struct Program {
    pub locals: Vec<Local>,
    pub body: Vec<Stmt>,
}

impl Program {
    pub fn local(&self, id: LocalId) -> &Local {
        &self.locals[id.index()]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LocalId(pub u32);

impl LocalId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

pub struct Local {
    pub name: String,
    pub ty: Type,
    pub mutable: bool,
    /// Introduced by the compiler, for instance to evaluate an operand of a chained
    /// comparison only once.
    pub temporary: bool,
    pub span: Span,
}

pub enum Stmt {
    /// Stores a value in a local.
    Assign { local: LocalId, value: Expr },
    /// Evaluates an expression for its effects and discards its value.
    Expr(Expr),
}

#[derive(Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub ty: Type,
    pub span: Span,
}

#[derive(Clone)]
pub enum ExprKind {
    Int(i64),
    Float(f64),
    Bool(bool),
    Text(String),
    None,
    Local(LocalId),
    /// Stores `value` in `local`, then evaluates to `body`.
    Let {
        local: LocalId,
        value: Box<Expr>,
        body: Box<Expr>,
    },
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    /// Both operands are evaluated, left to right (§9.2).
    Binary {
        op: BinaryOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// `rhs` is evaluated only when `lhs` is true (§9.2).
    And {
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// `rhs` is evaluated only when `lhs` is false (§9.2).
    Or {
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Convert {
        conversion: Conversion,
        value: Box<Expr>,
    },
    /// Joins Text values, in order (text interpolation).
    Concat(Vec<Expr>),
    CallBuiltin {
        builtin: Builtin,
        args: Vec<Expr>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    NegInt,
    NegFloat,
    Not,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    AddInt,
    SubInt,
    MulInt,
    /// Euclidean `div` (§8.1).
    DivInt,
    /// Euclidean `mod` (§8.1).
    ModInt,
    PowInt,
    AddFloat,
    SubFloat,
    MulFloat,
    DivFloat,
    PowFloat,
    EqInt,
    NeInt,
    LtInt,
    LeInt,
    GtInt,
    GeInt,
    EqFloat,
    NeFloat,
    LtFloat,
    LeFloat,
    GtFloat,
    GeFloat,
    EqBool,
    NeBool,
    EqText,
    NeText,
    EqNone,
    NeNone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conversion {
    /// Implicit or `as Float`; may lose precision beyond 2^53 (§8.5).
    IntToFloat,
    /// `as Int`: truncation toward zero (§8.5, D74).
    FloatToInt,
    /// The text of any value, as `show` writes it; also `as Text` on numbers.
    ToText,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Builtin {
    /// `show(value)`: writes the value and a line end (§23).
    Show,
}

impl BinaryOp {
    pub fn name(self) -> &'static str {
        use BinaryOp::*;
        match self {
            AddInt => "add_int",
            SubInt => "sub_int",
            MulInt => "mul_int",
            DivInt => "div_int",
            ModInt => "mod_int",
            PowInt => "pow_int",
            AddFloat => "add_float",
            SubFloat => "sub_float",
            MulFloat => "mul_float",
            DivFloat => "div_float",
            PowFloat => "pow_float",
            EqInt => "eq_int",
            NeInt => "ne_int",
            LtInt => "lt_int",
            LeInt => "le_int",
            GtInt => "gt_int",
            GeInt => "ge_int",
            EqFloat => "eq_float",
            NeFloat => "ne_float",
            LtFloat => "lt_float",
            LeFloat => "le_float",
            GtFloat => "gt_float",
            GeFloat => "ge_float",
            EqBool => "eq_bool",
            NeBool => "ne_bool",
            EqText => "eq_text",
            NeText => "ne_text",
            EqNone => "eq_none",
            NeNone => "ne_none",
        }
    }
}

impl UnaryOp {
    pub fn name(self) -> &'static str {
        match self {
            UnaryOp::NegInt => "neg_int",
            UnaryOp::NegFloat => "neg_float",
            UnaryOp::Not => "not",
        }
    }
}

impl Conversion {
    pub fn name(self) -> &'static str {
        match self {
            Conversion::IntToFloat => "int_to_float",
            Conversion::FloatToInt => "float_to_int",
            Conversion::ToText => "to_text",
        }
    }
}

impl Builtin {
    pub fn name(self) -> &'static str {
        match self {
            Builtin::Show => "show",
        }
    }
}
