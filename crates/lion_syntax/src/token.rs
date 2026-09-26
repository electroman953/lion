use lion_diagnostics::Span;

macro_rules! keywords {
    ($($variant:ident => $text:literal,)*) => {
        /// The reserved words of Lion (spec §4.4).
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Keyword {
            $($variant,)*
        }

        impl Keyword {
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Keyword::$variant => $text,)*
                }
            }

            pub fn from_word(word: &str) -> Option<Keyword> {
                match word {
                    $($text => Some(Keyword::$variant),)*
                    _ => None,
                }
            }
        }
    };
}

keywords! {
    Let => "let", Var => "var", Fun => "fun", Return => "return",
    If => "if", Elif => "elif", Else => "else", Then => "then",
    Match => "match", Otherwise => "otherwise",
    For => "for", While => "while", Break => "break", Continue => "continue",
    In => "in", As => "as", And => "and", Or => "or", Not => "not", Same => "same",
    Union => "union", Inter => "inter", Minus => "minus", Subset => "subset",
    Div => "div", Mod => "mod", Over => "over",
    True => "true", False => "false", NoneValue => "none",
    Maybe => "maybe", Of => "of",
    Struct => "struct", Trait => "trait", Use => "use", Private => "private",
    Shared => "shared", Synced => "synced", Task => "task", Wait => "wait",
    Parallel => "parallel", Try => "try", Compile => "compile", Modifies => "modifies",
    Infix => "infix", Foreign => "foreign", Pure => "pure", Unsafe => "unsafe",
    Test => "test", Expect => "expect", SelfValue => "self",
}

#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    /// The end of a statement: one or more line ends outside brackets (§5.1).
    Newline,
    Eof,
    /// A name starting with a lowercase letter: a value (§4.2).
    LowerIdent(String),
    /// A name starting with an uppercase letter: a type (§4.2).
    UpperIdent(String),
    Keyword(Keyword),
    Int(i64),
    Float(f64),
    /// The opening `"` of a text literal.
    TextStart,
    /// Literal characters of a text, with escapes already resolved.
    TextChunk(String),
    /// The `{` starting an interpolation inside a text.
    InterpStart,
    /// The `}` ending an interpolation inside a text.
    InterpEnd,
    /// The closing `"` of a text literal.
    TextEnd,
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    EqEq,
    NotEq,
    Lt,
    Gt,
    Le,
    Ge,
    Assign,
    PlusAssign,
    MinusAssign,
    StarAssign,
    DotDot,
    Dot,
    Comma,
    Colon,
    Semicolon,
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
}

impl TokenKind {
    /// The source text of a symbol or keyword token.
    pub fn symbol(&self) -> Option<&'static str> {
        use TokenKind::*;
        Some(match self {
            Keyword(keyword) => keyword.as_str(),
            Plus => "+",
            Minus => "-",
            Star => "*",
            Slash => "/",
            Caret => "^",
            EqEq => "==",
            NotEq => "!=",
            Lt => "<",
            Gt => ">",
            Le => "<=",
            Ge => ">=",
            Assign => "=",
            PlusAssign => "+=",
            MinusAssign => "-=",
            StarAssign => "*=",
            DotDot => "..",
            Dot => ".",
            Comma => ",",
            Colon => ":",
            Semicolon => ";",
            LParen => "(",
            RParen => ")",
            LBracket => "[",
            RBracket => "]",
            LBrace | InterpStart => "{",
            RBrace | InterpEnd => "}",
            _ => return None,
        })
    }

    /// How the token is named in error messages.
    pub fn describe(&self) -> String {
        match self {
            TokenKind::Newline => "end of line".to_string(),
            TokenKind::Eof => "end of file".to_string(),
            TokenKind::LowerIdent(name) => format!("name `{name}`"),
            TokenKind::UpperIdent(name) => format!("type name `{name}`"),
            TokenKind::Int(_) | TokenKind::Float(_) => "a number".to_string(),
            TokenKind::TextStart | TokenKind::TextChunk(_) => "a text".to_string(),
            TokenKind::TextEnd => "the end of the text".to_string(),
            other => format!("`{}`", other.symbol().unwrap_or("?")),
        }
    }

    /// The form used by `lion debug tokens`.
    pub fn dump(&self) -> String {
        match self {
            TokenKind::Newline => "newline".to_string(),
            TokenKind::Eof => "eof".to_string(),
            TokenKind::LowerIdent(name) => format!("lower {name}"),
            TokenKind::UpperIdent(name) => format!("upper {name}"),
            TokenKind::Keyword(keyword) => format!("keyword {}", keyword.as_str()),
            TokenKind::Int(value) => format!("int {value}"),
            TokenKind::Float(value) => format!("float {value:?}"),
            TokenKind::TextStart => "text-start".to_string(),
            TokenKind::TextChunk(text) => format!("text-chunk \"{}\"", escape_text(text)),
            TokenKind::InterpStart => "interp-start".to_string(),
            TokenKind::InterpEnd => "interp-end".to_string(),
            TokenKind::TextEnd => "text-end".to_string(),
            other => other.symbol().unwrap_or("?").to_string(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
    /// The indentation width of the token's line (a tab counts up to the next multiple
    /// of 4). Indentation never changes the meaning of a program; it only helps to
    /// locate a forgotten `;` (§5.3).
    pub indent: u32,
    /// Whether the token is the first of its line.
    pub starts_line: bool,
    /// The 1-based line of the start of the token.
    pub line: u32,
}

/// Writes text content with Lion escapes, as it would appear between quotes.
pub fn escape_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            c => out.push(c),
        }
    }
    out
}
