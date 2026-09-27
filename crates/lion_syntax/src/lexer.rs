//! Turns source text into tokens (spec §4 and §5.1).
//!
//! Line ends are significant because they end statements. The lexer emits one
//! [`TokenKind::Newline`] per group of line ends, and none while a `(`, `[` or `{` is
//! open, which is how an expression continues on the next line (§5.1, D52).
//!
//! A text literal becomes `TextStart`, chunks, interpolations (`InterpStart`, ordinary
//! tokens, `InterpEnd`) and `TextEnd`, so the parser reads the expressions inside
//! `{...}` like any other code.

use lion_diagnostics::{Diagnostic, SourceId, Span};

use crate::token::{Keyword, Token, TokenKind};

/// The tokens of one file, always ending with [`TokenKind::Eof`], and the problems found.
pub struct Lexed {
    pub tokens: Vec<Token>,
    pub diagnostics: Vec<Diagnostic>,
    /// The byte ranges of the `/* ... */` comments, for the formatter.
    pub block_comments: Vec<(usize, usize)>,
}

pub fn lex(source: SourceId, text: &str) -> Lexed {
    let mut lexer = Lexer {
        source,
        text,
        pos: 0,
        tokens: Vec::new(),
        diagnostics: Vec::new(),
        frames: vec![Frame::Code { depth: 0 }],
        counted: (0, 1),
        block_comments: Vec::new(),
    };
    lexer.run();
    Lexed { tokens: lexer.tokens, diagnostics: lexer.diagnostics, block_comments: lexer.block_comments }
}

/// What the lexer is inside of. The bottom frame is always `Code`.
enum Frame {
    /// Ordinary code; `depth` counts the brackets opened and not yet closed.
    Code { depth: usize },
    /// The characters of a text literal whose opening `"` is at byte `open`.
    Text { open: usize },
    /// The code between `{` and `}` inside a text literal.
    Interpolation { depth: usize },
}

struct Lexer<'a> {
    source: SourceId,
    text: &'a str,
    pos: usize,
    tokens: Vec<Token>,
    diagnostics: Vec<Diagnostic>,
    frames: Vec<Frame>,
    /// A byte offset and its line, from which the line of the next token is counted.
    counted: (usize, u32),
    block_comments: Vec<(usize, usize)>,
}

impl Lexer<'_> {
    fn run(&mut self) {
        // A UTF-8 byte order mark carries no meaning.
        if self.text.starts_with('\u{feff}') {
            self.pos = '\u{feff}'.len_utf8();
        }
        while self.pos < self.text.len() {
            if matches!(self.frames.last(), Some(Frame::Text { .. })) {
                self.text_step();
            } else {
                self.code_step();
            }
        }
        let end = self.text.len();
        self.interrupt_text(end);
        self.push(TokenKind::Eof, end, end);
    }

    fn code_step(&mut self) {
        let start = self.pos;
        let c = self.peek().expect("not at the end");
        match c {
            ' ' | '\t' | '\r' => self.pos += 1,
            '\n' => {
                self.pos += 1;
                self.line_end(start);
            }
            '/' if self.peek_second() == Some('/') => self.skip_line_comment(),
            '/' if self.peek_second() == Some('*') => self.block_comment(),
            '"' => {
                self.pos += 1;
                self.push(TokenKind::TextStart, start, self.pos);
                self.frames.push(Frame::Text { open: start });
            }
            '0'..='9' => self.number(),
            c if c.is_alphabetic() || c == '_' => self.word(),
            c => self.symbol(c),
        }
    }

    /// A line end at byte `at`: it interrupts any unclosed text literal, and ends the
    /// statement unless a bracket is open.
    fn line_end(&mut self, at: usize) {
        self.interrupt_text(at);
        let [Frame::Code { depth: 0 }] = self.frames[..] else { return };
        if !matches!(self.tokens.last().map(|t| &t.kind), None | Some(TokenKind::Newline)) {
            self.push(TokenKind::Newline, at, at + 1);
        }
    }

    /// Closes every text literal and interpolation still open, as a text cannot span
    /// lines, and reports the innermost literal as unterminated.
    fn interrupt_text(&mut self, at: usize) {
        let innermost = self.frames.iter().rev().find_map(|frame| match frame {
            Frame::Text { open } => Some(*open),
            _ => None,
        });
        let Some(open) = innermost else { return };
        self.diagnostics.push(
            Diagnostic::error("unterminated text")
                .with_primary(self.span(open, open + 1), "this text is never closed")
                .with_help(
                    "close the text with `\"` on the same line; write `\\n` for a line break inside a text",
                ),
        );
        while self.frames.len() > 1 {
            let kind = match self.frames.pop() {
                Some(Frame::Text { .. }) => TokenKind::TextEnd,
                _ => TokenKind::InterpEnd,
            };
            self.push(kind, at, at);
        }
    }

    fn text_step(&mut self) {
        let chunk_start = self.pos;
        let mut chunk = String::new();
        while let Some(c) = self.peek() {
            let start = self.pos;
            match c {
                '"' => {
                    self.flush_chunk(chunk, chunk_start);
                    self.pos += 1;
                    self.push(TokenKind::TextEnd, start, self.pos);
                    self.frames.pop();
                    return;
                }
                '{' => {
                    self.flush_chunk(chunk, chunk_start);
                    self.pos += 1;
                    self.push(TokenKind::InterpStart, start, self.pos);
                    self.frames.push(Frame::Interpolation { depth: 0 });
                    return;
                }
                '\n' => {
                    self.flush_chunk(chunk, chunk_start);
                    self.pos += 1;
                    self.line_end(start);
                    return;
                }
                '\\' => self.escape(&mut chunk),
                '}' => {
                    self.pos += 1;
                    self.diagnostics.push(
                        Diagnostic::error("unescaped `}` in a text")
                            .with_primary(self.span(start, self.pos), "")
                            .with_help("write `\\}` for a literal brace; `{` and `}` delimit interpolations"),
                    );
                    chunk.push('}');
                }
                c => {
                    self.pos += c.len_utf8();
                    chunk.push(c);
                }
            }
        }
        // End of file: `run` reports the unterminated text.
        self.flush_chunk(chunk, chunk_start);
    }

    fn flush_chunk(&mut self, chunk: String, start: usize) {
        if !chunk.is_empty() {
            self.push(TokenKind::TextChunk(chunk), start, self.pos);
        }
    }

    /// An escape sequence inside a text (§4.5, D60).
    fn escape(&mut self, chunk: &mut String) {
        let start = self.pos;
        self.pos += 1;
        let value = match self.peek() {
            Some('n') => '\n',
            Some('t') => '\t',
            Some('\\') => '\\',
            Some('"') => '"',
            Some('{') => '{',
            Some('}') => '}',
            // Leave the line end or the end of file to `text_step`.
            None | Some('\n') => return,
            Some(other) => {
                self.pos += other.len_utf8();
                self.diagnostics.push(
                    Diagnostic::error(format!("unknown escape sequence `\\{other}`"))
                        .with_primary(self.span(start, self.pos), "")
                        .with_note("the escapes are `\\n`, `\\t`, `\\\\`, `\\\"`, `\\{` and `\\}`"),
                );
                return;
            }
        };
        self.pos += 1;
        chunk.push(value);
    }

    fn skip_line_comment(&mut self) {
        self.pos = self.text[self.pos..].find('\n').map_or(self.text.len(), |i| self.pos + i);
    }

    /// `/* ... */`, not nested (§4.3, D58). A comment spanning lines ends the statement,
    /// like the line ends it contains.
    fn block_comment(&mut self) {
        let start = self.pos;
        match self.text[start + 2..].find("*/") {
            Some(length) => {
                self.pos = start + 2 + length + 2;
                self.block_comments.push((start, self.pos));
                if self.text[start..self.pos].contains('\n') {
                    self.line_end(start);
                }
            }
            None => {
                self.pos = self.text.len();
                self.diagnostics.push(
                    Diagnostic::error("unterminated comment")
                        .with_primary(self.span(start, start + 2), "this comment is never closed")
                        .with_help("close it with `*/`"),
                );
            }
        }
    }

    /// A name or keyword (§4.1, §4.2).
    fn word(&mut self) {
        let start = self.pos;
        self.eat_while(|c| c.is_alphanumeric() || c == '_');
        let word = &self.text[start..self.pos];
        if let Some(bad) = word.chars().find(|c| !c.is_ascii()) {
            self.diagnostics.push(
                Diagnostic::error(format!("the name `{word}` contains the non-ASCII character `{bad}`"))
                    .with_primary(self.span(start, self.pos), "")
                    .with_note("names use the letters a-z and A-Z, digits and `_`; other characters may appear only in texts and comments (§4.1)"),
            );
        } else if word.starts_with('_') {
            self.diagnostics.push(
                Diagnostic::error("a name must start with a letter")
                    .with_primary(self.span(start, self.pos), "")
                    .with_note("a name is a letter followed by letters, digits or `_` (§4.1)"),
            );
        }
        let kind = if let Some(keyword) = Keyword::from_word(word) {
            TokenKind::Keyword(keyword)
        } else if word.starts_with(char::is_uppercase) {
            TokenKind::UpperIdent(word.to_string())
        } else {
            TokenKind::LowerIdent(word.to_string())
        };
        self.push(kind, start, self.pos);
    }

    fn number(&mut self) {
        let start = self.pos;
        let bytes = self.text.as_bytes();
        if bytes[start] == b'0' && matches!(bytes.get(start + 1), Some(b'x' | b'b')) {
            self.radix_number(start);
        } else {
            self.decimal_number(start);
        }
    }

    /// `0xFF` or `0b1010`; `_` may separate digits (§4.5, D43, D59).
    fn radix_number(&mut self, start: usize) {
        let (radix, name) =
            if self.text.as_bytes()[start + 1] == b'x' { (16, "hexadecimal") } else { (2, "binary") };
        self.pos = start + 2;
        self.eat_while(|c| c.is_alphanumeric() || c == '_');
        let digits = &self.text[start + 2..self.pos];
        let span = self.span(start, self.pos);
        let error = if let Some(bad) = digits.chars().find(|&c| c != '_' && !c.is_digit(radix)) {
            Some(Diagnostic::error(format!("invalid digit `{bad}` in a {name} number")))
        } else if !digits.starts_with(|c: char| c.is_digit(radix)) {
            Some(Diagnostic::error(format!(
                "a {name} number needs a digit after `{}`",
                &self.text[start..start + 2]
            )))
        } else {
            None
        };
        if let Some(error) = error {
            self.diagnostics.push(error.with_primary(span, ""));
            self.push(TokenKind::Int(0), start, self.pos);
            return;
        }
        let clean: String = digits.chars().filter(|&c| c != '_').collect();
        let value = u64::from_str_radix(&clean, radix).ok().and_then(|v| i64::try_from(v).ok());
        self.push_int(value, start);
    }

    /// `42`, `1_000`, `3.14` or `2.5e-3` (§4.5, D59).
    fn decimal_number(&mut self, start: usize) {
        self.eat_while(|c| c.is_ascii_digit() || c == '_');
        let mut is_float = false;
        if self.peek() == Some('.') && self.peek_second().is_some_and(|c| c.is_ascii_digit()) {
            is_float = true;
            self.pos += 1;
            self.eat_while(|c| c.is_ascii_digit() || c == '_');
            let rest = &self.text.as_bytes()[self.pos..];
            let exponent_length = match rest {
                [b'e' | b'E', b'0'..=b'9', ..] => Some(1),
                [b'e' | b'E', b'+' | b'-', b'0'..=b'9', ..] => Some(2),
                _ => None,
            };
            if let Some(length) = exponent_length {
                self.pos += length;
                self.eat_while(|c| c.is_ascii_digit());
            }
        }
        let literal_end = self.pos;
        // Letters or digits glued to a number make it malformed: `1e5`, `12abc`.
        if self.peek().is_some_and(|c| c.is_alphanumeric() || c == '_') {
            self.eat_while(|c| c.is_alphanumeric() || c == '_');
            self.malformed_number(start, literal_end, is_float);
            return;
        }
        let clean: String = self.text[start..literal_end].chars().filter(|&c| c != '_').collect();
        if !is_float {
            self.push_int(clean.parse().ok(), start);
            return;
        }
        let value: f64 = clean.parse().expect("the lexer checked the syntax of the Float");
        if value.is_infinite() {
            self.diagnostics.push(
                Diagnostic::error("this number is too large for a Float")
                    .with_primary(self.span(start, self.pos), "")
                    .with_note("a Float is a 64-bit IEEE 754 number; the largest is about 1.8e308 (§8.2)"),
            );
            self.push(TokenKind::Float(0.0), start, self.pos);
        } else {
            self.push(TokenKind::Float(value), start, self.pos);
        }
    }

    fn malformed_number(&mut self, start: usize, literal_end: usize, is_float: bool) {
        let whole = &self.text[start..self.pos];
        let suffix = &self.text[literal_end..self.pos];
        let mut error = Diagnostic::error(format!("invalid number `{whole}`"))
            .with_primary(self.span(start, self.pos), "");
        if !is_float && suffix.starts_with(['e', 'E']) {
            let mantissa = &self.text[start..literal_end];
            error = error.with_help(format!(
                "a Float has a digit on each side of the point: write `{mantissa}.0{suffix}`"
            ));
        } else {
            error = error.with_note(
                "numbers are written like `42`, `1_000`, `0xFF`, `0b1010`, `3.14` or `2.5e-3` (§4.5)",
            );
        }
        self.diagnostics.push(error);
        let kind = if is_float { TokenKind::Float(0.0) } else { TokenKind::Int(0) };
        self.push(kind, start, self.pos);
    }

    fn push_int(&mut self, value: Option<i64>, start: usize) {
        match value {
            Some(value) => self.push(TokenKind::Int(value), start, self.pos),
            None => {
                self.diagnostics.push(
                    Diagnostic::error("this number is too large for an Int")
                        .with_primary(self.span(start, self.pos), "")
                        .with_note(
                            "an Int is a 64-bit signed integer; the largest is 9223372036854775807 (§8.1)",
                        ),
                );
                self.push(TokenKind::Int(0), start, self.pos);
            }
        }
    }

    fn symbol(&mut self, c: char) {
        use TokenKind::*;
        let start = self.pos;
        let (kind, length) = match (c, self.peek_second()) {
            ('+', Some('=')) => (PlusAssign, 2),
            ('+', _) => (Plus, 1),
            ('-', Some('=')) => (MinusAssign, 2),
            ('-', _) => (Minus, 1),
            ('*', Some('=')) => (StarAssign, 2),
            ('*', _) => (Star, 1),
            ('/', _) => (Slash, 1),
            ('^', _) => (Caret, 1),
            ('=', Some('=')) => (EqEq, 2),
            ('=', _) => (Assign, 1),
            ('!', Some('=')) => (NotEq, 2),
            ('<', Some('=')) => (Le, 2),
            ('<', _) => (Lt, 1),
            ('>', Some('=')) => (Ge, 2),
            ('>', _) => (Gt, 1),
            ('.', Some('.')) => (DotDot, 2),
            ('.', _) => (Dot, 1),
            (',', _) => (Comma, 1),
            (':', _) => (Colon, 1),
            (';', _) => (Semicolon, 1),
            ('(', _) => (LParen, 1),
            (')', _) => (RParen, 1),
            ('[', _) => (LBracket, 1),
            (']', _) => (RBracket, 1),
            ('{', _) => (LBrace, 1),
            ('}', _) => (RBrace, 1),
            _ => {
                self.pos += c.len_utf8();
                self.unexpected_character(c, start);
                return;
            }
        };
        let kind = self.track_brackets(kind);
        self.pos += length;
        self.push(kind, start, self.pos);
    }

    /// Counts open brackets; a `}` closing an interpolation becomes `InterpEnd`.
    fn track_brackets(&mut self, kind: TokenKind) -> TokenKind {
        let frame = self.frames.last_mut().expect("the code frame is never popped");
        if kind == TokenKind::RBrace && matches!(frame, Frame::Interpolation { depth: 0 }) {
            self.frames.pop();
            return TokenKind::InterpEnd;
        }
        let (Frame::Code { depth } | Frame::Interpolation { depth }) = frame else {
            unreachable!("symbols are not lexed inside text characters")
        };
        match kind {
            TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => *depth += 1,
            TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => *depth = depth.saturating_sub(1),
            _ => {}
        }
        kind
    }

    fn unexpected_character(&mut self, c: char, start: usize) {
        let message = if c.is_ascii() {
            format!("unexpected character `{c}`")
        } else {
            format!("non-ASCII character `{c}` outside a text or comment")
        };
        let mut error = Diagnostic::error(message).with_primary(self.span(start, self.pos), "");
        if let Some(help) = character_help(c) {
            error = error.with_help(help);
        } else if !c.is_ascii() {
            error = error.with_note(
                "Lion code is written in ASCII; other characters may appear only in texts and comments (§4.1)",
            );
        }
        self.diagnostics.push(error);
    }

    fn peek(&self) -> Option<char> {
        self.text[self.pos..].chars().next()
    }

    fn peek_second(&self) -> Option<char> {
        let mut chars = self.text[self.pos..].chars();
        chars.next();
        chars.next()
    }

    fn eat_while(&mut self, accept: impl Fn(char) -> bool) {
        while let Some(c) = self.peek().filter(|&c| accept(c)) {
            self.pos += c.len_utf8();
        }
    }

    fn span(&self, start: usize, end: usize) -> Span {
        Span::new(self.source, start, end)
    }

    fn push(&mut self, kind: TokenKind, start: usize, end: usize) {
        let span = self.span(start, end);
        let line_start = self.text[..start].rfind('\n').map_or(0, |i| i + 1);
        let before = &self.text[line_start..start];
        let starts_line = before.chars().all(|c| c == ' ' || c == '\t' || c == '\u{feff}');
        let mut indent = 0;
        for c in self.text[line_start..].chars() {
            match c {
                ' ' => indent += 1,
                '\t' => indent = (indent / 4 + 1) * 4,
                _ => break,
            }
        }
        // Tokens come in increasing order, so lines are counted incrementally.
        let (from, line) = self.counted;
        let line = line + self.text[from..start].matches('\n').count() as u32;
        self.counted = (start, line);
        self.tokens.push(Token { kind, span, indent, starts_line, line });
    }
}

/// Suggestions for characters that other languages or mathematics use (§4.1, D57).
fn character_help(c: char) -> Option<&'static str> {
    Some(match c {
        '!' => "use `not` for negation and `!=` for inequality",
        '%' => "use `mod` for the remainder of a division (§8.1)",
        '&' => "use `and`",
        '|' => "use `or`",
        '\'' | '“' | '”' | '«' | '»' => "texts are written between ASCII double quotes: \"...\"",
        '#' => "comments start with `//`",
        '∈' => "write `in`",
        '∉' => "write `not x in S`",
        '≤' => "write `<=`",
        '≥' => "write `>=`",
        '≠' => "write `!=`",
        '×' | '·' => "write `*`",
        '÷' => "write `/`",
        '−' => "write `-` (the ASCII minus sign)",
        '∪' => "write `union`",
        '∩' => "write `inter`",
        '⊂' | '⊆' => "write `subset`",
        '¬' => "write `not`",
        '∧' => "write `and`",
        '∨' => "write `or`",
        '→' | '←' => "assign a value with `=`",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use lion_diagnostics::SourceMap;

    use super::*;

    fn kinds(text: &str) -> Vec<TokenKind> {
        let mut map = SourceMap::new();
        let id = map.add("t.lion", text);
        let lexed = lex(id, text);
        assert!(lexed.diagnostics.is_empty(), "{:?}", lexed.diagnostics);
        lexed.tokens.into_iter().map(|t| t.kind).collect()
    }

    fn errors(text: &str) -> Vec<String> {
        let mut map = SourceMap::new();
        let id = map.add("t.lion", text);
        lex(id, text).diagnostics.into_iter().map(|d| d.message).collect()
    }

    fn lower(name: &str) -> TokenKind {
        TokenKind::LowerIdent(name.to_string())
    }

    #[test]
    fn indentation_of_tokens() {
        let mut map = SourceMap::new();
        let text = "if a:\n    b = 1\n\t c\n;";
        let id = map.add("t.lion", text);
        let tokens = lex(id, text).tokens;
        let summary: Vec<(u32, bool)> = tokens.iter().map(|t| (t.indent, t.starts_line)).collect();
        assert_eq!(
            summary,
            [
                (0, true),
                (0, false),
                (0, false),
                (0, false), // if a : newline
                (4, true),
                (4, false),
                (4, false),
                (4, false), // b = 1 newline
                (5, true),
                (5, false), // c newline
                (0, true),
                (0, false), // ; eof
            ]
        );
        let lines: Vec<u32> = tokens.iter().map(|t| t.line).collect();
        assert_eq!(lines, [1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 4, 4]);
    }

    #[test]
    fn let_statement() {
        use TokenKind::*;
        assert_eq!(
            kinds("let x = 42\n"),
            vec![Keyword(crate::Keyword::Let), lower("x"), Assign, Int(42), Newline, Eof]
        );
    }

    #[test]
    fn newlines_are_collapsed_and_ignored_inside_brackets() {
        use TokenKind::*;
        assert_eq!(kinds("\n\na\n\n\nb"), vec![lower("a"), Newline, lower("b"), Eof]);
        assert_eq!(
            kinds("f(a,\n  b)\n"),
            vec![lower("f"), LParen, lower("a"), Comma, lower("b"), RParen, Newline, Eof]
        );
    }

    #[test]
    fn numbers() {
        use TokenKind::*;
        assert_eq!(kinds("1_000_000"), vec![Int(1_000_000), Eof]);
        assert_eq!(kinds("0xFF 0b1010"), vec![Int(255), Int(10), Eof]);
        assert_eq!(kinds("2.75 2.5e-3 1.0E+2"), vec![Float(2.75), Float(2.5e-3), Float(100.0), Eof]);
        assert_eq!(kinds("9223372036854775807"), vec![Int(i64::MAX), Eof]);
        // `1..9` is a range, not the Float `1.`.
        assert_eq!(kinds("1..9"), vec![Int(1), DotDot, Int(9), Eof]);
        assert_eq!(kinds("x.abs"), vec![lower("x"), Dot, lower("abs"), Eof]);
    }

    #[test]
    fn malformed_numbers() {
        assert_eq!(errors("9223372036854775808"), ["this number is too large for an Int"]);
        assert_eq!(errors("0x8000000000000000"), ["this number is too large for an Int"]);
        assert_eq!(errors("1e5"), ["invalid number `1e5`"]);
        assert_eq!(errors("12abc"), ["invalid number `12abc`"]);
        assert_eq!(errors("0b102"), ["invalid digit `2` in a binary number"]);
        assert_eq!(errors("0x"), ["a hexadecimal number needs a digit after `0x`"]);
        assert_eq!(errors("1.0e999"), ["this number is too large for a Float"]);
    }

    #[test]
    fn keywords_and_case() {
        use TokenKind::*;
        assert_eq!(
            kinds("let Student none self"),
            vec![
                Keyword(crate::Keyword::Let),
                UpperIdent("Student".to_string()),
                Keyword(crate::Keyword::NoneValue),
                Keyword(crate::Keyword::SelfValue),
                Eof
            ]
        );
    }

    #[test]
    fn operators() {
        use TokenKind::*;
        assert_eq!(
            kinds("+ += - -= * *= / ^ == != < <= > >= = .. . , : ;"),
            vec![
                Plus,
                PlusAssign,
                Minus,
                MinusAssign,
                Star,
                StarAssign,
                Slash,
                Caret,
                EqEq,
                NotEq,
                Lt,
                Le,
                Gt,
                Ge,
                Assign,
                DotDot,
                Dot,
                Comma,
                Colon,
                Semicolon,
                Eof
            ]
        );
    }

    #[test]
    fn comments() {
        use TokenKind::*;
        assert_eq!(kinds("a // note\nb /// doc\n"), vec![lower("a"), Newline, lower("b"), Newline, Eof]);
        assert_eq!(kinds("a /* x */ b"), vec![lower("a"), lower("b"), Eof]);
        // A comment spanning lines ends the statement.
        assert_eq!(kinds("a /* x\n y */ b"), vec![lower("a"), Newline, lower("b"), Eof]);
        assert_eq!(errors("a /* never closed"), ["unterminated comment"]);
    }

    #[test]
    fn texts_and_interpolation() {
        use TokenKind::*;
        assert_eq!(kinds("\"\""), vec![TextStart, TextEnd, Eof]);
        assert_eq!(kinds("\"a\\n\\{b\\}\""), vec![TextStart, TextChunk("a\n{b}".to_string()), TextEnd, Eof]);
        assert_eq!(
            kinds("\"x = {x + 1}!\""),
            vec![
                TextStart,
                TextChunk("x = ".to_string()),
                InterpStart,
                lower("x"),
                Plus,
                Int(1),
                InterpEnd,
                TextChunk("!".to_string()),
                TextEnd,
                Eof
            ]
        );
        // A text inside an interpolation, and a set literal inside an interpolation.
        assert_eq!(
            kinds("\"{f(\"a\")}{ {1} }\""),
            vec![
                TextStart,
                InterpStart,
                lower("f"),
                LParen,
                TextStart,
                TextChunk("a".to_string()),
                TextEnd,
                RParen,
                InterpEnd,
                InterpStart,
                LBrace,
                Int(1),
                RBrace,
                InterpEnd,
                TextEnd,
                Eof
            ]
        );
    }

    #[test]
    fn text_errors() {
        assert_eq!(errors("\"abc\nx"), ["unterminated text"]);
        assert_eq!(errors("\"a {b\nx"), ["unterminated text"]);
        assert_eq!(errors("\"\\q\""), ["unknown escape sequence `\\q`"]);
        assert_eq!(errors("\"}\""), ["unescaped `}` in a text"]);
    }

    #[test]
    fn unterminated_text_keeps_tokens_balanced() {
        use TokenKind::*;
        let mut map = SourceMap::new();
        let id = map.add("t.lion", "\"a {b\nc");
        let kinds: Vec<_> = lex(id, "\"a {b\nc").tokens.into_iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                TextStart,
                TextChunk("a ".to_string()),
                InterpStart,
                lower("b"),
                InterpEnd,
                TextEnd,
                Newline,
                lower("c"),
                Eof
            ]
        );
    }

    #[test]
    fn non_ascii_outside_texts() {
        assert_eq!(errors("let café = 1"), ["the name `café` contains the non-ASCII character `é`"]);
        assert_eq!(errors("a ≤ b"), ["non-ASCII character `≤` outside a text or comment"]);
        assert_eq!(errors("\"é\" // é"), Vec::<String>::new());
        assert_eq!(errors("_x"), ["a name must start with a letter"]);
        assert_eq!(errors("a % b"), ["unexpected character `%`"]);
    }
}
