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
    /// `for x in values: ... ;` (§10.2), or `parallel for` (§19.2).
    For {
        parallel: Option<Span>,
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
    /// `match value: pattern: ... ; ... ;` (§10.3).
    Match {
        scrutinee: Expr,
        cases: Vec<(Case, Block)>,
    },
    /// `struct Name: fields and invariants ;` (§12.1).
    Struct(StructDecl),
    /// `Color = {red, green}`, `Days = [mon, tue]`, `Shape = Circle or Rect` (§13).
    TypeDef(TypeDef),
    /// `use geometry`, `use shapes.circle` (§20.2).
    Use(Vec<Ident>),
    /// `trait Shape: required methods and fields ;` (§14).
    Trait(TraitDecl),
}

/// A trait: the methods and the fields that a type needs to satisfy it (§14).
#[derive(Clone, Debug)]
pub struct TraitDecl {
    pub name: Ident,
    /// Without a body, a method is required; with one, it is a default (§14.2).
    pub methods: Vec<FunDecl>,
    /// Required fields, such as `name in Text` (§14.1).
    pub fields: Vec<(Ident, TypeExpr)>,
}

#[derive(Clone, Debug)]
pub struct TypeDef {
    pub name: Ident,
    pub kind: TypeDefKind,
}

#[derive(Clone, Debug)]
pub enum TypeDefKind {
    /// An enumeration: `{...}`, or `[...]` when its values are ordered (§13.1, D33).
    Enum { ordered: bool, values: Vec<Ident> },
    /// A named union: a closed list of types (§13.2).
    Union(TypeExpr),
}

#[derive(Clone, Debug)]
pub struct StructDecl {
    pub name: Ident,
    /// The fields and the invariants, in the order of the file.
    pub lines: Vec<StructLine>,
}

#[derive(Clone, Debug)]
pub enum StructLine {
    Field(FieldDecl),
    /// A condition on several fields (§12.1).
    Invariant(Condition),
}

/// `[private] name in Type [= default] {, condition}` (§12.1).
#[derive(Clone, Debug)]
pub struct FieldDecl {
    /// `private`: visible only in this file (D10).
    pub private: Option<Span>,
    pub name: Ident,
    pub ty: TypeExpr,
    pub default: Option<Expr>,
    pub conditions: Vec<Condition>,
}

/// A condition that the fields of a structure satisfy, and its text as written, for
/// the messages that report it broken.
#[derive(Clone, Debug)]
pub struct Condition {
    pub expr: Expr,
    pub text: String,
}

/// A case of `match`: a pattern, then conditions that all hold (§10.3).
#[derive(Clone, Debug)]
pub struct Case {
    pub pattern: Pattern,
    /// After the pattern, separated by commas: the comma means `and`.
    pub conditions: Vec<Expr>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum Pattern {
    /// Every remaining value (§10.3, D16).
    Otherwise,
    /// Equal to this value: `42`, `"oui"`, `red`.
    Value(Expr),
    /// Of this type: `in Int g`, where `g` names the narrowed value.
    Type { ty: TypeExpr, binding: Option<Ident> },
    /// In this set of values: `in 1..9`, `in primes` (D76).
    In { set: Expr, binding: Option<Ident> },
}

#[derive(Clone, Debug)]
pub struct FunDecl {
    /// `private fun`: visible only in its file (§20.3, D10).
    pub private: Option<Span>,
    /// `foreign "lion" fun`: provided by the implementation, in the standard library
    /// (§21.2). The text is the one after `foreign`.
    pub foreign: Option<(String, Span)>,
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
    /// A `foreign` function has no body.
    Foreign,
    /// A method that a trait requires, without a default (§14).
    Required,
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
    /// `private let`, `private var`: visible only in its file (§20.3, D10).
    pub private: Option<Span>,
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
    /// `{a, b}`, or a comprehension `{f(x), x in values, condition}` (§16.1, §16.4).
    Set(Vec<Expr>),
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
    /// `match value: pattern then result ... ;` (§10.3, D51).
    Match {
        scrutinee: Box<Expr>,
        cases: Vec<(Case, Expr)>,
    },
    /// `if c then a elif d then b else e` (§10.1).
    If {
        branches: Vec<(Expr, Expr)>,
        otherwise: Option<Box<Expr>>,
    },
    /// `("Léa", 12)`, `(x,)`, `(name: "Léa", grade: 12)` (§4.5, §12.2).
    Tuple(Vec<Element>),
    /// `parallel [f(x), x in l]`: a comprehension computed in parallel (§19.2).
    Parallel(Box<Expr>),
    /// `fun(x in Int) = x * 2`: an anonymous function (§11.1); its name is `fun`.
    Fun(Box<FunDecl>),
}

/// An element of a tuple, named or not (§26: `element`).
#[derive(Clone, Debug)]
pub struct Element {
    pub name: Option<Ident>,
    pub value: Expr,
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
