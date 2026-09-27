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
pub use types::{EnumRef, StructRef, TupleRef, Type, TypeRef, UnionRef};

use lion_diagnostics::Span;

/// A checked program: its functions, one of which is the script itself.
pub struct Program {
    pub functions: Vec<Function>,
    /// The structures, for the names of their fields.
    pub structs: Vec<StructDef>,
    /// The enumerations, for the names of their values.
    pub enums: Vec<EnumRef>,
    /// For each file, the function that gives its globals their values, if it has some:
    /// it runs once, before the first use of the module (§20.2, D81).
    pub module_inits: Vec<Option<FunctionId>>,
    /// The top-level statements of the file that is run (§20.1). The locals declared
    /// at its top level are the globals, which other functions reach with `Global`.
    pub main: FunctionId,
}

impl Program {
    pub fn function(&self, id: FunctionId) -> &Function {
        &self.functions[id.index()]
    }

    pub fn structure(&self, id: StructRef) -> &StructDef {
        self.structs.iter().find(|def| def.id == id).expect("every structure of a program is described")
    }
}

/// A structure (§12): its fields, in order.
pub struct StructDef {
    pub id: StructRef,
    pub name: String,
    pub fields: Vec<(String, Type)>,
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

/// One step from a value to a part of it.
#[derive(Clone)]
pub enum Step {
    /// An element of a List, from 1 (§16.2).
    Index(Expr),
    /// A field of a structure, by its position.
    Field(u32),
}

/// Where an assignment stores its value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Local(LocalId),
    /// A top-level variable of the script, from another function.
    Global(LocalId),
}

#[derive(Clone)]
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
    /// Statements in order, without a block of their own.
    Seq(Vec<Stmt>),
    /// `l[i] = value`, `s.grade = value`, `l[i].name = value`: replaces a part of the
    /// variable `root`, found by following the path (§6.3).
    AssignElement {
        root: Place,
        path: Vec<Step>,
        value: Expr,
    },
    /// `l.add(value)`, `s.notes.add(value)`: adds at the end of a list inside `root`.
    Add {
        root: Place,
        path: Vec<Step>,
        value: Expr,
    },
    /// Goes through the elements of `iterable`, evaluated once, in `var` (§10.2).
    For {
        var: LocalId,
        iterable: Expr,
        body: Vec<Stmt>,
    },
    /// Leaves the innermost loop.
    Break,
    /// Goes to the next turn of the innermost loop.
    Continue,
    /// Leaves the function with its value (§11.4); in the script, ends the program (§20.1).
    Return(Option<Expr>),
    /// Gives the globals of a module their values, unless it is done (D81).
    InitModule {
        module: u32,
    },
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
    /// `[a, b, c]`.
    List(Vec<Expr>),
    /// `{a, b, c}`: the values without repetition, in the order of their first
    /// appearance (§16.1, C57).
    Set(Vec<Expr>),
    /// `(a, b)` (§4.5).
    Tuple(Vec<Expr>),
    /// Whether the value belongs to `ty`, a set of the value's possible types (§7.1).
    TypeTest {
        value: Box<Expr>,
        ty: Type,
    },
    /// The value, unless it is an Error: then the function returns it at once, or the
    /// script stops (§18.3).
    Try(Box<Expr>),
    /// `l[i]`, from 1 (§16.2); also the character `i` of a Text (D42).
    Index {
        object: Box<Expr>,
        index: Box<Expr>,
    },
    /// `l[a..b]`, bounds included (D41).
    Slice {
        object: Box<Expr>,
        range: Box<Expr>,
    },
    Property {
        object: Box<Expr>,
        property: Property,
    },
    /// Runs statements, then evaluates to `value` (a comprehension, §16.4).
    Block {
        stmts: Vec<Stmt>,
        value: Box<Expr>,
    },
    /// `start..end`: the integers from `start` to `end` included (§16.3).
    Range {
        start: Box<Expr>,
        end: Box<Expr>,
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
    /// A value of a structure, from the values of its fields in order (§12.2). The
    /// invariants are not checked here: the front end checks them before.
    Struct {
        structure: StructRef,
        fields: Vec<Expr>,
    },
    /// A field of a structure, by its position (§12.1).
    Field {
        object: Box<Expr>,
        field: u32,
    },
    /// A value of an enumeration, by its position (§13.1).
    Enum {
        enumeration: EnumRef,
        value: u32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Property {
    /// The number of elements of a List, characters of a Text, integers of a Range.
    Size,
    First,
    Last,
}

impl Property {
    pub fn name(self) -> &'static str {
        match self {
            Property::Size => "size",
            Property::First => "first",
            Property::Last => "last",
        }
    }
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
    /// An Int in a Range (§16.3).
    InRange,
    /// A value among the elements of a List, compared with `==` (§16.2).
    InList,
    /// Equality of Lists and Ranges, element by element (§9.4).
    EqValue,
    NeValue,
    /// A value among the elements of a Set (§16.1).
    InSet,
    /// `a union b`, `a inter b`, `a minus b` on Sets, and `a subset b` (§16.6).
    SetUnion,
    SetInter,
    SetMinus,
    Subset,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conversion {
    /// Implicit or `as Float`; may lose precision beyond 2^53 (§8.5).
    IntToFloat,
    /// `as Int`: truncation toward zero (§8.5, D74).
    FloatToInt,
    /// The text of any value, as `show` writes it; also `as Text` on numbers.
    ToText,
    /// `"12" as Int`: an Int, or an Error that says why (§8.5, D14).
    TextToInt,
    /// `"2.5" as Float`: a Float, or an Error that says why (§8.5, D14).
    TextToFloat,
    /// The text of any value as a literal: a Text is written between quotes, as in a
    /// shown collection (C24). For the messages about invariants.
    Literal,
    /// The position of a value of an ordered enumeration, from 0: its order (D33).
    EnumPosition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Builtin {
    /// `show(value)`: writes the value and a line end (§23).
    Show,
    /// `sum(values)`: the sum of a List of Int or of Float (§23).
    Sum,
    /// `error(message)`: a simple Error (§18.2, D65).
    Error,
    /// `e.message()`: the text of an Error (§18.2, D66).
    Message,
    /// Stops the program with a bug: the structure named by the first argument no
    /// longer satisfies its invariants, as the second one says (§12.3, D40).
    Broken,
    /// `ask(prompt)`: writes the prompt, reads a line (§23, D29).
    Ask,
    /// `exit(code)`: stops the program with this exit code (§20.1, D36).
    Exit,
    /// `reverse(l)`: a List or a Text in the opposite order (§23).
    Reverse,
    /// `floor(x)`, `ceil(x)`, `round(x)` of a Float, as an Int (§23).
    Floor,
    Ceil,
    Round,
    /// `isqrt(n)`: the integer square root (§23).
    Isqrt,
    /// A function of the standard library provided by the implementation, declared
    /// `foreign "lion"` (§23).
    Native(Native),
}

/// The functions of the standard modules that the implementation provides (§23).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Native {
    FilesRead,
    FilesWrite,
    FilesExists,
    TextSplit,
    TextJoin,
    TextUpper,
    TextLower,
    TextTrim,
    TextContains,
    TextStartsWith,
    TextEndsWith,
    TextReplace,
    TextFind,
    TextLines,
    MathSqrt,
    MathSin,
    MathCos,
    MathTan,
    MathAsin,
    MathAcos,
    MathAtan,
    MathAtan2,
    MathExp,
    MathLog,
    MathLog10,
    RandomAdvance,
    RandomUnit,
    RandomBelow,
    RandomSeed,
    CsvParse,
}

impl Native {
    const ALL: &[(&str, &str, Native)] = &[
        ("files", "read", Native::FilesRead),
        ("files", "write", Native::FilesWrite),
        ("files", "exists", Native::FilesExists),
        ("text", "split", Native::TextSplit),
        ("text", "join", Native::TextJoin),
        ("text", "upper", Native::TextUpper),
        ("text", "lower", Native::TextLower),
        ("text", "trim", Native::TextTrim),
        ("text", "contains", Native::TextContains),
        ("text", "starts_with", Native::TextStartsWith),
        ("text", "ends_with", Native::TextEndsWith),
        ("text", "replace", Native::TextReplace),
        ("text", "find", Native::TextFind),
        ("text", "lines", Native::TextLines),
        ("math", "sqrt", Native::MathSqrt),
        ("math", "sin", Native::MathSin),
        ("math", "cos", Native::MathCos),
        ("math", "tan", Native::MathTan),
        ("math", "asin", Native::MathAsin),
        ("math", "acos", Native::MathAcos),
        ("math", "atan", Native::MathAtan),
        ("math", "atan2", Native::MathAtan2),
        ("math", "exp", Native::MathExp),
        ("math", "log", Native::MathLog),
        ("math", "log10", Native::MathLog10),
        ("random", "advance", Native::RandomAdvance),
        ("random", "unit", Native::RandomUnit),
        ("random", "below", Native::RandomBelow),
        ("random", "seed", Native::RandomSeed),
        ("csv", "parse", Native::CsvParse),
    ];

    /// The function `name` of the standard module `module`.
    pub fn find(module: &str, name: &str) -> Option<Native> {
        Native::ALL.iter().find(|(m, n, _)| *m == module && *n == name).map(|(_, _, native)| *native)
    }

    /// `module.name`.
    pub fn name(self) -> String {
        let (module, name, _) =
            Native::ALL.iter().find(|(.., native)| *native == self).expect("every native is listed");
        format!("{module}.{name}")
    }
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
            InRange => "in_range",
            InList => "in_list",
            EqValue => "eq_value",
            NeValue => "ne_value",
            InSet => "in_set",
            SetUnion => "union",
            SetInter => "inter",
            SetMinus => "minus",
            Subset => "subset",
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
            Conversion::TextToInt => "text_to_int",
            Conversion::TextToFloat => "text_to_float",
            Conversion::Literal => "literal",
            Conversion::EnumPosition => "enum_position",
        }
    }
}

impl Builtin {
    pub fn name(self) -> &'static str {
        match self {
            Builtin::Show => "show",
            Builtin::Sum => "sum",
            Builtin::Error => "error",
            Builtin::Message => "message",
            Builtin::Broken => "broken",
            Builtin::Ask => "ask",
            Builtin::Exit => "exit",
            Builtin::Reverse => "reverse",
            Builtin::Floor => "floor",
            Builtin::Ceil => "ceil",
            Builtin::Round => "round",
            Builtin::Isqrt => "isqrt",
            Builtin::Native(_) => "native",
        }
    }
}
