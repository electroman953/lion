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

/// A checked program: its functions, one of which is the script itself.
pub struct Program {
    pub functions: Vec<Function>,
    /// The top-level statements of the file that is run (§20.1). The locals declared
    /// at its top level are the globals, which other functions reach with `Global`.
    pub main: FunctionId,
}

impl Program {
    pub fn function(&self, id: FunctionId) -> &Function {
        &self.functions[id.index()]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FunctionId(pub u32);

impl FunctionId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

pub struct Function {
    pub name: String,
    /// The first `params` locals are the parameters, in order.
    pub params: u32,
    /// The values of omitted arguments. A call that gives fewer than `index + 1`
    /// arguments gives parameter `index` this value, evaluated at the call, in the
    /// order of the parameters (§11.2).
    pub defaults: Vec<(u32, Expr)>,
    pub ret: Type,
    pub locals: Vec<Local>,
    pub body: Vec<Stmt>,
    /// The name in the declaration; `None` for the script.
    pub span: Option<Span>,
}

impl Function {
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
    /// A `var` parameter: it designates the caller's variable (§11.2). Reading and
    /// assigning it go through the reference.
    pub by_reference: bool,
    pub span: Span,
}

/// Where an assignment stores its value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Local(LocalId),
    /// A top-level variable of the script, from another function.
    Global(LocalId),
}

pub enum Stmt {
    Assign {
        place: Place,
        value: Expr,
    },
    /// Evaluates an expression for its effects and discards its value.
    Expr(Expr),
    /// `elif` chains are nested in `otherwise`.
    If {
        cond: Expr,
        then: Vec<Stmt>,
        otherwise: Vec<Stmt>,
    },
    While {
        cond: Expr,
        body: Vec<Stmt>,
    },
    /// Leaves the innermost loop.
    Break,
    /// Goes to the next turn of the innermost loop.
    Continue,
    /// Leaves the function with its value (§11.4); in the script, ends the program (§20.1).
    Return(Option<Expr>),
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
    /// A top-level variable of the script, read from another function (§11.5).
    Global(LocalId),
    /// Arguments are evaluated left to right (§9.2); omitted ones take their default.
    Call {
        function: FunctionId,
        args: Vec<Arg>,
    },
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
    /// `if cond then then else otherwise`; `elif` chains are nested in `otherwise`.
    If {
        cond: Box<Expr>,
        then: Box<Expr>,
        otherwise: Box<Expr>,
    },
    CallBuiltin {
        builtin: Builtin,
        args: Vec<Expr>,
    },
}

/// An argument of a call.
#[derive(Clone)]
pub enum Arg {
    Value(Expr),
    /// For a `var` parameter: the variable itself (§11.2).
    Reference(Place),
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
