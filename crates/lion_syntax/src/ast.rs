//! The abstract syntax tree: the program as written, before names and types are checked.

use lion_diagnostics::Span;

#[derive(Clone, Debug)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

/// One source file.
#[derive(Clone, Debug)]
pub struct Module {
    pub stmts: Vec<Stmt>,
}

#[derive(Clone, Debug)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum StmtKind {
    /// `let x = value`, `var x = value in T`, `var x in T` (§6.1).
    Let(LetStmt),
    /// `place = value`, `place += value`, ... (§6.3).
    Assign {
        target: Expr,
        op: AssignOp,
        op_span: Span,
        value: Expr,
    },
    Expr(Expr),
    /// `if c: ... elif d: ... else: ... ;` (§5.2, §10.1).
    If {
        branches: Vec<Branch>,
        otherwise: Option<Block>,
    },
    /// `while c: ... ;` (§10.2).
    While {
        cond: Expr,
        body: Block,
    },
    /// `for x in values: ... ;` (§10.2).
    For {
        var: Ident,
        iterable: Expr,
        body: Block,
    },
    Break,
    Continue,
    /// `return`, with an optional value (§11.4, §20.1).
    Return(Option<Expr>),
    /// `fun name(params) in T modifies x: ... ;` or `fun name(params) = expr` (§11.1).
    Fun(FunDecl),
}

#[derive(Clone, Debug)]
pub struct FunDecl {
    /// `infix fun` (§9.5).
    pub infix: bool,
    /// The type of `self` in `fun Student.passes()` (§12.4).
    pub receiver: Option<Ident>,
    pub name: Ident,
    pub params: Vec<Param>,
    /// The type after `in`, if written.
    pub ret: Option<TypeExpr>,
    /// Type variables: `T in Comparable` (§15.2).
    pub type_params: Vec<(Ident, TypeExpr)>,
    /// The outer variables the function modifies directly (§11.5).
    pub modifies: Vec<Ident>,
    pub body: FunBody,
}

#[derive(Clone, Debug)]
pub struct Param {
    /// `var x`: the parameter works on the caller's variable (§11.2).
    pub var: Option<Span>,
    /// `self` is written as a name.
    pub name: Ident,
    pub ty: Option<TypeExpr>,
    pub default: Option<Expr>,
}

#[derive(Clone, Debug)]
pub enum FunBody {
    Block(Block),
    /// The short form `= expr` (§11.1, D7).
    Expr(Expr),
}

/// One `if` or `elif` branch of an `if` statement.
#[derive(Clone, Debug)]
pub struct Branch {
    pub cond: Expr,
    pub body: Block,
}

/// The statements between `:` and the `;`, `elif` or `else` that ends them (§5.2).
#[derive(Clone, Debug)]
pub struct Block {
    pub stmts: Vec<Stmt>,
}

#[derive(Clone, Debug)]
pub struct LetStmt {
    /// `var` rather than `let`.
    pub mutable: bool,
    pub name: Ident,
    pub value: Option<Expr>,
    /// The type after a final `in` (§6.2, §26 rule 1).
    pub annotation: Option<TypeExpr>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssignOp {
    Set,
    Add,
    Sub,
    Mul,
}

impl AssignOp {
    pub fn as_str(self) -> &'static str {
        match self {
            AssignOp::Set => "=",
            AssignOp::Add => "+=",
            AssignOp::Sub => "-=",
            AssignOp::Mul => "*=",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum ExprKind {
    Int(i64),
    Float(f64),
    Bool(bool),
    None,
    Text(Vec<TextPart>),
    /// A lowercase name: a variable, constant or function (§4.2).
    Name(String),
    /// An uppercase name used as a value, such as `Float` in `x in Float` (§4.2).
    TypeName(String),
    Paren(Box<Expr>),
    /// `[a, b, c]`, or a comprehension `[f(x), x in values, condition]` when an element
    /// is `v in X` with a new name `v` (§16.4, §26 rule 4).
    List(Vec<Expr>),
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    Binary {
        op: BinaryOp,
        op_span: Span,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// One comparison, or several chained ones sharing operands: `a < b <= c` (§9.3).
    Compare {
        first: Box<Expr>,
        rest: Vec<Comparison>,
    },
    As {
        value: Box<Expr>,
        ty: TypeExpr,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Arg>,
    },
    Field {
        object: Box<Expr>,
        name: Ident,
    },
    Index {
        object: Box<Expr>,
        index: Box<Expr>,
    },
    /// `x in T`: whether the value belongs to the type (§7.1).
    TypeTest {
        value: Box<Expr>,
        ty: TypeExpr,
    },
    /// `try expr`: the value, or the Error it gives leaves the function (§18.3).
    Try(Box<Expr>),
    /// `if c then a elif d then b else e` (§10.1).
    If {
        branches: Vec<(Expr, Expr)>,
        otherwise: Option<Box<Expr>>,
    },
}

#[derive(Clone, Debug)]
pub enum TextPart {
    Literal(String),
    Interpolation(Expr),
}

#[derive(Clone, Debug)]
pub struct Comparison {
    pub op: CompareOp,
    pub op_span: Span,
    pub rhs: Expr,
}

#[derive(Clone, Debug)]
pub struct Arg {
    /// `var` written at the call site (§11.2).
    pub var_marker: Option<Span>,
    /// `name:` written at the call site (§11.2).
    pub name: Option<Ident>,
    pub value: Expr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    IntDiv,
    Mod,
    Over,
    Pow,
    Range,
    Inter,
    Union,
    Minus,
    In,
    Subset,
    Same,
    And,
    Or,
}

impl BinaryOp {
    pub fn as_str(self) -> &'static str {
        match self {
            BinaryOp::Add => "+",
            BinaryOp::Sub => "-",
            BinaryOp::Mul => "*",
            BinaryOp::Div => "/",
            BinaryOp::IntDiv => "div",
            BinaryOp::Mod => "mod",
            BinaryOp::Over => "over",
            BinaryOp::Pow => "^",
            BinaryOp::Range => "..",
            BinaryOp::Inter => "inter",
            BinaryOp::Union => "union",
            BinaryOp::Minus => "minus",
            BinaryOp::In => "in",
            BinaryOp::Subset => "subset",
            BinaryOp::Same => "same",
            BinaryOp::And => "and",
            BinaryOp::Or => "or",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompareOp {
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
}

impl CompareOp {
    pub fn as_str(self) -> &'static str {
        match self {
            CompareOp::Eq => "==",
            CompareOp::Ne => "!=",
            CompareOp::Lt => "<",
            CompareOp::Gt => ">",
            CompareOp::Le => "<=",
            CompareOp::Ge => ">=",
        }
    }
}

#[derive(Clone, Debug)]
pub struct TypeExpr {
    pub kind: TypeExprKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum TypeExprKind {
    /// `Int`, `notes_data.Student`, `List of Int`, `Map of (Text, Int)` (§15.1).
    Named { module: Vec<Ident>, name: Ident, args: Vec<TypeExpr> },
    /// `maybe T` (§7.3).
    Maybe(Box<TypeExpr>),
    /// `A or B` (§7.3).
    Union(Vec<TypeExpr>),
    /// `(Text, Int)`.
    Tuple(Vec<TypeExpr>),
    /// `fun(Int, Text) in Bool` (§7.2, D63).
    Fun { params: Vec<TypeExpr>, ret: Option<Box<TypeExpr>> },
}
