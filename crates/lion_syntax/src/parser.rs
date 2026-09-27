//! Builds the syntax tree from tokens, following the grammar of spec §26.
//!
//! Expressions are parsed by recursive descent, one function per precedence level
//! of the table in §9.1, from the weakest (`or`) to the strongest (postfix).
//! Constructions that this version does not support yet are rejected with an explicit
//! "not implemented yet" error naming the spec section, never silently accepted.

use lion_diagnostics::{Diagnostic, Span};

use crate::ast::*;
use crate::token::{Keyword, Token, TokenKind};

pub struct Parsed {
    pub module: Module,
    pub diagnostics: Vec<Diagnostic>,
}

/// Parses the tokens of one file, which must end with [`TokenKind::Eof`]. The text
/// of the file gives the conditions of structures as written, for messages.
pub fn parse(text: &str, tokens: &[Token]) -> Parsed {
    assert!(matches!(tokens.last().map(|t| &t.kind), Some(TokenKind::Eof)));
    let mut parser = Parser {
        text,
        tokens,
        pos: 0,
        diagnostics: Vec::new(),
        annotation_ends_expr: false,
        misaligned: None,
        reported_unclosed: false,
    };
    let module = parser.module();
    Parsed { module, diagnostics: parser.diagnostics }
}

/// The problem has been reported; the caller recovers.
struct Reported;

type PResult<T> = Result<T, Reported>;

struct Parser<'t> {
    text: &'t str,
    tokens: &'t [Token],
    pos: usize,
    diagnostics: Vec<Diagnostic>,
    /// Set while parsing the value of a `let` or `var`: a top-level `in` followed by a
    /// type then starts the type annotation, not a membership test (§6.2, §26 rule 1).
    annotation_ends_expr: bool,
    /// The first block closed by a `;`, `elif` or `else` less indented than the line
    /// that opened it: the likely place of a forgotten `;` (§5.3).
    misaligned: Option<(Opener, usize)>,
    /// Whether a block left open at the end of the file has been reported.
    reported_unclosed: bool,
}

/// The statement that opened a block, for messages about how the block ends.
#[derive(Clone, Copy)]
struct Opener {
    /// `if` or `while`.
    keyword: &'static str,
    /// The token index of that keyword.
    index: usize,
    /// The token index of the keyword starting the current branch (`if`, `elif`, `else`).
    branch: usize,
}

impl<'t> Parser<'t> {
    fn module(&mut self) -> Module {
        let mut stmts = Vec::new();
        loop {
            while self.eat(&TokenKind::Newline) {}
            if self.at(&TokenKind::Eof) {
                return Module { stmts };
            }
            match self.statement().and_then(|stmt| self.end_of_statement().map(|()| stmt)) {
                Ok(stmt) => stmts.push(stmt),
                Err(Reported) => self.skip_statement(),
            }
        }
    }

    // ----- Statements -----

    fn statement(&mut self) -> PResult<Stmt> {
        let start = self.span();
        if let Some((what, section)) = self.unsupported_statement() {
            return Err(self.not_implemented(start, what, section));
        }
        match self.peek() {
            TokenKind::Keyword(Keyword::Let | Keyword::Var) => self.let_statement(),
            TokenKind::Keyword(Keyword::If) => self.if_statement(),
            TokenKind::Keyword(Keyword::While) => self.while_statement(),
            TokenKind::Keyword(Keyword::For) => self.for_statement(),
            TokenKind::Keyword(Keyword::Parallel)
                if self.kind_at(self.pos + 1) == &TokenKind::Keyword(Keyword::For) =>
            {
                self.for_statement()
            }
            TokenKind::Keyword(Keyword::Match) => self.match_statement(),
            TokenKind::Keyword(Keyword::Break) => Ok(Stmt { kind: StmtKind::Break, span: self.bump().span }),
            TokenKind::Keyword(Keyword::Continue) => {
                Ok(Stmt { kind: StmtKind::Continue, span: self.bump().span })
            }
            TokenKind::Keyword(Keyword::Return) => self.return_statement(),
            TokenKind::Keyword(Keyword::Fun | Keyword::Infix | Keyword::Foreign) => self.fun_statement(),
            TokenKind::Keyword(Keyword::Use) => self.use_statement(),
            TokenKind::Keyword(Keyword::Private) => self.private_statement(),
            TokenKind::Keyword(Keyword::Struct) => self.struct_statement(),
            TokenKind::UpperIdent(_) if self.kind_at(self.pos + 1) == &TokenKind::Assign => {
                self.type_definition()
            }
            TokenKind::Keyword(keyword @ (Keyword::Elif | Keyword::Else)) => {
                let error = Diagnostic::error(format!("`{}` without `if`", keyword.as_str()))
                    .with_primary(start, "")
                    .with_note("`elif` and `else` continue the block of an `if` (§5.2)");
                Err(self.error(error))
            }
            TokenKind::Semicolon => Err(self.stray_semicolon()),
            kind if is_operator(kind) => Err(self.error(
                Diagnostic::error("a line cannot start with an operator").with_primary(start, "").with_help(
                    "to continue an expression on the next line, keep it inside parentheses (§5.1)",
                ),
            )),
            _ => self.expr_statement(),
        }
    }

    fn unsupported_statement(&self) -> Option<(&'static str, &'static str)> {
        let TokenKind::Keyword(keyword) = self.peek() else { return None };
        Some(match keyword {
            Keyword::Trait => ("traits", "§14"),
            Keyword::Test | Keyword::Expect => ("tests", "§24.1"),
            Keyword::Unsafe => ("calling C code", "§21.2"),
            _ => return None,
        })
    }

    /// `use geometry`, `use shapes.circle` (§20.2).
    fn use_statement(&mut self) -> PResult<Stmt> {
        let start = self.bump().span;
        let mut path = vec![self.binding_name()?];
        while self.eat(&TokenKind::Dot) {
            path.push(self.binding_name()?);
        }
        let end = path.last().expect("one name").span;
        Ok(Stmt { kind: StmtKind::Use(path), span: start.to(end) })
    }

    /// `private` before a function or a variable of the file (§20.3, D10).
    fn private_statement(&mut self) -> PResult<Stmt> {
        let private = self.span();
        match self.kind_at(self.pos + 1) {
            TokenKind::Keyword(Keyword::Fun | Keyword::Infix | Keyword::Foreign) => {
                self.bump();
                let mut stmt = self.fun_statement()?;
                if let StmtKind::Fun(decl) = &mut stmt.kind {
                    decl.private = Some(private);
                }
                stmt.span = private.to(stmt.span);
                Ok(stmt)
            }
            TokenKind::Keyword(Keyword::Let | Keyword::Var) => {
                self.bump();
                let mut stmt = self.let_statement()?;
                if let StmtKind::Let(decl) = &mut stmt.kind {
                    decl.private = Some(private);
                }
                stmt.span = private.to(stmt.span);
                Ok(stmt)
            }
            _ => {
                let error = Diagnostic::error("`private` goes before a function, a variable or a field")
                    .with_primary(private, "")
                    .with_note("`private` limits a declaration to its file (§20.3)");
                Err(self.error(error))
            }
        }
    }

    /// `let x = value`, `var x = value in T`, `var x in T` (§6.1).
    fn let_statement(&mut self) -> PResult<Stmt> {
        let keyword = self.bump().clone();
        let mutable = keyword.kind == TokenKind::Keyword(Keyword::Var);
        let name = self.binding_name()?;
        let value = if self.eat(&TokenKind::Assign) { Some(self.declaration_value()?) } else { None };
        let annotation = if self.eat_keyword(Keyword::In) { Some(self.type_expr()?) } else { None };
        if value.is_none() && annotation.is_none() {
            let word = if mutable { "var" } else { "let" };
            let error = Diagnostic::error(format!("`{word} {}` needs a value or a type", name.name))
                .with_primary(name.span, "")
                .with_help(format!(
                    "write `{word} {0} = value`, or `{word} {0} in Type` to give it a value later (§6.1)",
                    name.name
                ));
            return Err(self.error(error));
        }
        let span = keyword.span.to(self.previous_span());
        Ok(Stmt { kind: StmtKind::Let(LetStmt { private: None, mutable, name, value, annotation }), span })
    }

    fn binding_name(&mut self) -> PResult<Ident> {
        let span = self.span();
        match self.peek() {
            TokenKind::LowerIdent(name) => {
                let name = name.clone();
                self.bump();
                Ok(Ident { name, span })
            }
            TokenKind::UpperIdent(name) => {
                let error = Diagnostic::error(format!("`{name}` cannot name a value"))
                    .with_primary(span, "names starting with an uppercase letter are types")
                    .with_help(format!("write `{}` (§4.2)", lowercase_first(name)));
                Err(self.error(error))
            }
            TokenKind::Keyword(keyword) => {
                let error = Diagnostic::error(format!("`{}` is a reserved keyword", keyword.as_str()))
                    .with_primary(span, "")
                    .with_help("choose another name (§4.4)");
                Err(self.error(error))
            }
            _ => Err(self.expected("a name")),
        }
    }

    fn declaration_value(&mut self) -> PResult<Expr> {
        let saved = std::mem::replace(&mut self.annotation_ends_expr, true);
        let value = self.expr();
        self.annotation_ends_expr = saved;
        value
    }

    /// `if c: ... elif d: ... else: ... ;`, or an `if ... then ... else` expression
    /// used as a statement (§5.2, §10.1).
    fn if_statement(&mut self) -> PResult<Stmt> {
        let index = self.pos;
        let start = self.bump().span;
        let cond = self.nested(Self::expr)?;
        if self.at_keyword(Keyword::Then) {
            let expr = self.if_expression_rest(start, cond)?;
            return Ok(Stmt { span: expr.span, kind: StmtKind::Expr(expr) });
        }
        let opener = Opener { keyword: "if", index, branch: index };
        let mut branches = vec![Branch { cond, body: self.block(opener)? }];
        let mut otherwise = None;
        while otherwise.is_none() {
            let branch = self.pos;
            match self.peek() {
                TokenKind::Keyword(Keyword::Elif) => {
                    self.check_alignment(opener, branch);
                    self.bump();
                    let cond = self.nested(Self::expr)?;
                    branches.push(Branch { cond, body: self.block(Opener { branch, ..opener })? });
                }
                TokenKind::Keyword(Keyword::Else) => {
                    self.check_alignment(opener, branch);
                    self.bump();
                    otherwise = Some(self.block(Opener { branch, ..opener })?);
                }
                _ => break,
            }
        }
        let end = self.close_block(opener)?;
        Ok(Stmt { kind: StmtKind::If { branches, otherwise }, span: start.to(end) })
    }

    /// `while c: ... ;` (§10.2).
    fn while_statement(&mut self) -> PResult<Stmt> {
        let index = self.pos;
        let start = self.bump().span;
        let cond = self.nested(Self::expr)?;
        let opener = Opener { keyword: "while", index, branch: index };
        let body = self.block(opener)?;
        let end = self.close_block(opener)?;
        Ok(Stmt { kind: StmtKind::While { cond, body }, span: start.to(end) })
    }

    /// `[infix] fun [Type.]name(params) [in T] [, T in Trait] [modifies x, y]`, then
    /// `: body ;` or `= expr` (§11.1, §26).
    fn fun_statement(&mut self) -> PResult<Stmt> {
        let start = self.span();
        let foreign = if self.at_keyword(Keyword::Foreign) {
            let keyword = self.bump().span;
            let TokenKind::TextStart = self.peek() else {
                return Err(
                    self.expected("the text that names where the function comes from, as `foreign \"C\"`")
                );
            };
            let text = self.text()?;
            let ExprKind::Text(parts) = &text.kind else { unreachable!("a text") };
            let [TextPart::Literal(name)] = parts.as_slice() else {
                return Err(self
                    .error(Diagnostic::error("this text has no interpolation").with_primary(text.span, "")));
            };
            let name = name.clone();
            // `pure` says the function changes no state (D48).
            self.eat_keyword(Keyword::Pure);
            Some((name, keyword.to(text.span)))
        } else {
            None
        };
        let index = self.pos;
        let infix = self.eat_keyword(Keyword::Infix);
        if !self.eat_keyword(Keyword::Fun) {
            return Err(self.expected("`fun`"));
        }
        if self.at(&TokenKind::LParen) {
            return Err(self.not_implemented(start.to(self.span()), "anonymous functions", "§11.1"));
        }
        let receiver = match (self.peek(), self.kind_at(self.pos + 1)) {
            (TokenKind::UpperIdent(name), TokenKind::Dot) => {
                let receiver = Ident { name: name.clone(), span: self.span() };
                self.bump();
                self.bump();
                Some(receiver)
            }
            _ => None,
        };
        let name = self.binding_name()?;
        if !self.eat(&TokenKind::LParen) {
            return Err(self.expected("`(` and the parameters"));
        }
        let params = self.nested(Self::params)?;
        self.expect(&TokenKind::RParen, "`)`")?;
        let ret = if self.eat_keyword(Keyword::In) { Some(self.type_expr()?) } else { None };
        let mut type_params = Vec::new();
        while self.eat(&TokenKind::Comma) {
            let span = self.span();
            let TokenKind::UpperIdent(type_name) = self.peek() else {
                return Err(self.expected("a type variable such as `T in Comparable`"));
            };
            let type_name = Ident { name: type_name.clone(), span };
            self.bump();
            if !self.eat_keyword(Keyword::In) {
                return Err(self.expected("`in` and the set of types of the type variable"));
            }
            type_params.push((type_name, self.type_expr()?));
        }
        let mut modifies = Vec::new();
        if self.eat_keyword(Keyword::Modifies) {
            loop {
                modifies.push(self.binding_name()?);
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
        }
        let body = if foreign.is_some() {
            FunBody::Foreign
        } else if self.eat(&TokenKind::Assign) {
            let value = self.expr()?;
            if matches!(
                self.peek(),
                TokenKind::Assign | TokenKind::PlusAssign | TokenKind::MinusAssign | TokenKind::StarAssign
            ) {
                let error =
                    Diagnostic::error("the short form of a function takes a value, not an assignment")
                        .with_primary(self.span(), "")
                        .with_help(format!(
                            "write the body as a block: `fun {}(...): ... ;` (§11.1)",
                            name.name
                        ));
                return Err(self.error(error));
            }
            FunBody::Expr(value)
        } else if self.at(&TokenKind::Colon) {
            let opener = Opener { keyword: "fun", index, branch: index };
            let block = self.block(opener)?;
            self.close_block(opener)?;
            FunBody::Block(block)
        } else {
            return Err(self.expected("`:` and the body of the function, or `=` and its value"));
        };
        let decl = FunDecl {
            private: None,
            foreign,
            infix,
            receiver,
            name,
            params,
            ret,
            type_params,
            modifies,
            body,
        };
        Ok(Stmt { kind: StmtKind::Fun(decl), span: start.to(self.previous_span()) })
    }

    /// `struct Name: fields and invariants ;`, one per line (§12.1, §26).
    fn struct_statement(&mut self) -> PResult<Stmt> {
        let index = self.pos;
        let start = self.bump().span;
        let span = self.span();
        let name = match self.peek() {
            TokenKind::UpperIdent(name) => {
                let name = Ident { name: name.clone(), span };
                self.bump();
                name
            }
            TokenKind::LowerIdent(name) => {
                let error = Diagnostic::error(format!("the structure `{name}` needs an uppercase name"))
                    .with_primary(span, "a structure is a type")
                    .with_help(format!("write `{}` (§4.2)", uppercase_first(name)));
                return Err(self.error(error));
            }
            _ => return Err(self.expected("the name of the structure")),
        };
        if self.at_keyword(Keyword::Of) {
            return Err(self.not_implemented(self.span(), "generic structures", "§15.1"));
        }
        if !self.eat(&TokenKind::Colon) {
            return Err(self.expected("`:` and the fields of the structure"));
        }
        if !self.at_line_end() {
            let error = Diagnostic::error("the fields of a structure go on their own lines")
                .with_primary(self.span(), "")
                .with_help("write one field per line, and `;` alone on the last line (§12.1)");
            return Err(self.error(error));
        }
        let opener = Opener { keyword: "struct", index, branch: index };
        let mut lines = Vec::new();
        loop {
            while self.eat(&TokenKind::Newline) {}
            if self.at(&TokenKind::Semicolon) {
                break;
            }
            if self.at(&TokenKind::Eof) || self.at_declaration() {
                return Err(self.unclosed_block(opener));
            }
            match self.struct_line() {
                Ok(line) => {
                    lines.push(line);
                    self.end_of_line_in_block(opener);
                }
                Err(Reported) => self.skip_statement(),
            }
        }
        let end = self.close_block(opener)?;
        Ok(Stmt { kind: StmtKind::Struct(StructDecl { name, lines }), span: start.to(end) })
    }

    /// `Color = {red, green}`, `Days = [mon, tue]` or `Shape = Circle or Rect` (§13, §26).
    fn type_definition(&mut self) -> PResult<Stmt> {
        let start = self.span();
        let TokenKind::UpperIdent(name) = self.peek() else { unreachable!("checked by the caller") };
        let name = Ident { name: name.clone(), span: start };
        self.bump();
        self.bump();
        let (ordered, close) = match self.peek() {
            TokenKind::LBrace => (false, TokenKind::RBrace),
            TokenKind::LBracket => (true, TokenKind::RBracket),
            _ => {
                let ty = self.type_expr()?;
                let span = start.to(ty.span);
                return Ok(Stmt {
                    kind: StmtKind::TypeDef(TypeDef { name, kind: TypeDefKind::Union(ty) }),
                    span,
                });
            }
        };
        self.bump();
        let values = self.nested(|parser| {
            let mut values = Vec::new();
            loop {
                let span = parser.span();
                match parser.peek() {
                    TokenKind::LowerIdent(value) => {
                        values.push(Ident { name: value.clone(), span });
                        parser.bump();
                    }
                    TokenKind::UpperIdent(value) => {
                        let error = Diagnostic::error(format!("the value `{value}` needs a lowercase name"))
                            .with_primary(span, "the values of an enumeration start with a lowercase letter")
                            .with_help(format!("write `{}` (§13.1)", lowercase_first(value)));
                        return Err(parser.error(error));
                    }
                    _ => return Err(parser.expected("a value of the enumeration")),
                }
                if !parser.eat(&TokenKind::Comma) {
                    return Ok(values);
                }
            }
        })?;
        let what = if ordered { "`,` or `]`" } else { "`,` or `}`" };
        let end = self.expect(&close, what)?;
        let kind = TypeDefKind::Enum { ordered, values };
        Ok(Stmt { kind: StmtKind::TypeDef(TypeDef { name, kind }), span: start.to(end) })
    }

    /// A keyword that starts a declaration or a statement, never a line of a structure.
    fn at_declaration(&self) -> bool {
        matches!(
            self.peek(),
            TokenKind::Keyword(
                Keyword::Fun
                    | Keyword::Struct
                    | Keyword::Let
                    | Keyword::Var
                    | Keyword::If
                    | Keyword::While
                    | Keyword::For
                    | Keyword::Match
                    | Keyword::Return
                    | Keyword::Trait
                    | Keyword::Use
            )
        )
    }

    /// `[private] name in Type [= default] {, condition}`, or a condition on several
    /// fields (§26: `struct_line`).
    fn struct_line(&mut self) -> PResult<StructLine> {
        let private = if self.at_keyword(Keyword::Private) { Some(self.bump().span) } else { None };
        let is_field = matches!(self.peek(), TokenKind::LowerIdent(_))
            && self.kind_at(self.pos + 1) == &TokenKind::Keyword(Keyword::In)
            && (self.type_starts_at(self.pos + 2) || self.kind_at(self.pos + 2) == &TokenKind::LParen);
        if !is_field {
            if private.is_some() {
                return Err(self.expected("a field: `private name in Type`"));
            }
            return Ok(StructLine::Invariant(self.condition()?));
        }
        let name = self.binding_name()?;
        self.bump();
        let ty = self.type_expr()?;
        let default = if self.eat(&TokenKind::Assign) { Some(self.expr()?) } else { None };
        let mut conditions = Vec::new();
        while self.eat(&TokenKind::Comma) {
            conditions.push(self.condition()?);
        }
        Ok(StructLine::Field(FieldDecl { private, name, ty, default, conditions }))
    }

    /// An expression, with its text as written, spaces reduced to one.
    fn condition(&mut self) -> PResult<Condition> {
        let expr = self.expr()?;
        let written = &self.text[expr.span.start as usize..expr.span.end as usize];
        let text = written.split_whitespace().collect::<Vec<_>>().join(" ");
        Ok(Condition { expr, text })
    }

    /// `[var] name [in T] [= default]`, separated by commas (§11.2).
    fn params(&mut self) -> PResult<Vec<Param>> {
        let mut params = Vec::new();
        if self.at(&TokenKind::RParen) {
            return Ok(params);
        }
        loop {
            let var = if self.at_keyword(Keyword::Var) { Some(self.bump().span) } else { None };
            let name = if self.at_keyword(Keyword::SelfValue) {
                Ident { name: "self".to_string(), span: self.bump().span }
            } else {
                self.binding_name()?
            };
            let ty = if self.eat_keyword(Keyword::In) { Some(self.type_expr()?) } else { None };
            let default = if self.eat(&TokenKind::Assign) { Some(self.expr()?) } else { None };
            params.push(Param { var, name, ty, default });
            if !self.eat(&TokenKind::Comma) {
                return Ok(params);
            }
        }
    }

    /// `for x in values: ... ;` (§10.2).
    fn for_statement(&mut self) -> PResult<Stmt> {
        let start = self.span();
        let parallel = if self.at_keyword(Keyword::Parallel) { Some(self.bump().span) } else { None };
        let index = self.pos;
        self.bump();
        let var = self.binding_name()?;
        if !self.eat_keyword(Keyword::In) {
            return Err(self.expected("`in` and the values to go through"));
        }
        let iterable = self.nested(Self::expr)?;
        let opener = Opener { keyword: "for", index, branch: index };
        let body = self.block(opener)?;
        let end = self.close_block(opener)?;
        Ok(Stmt { kind: StmtKind::For { parallel, var, iterable, body }, span: start.to(end) })
    }

    /// `match value:`, then one case per line, each `pattern: body ;`, then `;` (§10.3).
    fn match_statement(&mut self) -> PResult<Stmt> {
        let index = self.pos;
        let start = self.bump().span;
        let scrutinee = self.nested(Self::expr)?;
        let opener = Opener { keyword: "match", index, branch: index };
        self.start_cases()?;
        let mut cases = Vec::new();
        loop {
            while self.eat(&TokenKind::Newline) {}
            if self.at(&TokenKind::Semicolon) {
                break;
            }
            if self.at(&TokenKind::Eof) {
                return Err(self.unclosed_block(opener));
            }
            let case_index = self.pos;
            let case = self.case()?;
            let body = self.block(Opener { branch: case_index, ..opener })?;
            self.close_block(Opener { keyword: "case", index: case_index, branch: case_index })?;
            cases.push((case, body));
            if !self.at_line_end() && !self.at(&TokenKind::Semicolon) {
                return Err(self.expected("the end of the line after the case"));
            }
        }
        let end = self.close_block(opener)?;
        Ok(Stmt { kind: StmtKind::Match { scrutinee, cases }, span: start.to(end) })
    }

    /// `match value:`, then one case per line, each `pattern then result`, then `;` (D51).
    fn match_expression(&mut self) -> PResult<Expr> {
        let index = self.pos;
        let start = self.bump().span;
        let scrutinee = self.nested(Self::expr)?;
        let opener = Opener { keyword: "match", index, branch: index };
        self.start_cases()?;
        let mut cases = Vec::new();
        loop {
            while self.eat(&TokenKind::Newline) {}
            if self.at(&TokenKind::Semicolon) {
                break;
            }
            if self.at(&TokenKind::Eof) {
                return Err(self.unclosed_block(opener));
            }
            let case = self.case()?;
            if !self.eat_keyword(Keyword::Then) {
                return Err(self.expected("`then` and the value of the case"));
            }
            let value = self.nested(Self::expr)?;
            cases.push((case, value));
            if !self.at_line_end() {
                return Err(self.expected("the end of the line after the case"));
            }
        }
        let end = self.close_block(opener)?;
        Ok(Expr { kind: ExprKind::Match { scrutinee: Box::new(scrutinee), cases }, span: start.to(end) })
    }

    /// The `:` and the line end that open the cases of a `match`.
    fn start_cases(&mut self) -> PResult<()> {
        if !self.eat(&TokenKind::Colon) {
            return Err(self.expected("`:` and the cases"));
        }
        if !self.at_line_end() {
            let error = Diagnostic::error("the cases of a `match` start on the next line")
                .with_primary(self.span(), "")
                .with_note("each case of a `match` is on its own line (§10.3)")
                .with_note("inside parentheses or brackets, line ends are ignored (§5.1): give the `match` a name first, `let v = match ...`");
            return Err(self.error(error));
        }
        Ok(())
    }

    /// A pattern, then conditions separated by commas (§10.3, §26 `pattern`).
    fn case(&mut self) -> PResult<Case> {
        let start = self.span();
        let pattern = if self.eat_keyword(Keyword::Otherwise) {
            Pattern::Otherwise
        } else if self.eat_keyword(Keyword::In) {
            if self.type_starts_at(self.pos) {
                let ty = self.type_expr()?;
                Pattern::Type { ty, binding: self.binding()? }
            } else {
                let set = self.nested(Self::as_expr)?;
                Pattern::In { set, binding: self.binding()? }
            }
        } else {
            Pattern::Value(self.nested(Self::unary)?)
        };
        let mut conditions = Vec::new();
        while self.eat(&TokenKind::Comma) {
            conditions.push(self.nested(Self::expr)?);
        }
        Ok(Case { pattern, conditions, span: start.to(self.previous_span()) })
    }

    /// The optional name after `in T` or `in set`.
    fn binding(&mut self) -> PResult<Option<Ident>> {
        match self.peek() {
            TokenKind::LowerIdent(_) => Ok(Some(self.binding_name()?)),
            _ => Ok(None),
        }
    }

    fn return_statement(&mut self) -> PResult<Stmt> {
        let start = self.bump().span;
        if self.at_line_end() || self.at_block_end() {
            return Ok(Stmt { kind: StmtKind::Return(None), span: start });
        }
        let value = self.expr()?;
        Ok(Stmt { span: start.to(value.span), kind: StmtKind::Return(Some(value)) })
    }

    /// `:` and the body of a block: either one statement on the same line, or lines
    /// up to the `;`, `elif` or `else` that ends the body, left to the caller (§5.2).
    fn block(&mut self, opener: Opener) -> PResult<Block> {
        if !self.at(&TokenKind::Colon) {
            return Err(self.expected("`:` to open the block"));
        }
        self.bump();
        if self.at_block_end() {
            let error = Diagnostic::error("a one-line block needs a statement")
                .with_primary(self.span(), "")
                .with_help("an empty block is written with `:` at the end of the line and `;` on the next one (§5.2)");
            return Err(self.error(error));
        }
        if !self.at_line_end() {
            let stmt = self.statement()?;
            if !self.at_block_end() {
                return Err(self.unclosed_one_line_block(opener));
            }
            return Ok(Block { stmts: vec![stmt] });
        }
        let mut stmts = Vec::new();
        loop {
            while self.eat(&TokenKind::Newline) {}
            if self.at_block_end() {
                return Ok(Block { stmts });
            }
            if self.at(&TokenKind::Eof) {
                return Err(self.unclosed_block(opener));
            }
            match self.statement() {
                Ok(stmt) => {
                    stmts.push(stmt);
                    self.end_of_line_in_block(opener);
                }
                Err(Reported) => self.skip_statement(),
            }
        }
    }

    /// After a statement in a block of several lines. A `;` on the same line is reported,
    /// then taken as the end of the block to recover (§26: `line = [statement] NL`).
    fn end_of_line_in_block(&mut self, opener: Opener) {
        match self.peek() {
            TokenKind::Newline | TokenKind::Eof => {}
            TokenKind::Semicolon | TokenKind::Keyword(Keyword::Elif | Keyword::Else) => {
                let found = self.peek().describe();
                let opened = self.line_of(opener.branch);
                let error = Diagnostic::error(format!("{found} must start a new line here"))
                    .with_primary(self.span(), format!("this ends the block opened on line {opened}"))
                    .with_help("a block of several lines ends with `;`, `elif` or `else` at the start of a line; a block on one line is written `if x > 0: show(x) ;` (§5.2)");
                self.error(error);
            }
            _ => {
                let _ = self.end_of_statement();
                self.skip_statement();
            }
        }
    }

    /// The `;` that closes a whole `if` or `while` statement.
    fn close_block(&mut self, opener: Opener) -> PResult<Span> {
        let span = self.span();
        match self.peek() {
            TokenKind::Semicolon => {
                self.check_alignment(opener, self.pos);
                Ok(self.bump().span)
            }
            TokenKind::Keyword(keyword @ (Keyword::Elif | Keyword::Else)) => {
                let keyword = keyword.as_str();
                let note = if opener.keyword == "if" {
                    "an `if` has at most one `else`, which comes last (§5.2)"
                } else {
                    "only an `if` has `elif` and `else` branches (§5.2)"
                };
                let error = Diagnostic::error(format!("unexpected `{keyword}`"))
                    .with_primary(span, "")
                    .with_secondary(self.tokens[opener.index].span, format!("in this `{}`", opener.keyword))
                    .with_note(note);
                Err(self.error(error))
            }
            _ => Err(self.expected("`;` to close the block")),
        }
    }

    fn unclosed_one_line_block(&mut self, opener: Opener) -> Reported {
        let mut error = Diagnostic::error(format!("the one-line `{}` block is not closed", opener.keyword))
            .with_primary(self.span(), "expected `;` here")
            .with_secondary(self.tokens[opener.branch].span, "block opened here");
        error = error.with_help(if self.at_line_end() {
            "a block written on one line ends with `;` on the same line: `if x > 0: show(x) ;`; for several lines, start the block on a new line after `:` (§5.2)"
        } else {
            "a one-line block holds a single statement, followed by `;` (§5.2)"
        });
        self.error(error)
    }

    /// A block still open at the end of the file. Indentation does not change the meaning
    /// of a program, but it shows where the `;` was probably forgotten (§5.3).
    fn unclosed_block(&mut self, opener: Opener) -> Reported {
        if std::mem::replace(&mut self.reported_unclosed, true) {
            return Reported;
        }
        let keyword = opener.keyword;
        let mut error = Diagnostic::error(format!("the `{keyword}` block is never closed"))
            .with_primary(self.tokens[opener.index].span, "this block has no `;`");
        let suspect = self.misaligned.map_or(opener, |(suspect, _)| suspect);
        if let Some(line_start) = self.first_dedented_line(suspect) {
            error =
                error.with_secondary(self.tokens[line_start].span, "`;` probably forgotten before this line");
            if let Some((suspect, closing)) = self.misaligned {
                error = error.with_note(format!(
                    "the `{}` of line {} is closed by the {} of line {}, which is less indented than it",
                    suspect.keyword,
                    self.line_of(suspect.index),
                    self.tokens[closing].kind.describe(),
                    self.line_of(closing)
                ));
            }
        } else {
            error = error.with_help("close the block with `;` on its own line (§5.2)");
        }
        self.error(error)
    }

    /// The first line after the current branch of `opener` that is not more indented
    /// than the line of `opener`, other than the `elif` and `else` of the same `if`.
    fn first_dedented_line(&self, opener: Opener) -> Option<usize> {
        let indent = self.tokens[opener.index].indent;
        for (index, token) in self.tokens.iter().enumerate().skip(opener.branch + 1) {
            if token.kind == TokenKind::Eof {
                return None;
            }
            if !token.starts_line || token.indent > indent {
                continue;
            }
            let own_branch = matches!(token.kind, TokenKind::Keyword(Keyword::Elif | Keyword::Else))
                && opener.keyword == "if"
                && token.indent == indent;
            if !own_branch {
                return Some(index);
            }
        }
        None
    }

    /// Remembers a block whose end is less indented than its start (§5.3).
    fn check_alignment(&mut self, opener: Opener, closing: usize) {
        let (open, close) = (&self.tokens[opener.index], &self.tokens[closing]);
        if close.starts_line && close.indent < open.indent && self.misaligned.is_none() {
            self.misaligned = Some((opener, closing));
        }
    }

    fn expr_statement(&mut self) -> PResult<Stmt> {
        let target = self.expr()?;
        let op = match self.peek() {
            TokenKind::Assign => AssignOp::Set,
            TokenKind::PlusAssign => AssignOp::Add,
            TokenKind::MinusAssign => AssignOp::Sub,
            TokenKind::StarAssign => AssignOp::Mul,
            _ => return Ok(Stmt { span: target.span, kind: StmtKind::Expr(target) }),
        };
        let op_span = self.bump().span;
        let value = self.expr()?;
        let span = target.span.to(value.span);
        Ok(Stmt { kind: StmtKind::Assign { target, op, op_span, value }, span })
    }

    fn end_of_statement(&mut self) -> PResult<()> {
        let span = self.span();
        match self.peek() {
            TokenKind::Newline => {
                self.bump();
                Ok(())
            }
            TokenKind::Eof => Ok(()),
            TokenKind::Semicolon => Err(self.stray_semicolon()),
            TokenKind::Assign => Err(self.error(
                Diagnostic::error("unexpected `=`")
                    .with_primary(span, "")
                    .with_help("to compare two values, use `==`"),
            )),
            other => {
                let message = format!("expected the end of the line, found {}", other.describe());
                Err(self.error(
                    Diagnostic::error(message)
                        .with_primary(span, "")
                        .with_note("a line holds a single statement (§5.1)"),
                ))
            }
        }
    }

    fn at_line_end(&self) -> bool {
        matches!(self.peek(), TokenKind::Newline | TokenKind::Eof)
    }

    fn at_block_end(&self) -> bool {
        matches!(self.peek(), TokenKind::Semicolon | TokenKind::Keyword(Keyword::Elif | Keyword::Else))
    }

    fn line_of(&self, index: usize) -> u32 {
        self.tokens[index].line
    }

    fn stray_semicolon(&mut self) -> Reported {
        let error = Diagnostic::error("unexpected `;`")
            .with_primary(self.span(), "this `;` does not close any block")
            .with_help(
                "the end of the line already ends the statement; `;` only closes a block opened by `:` (§5)",
            );
        self.error(error)
    }

    /// Skips the rest of a statement after an error, including the blocks it opens,
    /// so that the lines of a block are not read as statements of their own.
    fn skip_statement(&mut self) {
        let (mut blocks, mut brackets) = (0usize, 0usize);
        loop {
            match self.peek() {
                TokenKind::Eof => return,
                TokenKind::Newline if blocks == 0 => {
                    self.bump();
                    return;
                }
                TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => brackets += 1,
                TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                    brackets = brackets.saturating_sub(1)
                }
                TokenKind::Colon if brackets == 0 => blocks += 1,
                // `elif` and `else` close the previous branch before opening theirs (§5.2).
                TokenKind::Semicolon | TokenKind::Keyword(Keyword::Elif | Keyword::Else) if brackets == 0 => {
                    blocks = blocks.saturating_sub(1)
                }
                _ => {}
            }
            self.bump();
        }
    }

    // ----- Expressions, from the weakest to the strongest binding (§9.1) -----

    fn expr(&mut self) -> PResult<Expr> {
        if let TokenKind::Keyword(keyword) = self.peek() {
            if *keyword == Keyword::If {
                return self.if_expression();
            }
            if *keyword == Keyword::Match {
                return self.match_expression();
            }
            if *keyword == Keyword::Try {
                let start = self.bump().span;
                let value = self.expr()?;
                return Ok(Expr { span: start.to(value.span), kind: ExprKind::Try(Box::new(value)) });
            }
            if *keyword == Keyword::Parallel {
                let start = self.bump().span;
                let value = self.expr()?;
                return Ok(Expr { span: start.to(value.span), kind: ExprKind::Parallel(Box::new(value)) });
            }
            let unsupported = match keyword {
                Keyword::Fun => Some(("anonymous functions", "§11.1")),
                Keyword::Task | Keyword::Wait => Some(("tasks", "§19.1")),
                Keyword::Compile => Some(("`compile`", "§21.1")),
                Keyword::Shared | Keyword::Synced => Some(("shared values", "§17.2")),
                _ => None,
            };
            if let Some((what, section)) = unsupported {
                return Err(self.not_implemented(self.span(), what, section));
            }
        }
        self.or_expr()
    }

    /// `if c then a elif d then b else e`, which covers the whole expression to its
    /// right (§9.1, §10.1).
    fn if_expression(&mut self) -> PResult<Expr> {
        let start = self.bump().span;
        let cond = self.nested(Self::expr)?;
        self.if_expression_rest(start, cond)
    }

    fn if_expression_rest(&mut self, start: Span, cond: Expr) -> PResult<Expr> {
        if !self.eat_keyword(Keyword::Then) {
            return Err(self.expected("`then`"));
        }
        let mut branches = vec![(cond, self.expr()?)];
        while self.eat_keyword(Keyword::Elif) {
            let cond = self.nested(Self::expr)?;
            if !self.eat_keyword(Keyword::Then) {
                return Err(self.expected("`then`"));
            }
            branches.push((cond, self.expr()?));
        }
        let otherwise = if self.eat_keyword(Keyword::Else) { Some(Box::new(self.expr()?)) } else { None };
        let end = otherwise.as_ref().map_or(branches.last().expect("one branch").1.span, |e| e.span);
        Ok(Expr { kind: ExprKind::If { branches, otherwise }, span: start.to(end) })
    }

    fn or_expr(&mut self) -> PResult<Expr> {
        self.left_associative(Self::and_expr, |kind| match kind {
            TokenKind::Keyword(Keyword::Or) => Some(BinaryOp::Or),
            _ => None,
        })
    }

    fn and_expr(&mut self) -> PResult<Expr> {
        self.left_associative(Self::not_expr, |kind| match kind {
            TokenKind::Keyword(Keyword::And) => Some(BinaryOp::And),
            _ => None,
        })
    }

    fn not_expr(&mut self) -> PResult<Expr> {
        if !self.at_keyword(Keyword::Not) {
            return self.comparison();
        }
        let start = self.bump().span;
        let operand = self.not_expr()?;
        Ok(Expr {
            span: start.to(operand.span),
            kind: ExprKind::Unary { op: UnaryOp::Not, operand: Box::new(operand) },
        })
    }

    /// Chainable comparisons, or a single `in`, `subset` or `same` (§9.3).
    fn comparison(&mut self) -> PResult<Expr> {
        let first = self.as_expr()?;
        let expr = if self.compare_op().is_some() {
            let mut rest = Vec::new();
            while let Some(op) = self.compare_op() {
                let op_span = self.bump().span;
                rest.push(Comparison { op, op_span, rhs: self.as_expr()? });
            }
            let span = first.span.to(rest.last().expect("one comparison").rhs.span);
            Expr { kind: ExprKind::Compare { first: Box::new(first), rest }, span }
        } else if let Some(op) = self.membership_op() {
            let op_span = self.bump().span;
            // `x in Int`, `x in List of Int`: a type after `in` makes a type test (§7.1).
            if op == BinaryOp::In && self.type_starts_at(self.pos) {
                let ty = self.type_expr()?;
                Expr { span: first.span.to(ty.span), kind: ExprKind::TypeTest { value: Box::new(first), ty } }
            } else {
                let rhs = self.as_expr()?;
                binary(op, op_span, first, rhs)
            }
        } else {
            return Ok(first);
        };
        if self.compare_op().is_some() || self.membership_op().is_some() {
            let error = Diagnostic::error("these comparisons cannot be combined without parentheses")
                .with_primary(self.span(), "")
                .with_note("only `==`, `!=`, `<`, `>`, `<=` and `>=` can be chained; `in`, `subset` and `same` need parentheses (§9.3)");
            return Err(self.error(error));
        }
        Ok(expr)
    }

    fn compare_op(&self) -> Option<CompareOp> {
        Some(match self.peek() {
            TokenKind::EqEq => CompareOp::Eq,
            TokenKind::NotEq => CompareOp::Ne,
            TokenKind::Lt => CompareOp::Lt,
            TokenKind::Gt => CompareOp::Gt,
            TokenKind::Le => CompareOp::Le,
            TokenKind::Ge => CompareOp::Ge,
            _ => return None,
        })
    }

    fn membership_op(&self) -> Option<BinaryOp> {
        match self.peek() {
            TokenKind::Keyword(Keyword::In)
                if !(self.annotation_ends_expr && self.type_starts_at(self.pos + 1)) =>
            {
                Some(BinaryOp::In)
            }
            TokenKind::Keyword(Keyword::Subset) => Some(BinaryOp::Subset),
            TokenKind::Keyword(Keyword::Same) => Some(BinaryOp::Same),
            _ => None,
        }
    }

    fn as_expr(&mut self) -> PResult<Expr> {
        let mut value = self.union_expr()?;
        while self.eat_keyword(Keyword::As) {
            let ty = self.type_expr()?;
            value = Expr { span: value.span.to(ty.span), kind: ExprKind::As { value: Box::new(value), ty } };
        }
        Ok(value)
    }

    fn union_expr(&mut self) -> PResult<Expr> {
        self.left_associative(Self::inter_expr, |kind| match kind {
            TokenKind::Keyword(Keyword::Union) => Some(BinaryOp::Union),
            TokenKind::Keyword(Keyword::Minus) => Some(BinaryOp::Minus),
            _ => None,
        })
    }

    fn inter_expr(&mut self) -> PResult<Expr> {
        self.left_associative(Self::range_expr, |kind| match kind {
            TokenKind::Keyword(Keyword::Inter) => Some(BinaryOp::Inter),
            _ => None,
        })
    }

    /// `a..b`, which does not associate (§9.1).
    fn range_expr(&mut self) -> PResult<Expr> {
        let start = self.additive()?;
        if !self.at(&TokenKind::DotDot) {
            return Ok(start);
        }
        let op_span = self.bump().span;
        let end = self.additive()?;
        if self.at(&TokenKind::DotDot) {
            let error = Diagnostic::error("`..` cannot be chained")
                .with_primary(self.span(), "")
                .with_note("an interval has one start and one end: `a..b` (§16.3)");
            return Err(self.error(error));
        }
        Ok(binary(BinaryOp::Range, op_span, start, end))
    }

    fn additive(&mut self) -> PResult<Expr> {
        self.left_associative(Self::multiplicative, |kind| match kind {
            TokenKind::Plus => Some(BinaryOp::Add),
            TokenKind::Minus => Some(BinaryOp::Sub),
            _ => None,
        })
    }

    fn multiplicative(&mut self) -> PResult<Expr> {
        self.left_associative(Self::unary, |kind| match kind {
            TokenKind::Star => Some(BinaryOp::Mul),
            TokenKind::Slash => Some(BinaryOp::Div),
            TokenKind::Keyword(Keyword::Div) => Some(BinaryOp::IntDiv),
            TokenKind::Keyword(Keyword::Mod) => Some(BinaryOp::Mod),
            TokenKind::Keyword(Keyword::Over) => Some(BinaryOp::Over),
            _ => None,
        })
    }

    /// Unary minus binds less tightly than `^`: `-2 ^ 2` is -4 (§8.4).
    fn unary(&mut self) -> PResult<Expr> {
        if !self.at(&TokenKind::Minus) {
            return self.power();
        }
        let start = self.bump().span;
        let operand = self.unary()?;
        Ok(Expr {
            span: start.to(operand.span),
            kind: ExprKind::Unary { op: UnaryOp::Neg, operand: Box::new(operand) },
        })
    }

    /// `^` is right-associative, and its exponent may be negated: `2 ^ -1` (§8.4).
    fn power(&mut self) -> PResult<Expr> {
        let base = self.postfix()?;
        if !self.at(&TokenKind::Caret) {
            return Ok(base);
        }
        let op_span = self.bump().span;
        let exponent = self.unary()?;
        Ok(binary(BinaryOp::Pow, op_span, base, exponent))
    }

    fn postfix(&mut self) -> PResult<Expr> {
        let mut expr = self.primary()?;
        loop {
            match self.peek() {
                TokenKind::LParen => {
                    self.bump();
                    let args = self.nested(Self::args)?;
                    let end = self.expect(&TokenKind::RParen, "`)`")?;
                    expr = Expr {
                        span: expr.span.to(end),
                        kind: ExprKind::Call { callee: Box::new(expr), args },
                    };
                }
                TokenKind::LBracket => {
                    self.bump();
                    let index = self.nested(Self::expr)?;
                    let end = self.expect(&TokenKind::RBracket, "`]`")?;
                    expr = Expr {
                        span: expr.span.to(end),
                        kind: ExprKind::Index { object: Box::new(expr), index: Box::new(index) },
                    };
                }
                TokenKind::Dot => {
                    self.bump();
                    let span = self.span();
                    let name = match self.peek() {
                        TokenKind::LowerIdent(name) | TokenKind::UpperIdent(name) => name.clone(),
                        _ => return Err(self.expected("a field or method name")),
                    };
                    self.bump();
                    expr = Expr {
                        span: expr.span.to(span),
                        kind: ExprKind::Field { object: Box::new(expr), name: Ident { name, span } },
                    };
                }
                _ => return Ok(expr),
            }
        }
    }

    /// Call arguments: `[var] [name:] value`, separated by commas (§11.2).
    fn args(&mut self) -> PResult<Vec<Arg>> {
        let mut args = Vec::new();
        if self.at(&TokenKind::RParen) {
            return Ok(args);
        }
        loop {
            let var_marker = if self.at_keyword(Keyword::Var) { Some(self.bump().span) } else { None };
            let name = match self.peek() {
                TokenKind::LowerIdent(name) if self.kind_at(self.pos + 1) == &TokenKind::Colon => {
                    let ident = Ident { name: name.clone(), span: self.span() };
                    self.bump();
                    self.bump();
                    Some(ident)
                }
                _ => None,
            };
            let value = self.expr()?;
            args.push(Arg { var_marker, name, value });
            if !self.eat(&TokenKind::Comma) {
                return Ok(args);
            }
        }
    }

    fn primary(&mut self) -> PResult<Expr> {
        let span = self.span();
        let kind = match self.peek() {
            TokenKind::Int(value) => ExprKind::Int(*value),
            TokenKind::Float(value) => ExprKind::Float(*value),
            TokenKind::Keyword(Keyword::True) => ExprKind::Bool(true),
            TokenKind::Keyword(Keyword::False) => ExprKind::Bool(false),
            TokenKind::Keyword(Keyword::NoneValue) => ExprKind::None,
            TokenKind::LowerIdent(name) => ExprKind::Name(name.clone()),
            TokenKind::UpperIdent(name) => ExprKind::TypeName(name.clone()),
            TokenKind::TextStart => return self.text(),
            TokenKind::LParen => return self.parenthesized(),
            TokenKind::LBracket => return self.list(),
            TokenKind::LBrace => return self.set(),
            TokenKind::Keyword(Keyword::SelfValue) => ExprKind::Name("self".to_string()),
            _ => return Err(self.expected("an expression")),
        };
        self.bump();
        Ok(Expr { kind, span })
    }

    /// `[a, b, c]` or a comprehension; newlines are ignored inside the brackets (§5.1).
    fn list(&mut self) -> PResult<Expr> {
        let start = self.bump().span;
        let mut elements = Vec::new();
        if !self.at(&TokenKind::RBracket) {
            loop {
                if let TokenKind::LowerIdent(name) = self.peek()
                    && self.kind_at(self.pos + 1) == &TokenKind::Colon
                {
                    let error =
                        Diagnostic::error(format!("the elements of a list have no name, like `{name}:`"))
                            .with_primary(self.span(), "")
                            .with_note("names belong to the fields of a structure (§12.2)");
                    return Err(self.error(error));
                }
                elements.push(self.nested(Self::expr)?);
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
        }
        let end = self.expect(&TokenKind::RBracket, "`,` or `]`")?;
        Ok(Expr { kind: ExprKind::List(elements), span: start.to(end) })
    }

    /// `(expr)`, or a tuple: `()`, `(x,)`, `(a, b)`, `(name: a, grade: b)` (§4.5, D56).
    /// `{a, b, c}` or a set comprehension; newlines are ignored inside the braces (§5.1).
    fn set(&mut self) -> PResult<Expr> {
        let start = self.bump().span;
        let mut elements = Vec::new();
        if !self.at(&TokenKind::RBrace) {
            loop {
                elements.push(self.nested(Self::expr)?);
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
        }
        let end = self.expect(&TokenKind::RBrace, "`,` or `}`")?;
        Ok(Expr { kind: ExprKind::Set(elements), span: start.to(end) })
    }

    fn parenthesized(&mut self) -> PResult<Expr> {
        let start = self.bump().span;
        if self.at(&TokenKind::RParen) {
            let end = self.bump().span;
            return Ok(Expr { kind: ExprKind::Tuple(Vec::new()), span: start.to(end) });
        }
        let first = self.nested(Self::element)?;
        if first.name.is_none() && self.at(&TokenKind::RParen) {
            let end = self.bump().span;
            return Ok(Expr { kind: ExprKind::Paren(Box::new(first.value)), span: start.to(end) });
        }
        if !self.at(&TokenKind::Comma) {
            if first.name.is_some() && self.at(&TokenKind::RParen) {
                let error = Diagnostic::error("a tuple of one element ends with a comma")
                    .with_primary(self.span(), "")
                    .with_help("write `(name: value,)` (D56)");
                return Err(self.error(error));
            }
            return Err(self.expected("`,` or `)`"));
        }
        let mut elements = vec![first];
        while self.eat(&TokenKind::Comma) {
            if self.at(&TokenKind::RParen) {
                break;
            }
            elements.push(self.nested(Self::element)?);
        }
        let end = self.expect(&TokenKind::RParen, "`,` or `)`")?;
        Ok(Expr { kind: ExprKind::Tuple(elements), span: start.to(end) })
    }

    /// `[name:] value`, in a tuple.
    fn element(&mut self) -> PResult<Element> {
        let name = match self.peek() {
            TokenKind::LowerIdent(name) if self.kind_at(self.pos + 1) == &TokenKind::Colon => {
                let name = Ident { name: name.clone(), span: self.span() };
                self.bump();
                self.bump();
                Some(name)
            }
            _ => None,
        };
        Ok(Element { name, value: self.expr()? })
    }

    /// A text literal with its interpolations (§4.5, D24).
    fn text(&mut self) -> PResult<Expr> {
        let start = self.bump().span;
        let mut parts = Vec::new();
        let mut failed = false;
        loop {
            match self.peek() {
                TokenKind::TextChunk(chunk) => {
                    parts.push(TextPart::Literal(chunk.clone()));
                    self.bump();
                }
                TokenKind::InterpStart => {
                    let open = self.bump().span;
                    if self.at(&TokenKind::InterpEnd) {
                        let error = Diagnostic::error("empty interpolation")
                            .with_primary(open.to(self.span()), "")
                            .with_help("put a value between the braces, or write `\\{` for a literal brace");
                        self.error(error);
                        self.bump();
                        failed = true;
                        continue;
                    }
                    match self.nested(Self::expr) {
                        Ok(value) if self.at(&TokenKind::InterpEnd) => {
                            self.bump();
                            parts.push(TextPart::Interpolation(value));
                        }
                        result => {
                            if result.is_ok() {
                                self.expected("`}` to end the interpolation");
                            }
                            failed = true;
                            self.skip_interpolation();
                        }
                    }
                }
                TokenKind::TextEnd => {
                    let end = self.bump().span;
                    if failed {
                        return Err(Reported);
                    }
                    return Ok(Expr { kind: ExprKind::Text(parts), span: start.to(end) });
                }
                _ => {
                    let error = Diagnostic::internal("unbalanced text tokens").with_primary(self.span(), "");
                    return Err(self.error(error));
                }
            }
        }
    }

    /// Skips to the `}` closing the current interpolation, past nested ones.
    fn skip_interpolation(&mut self) {
        let mut depth = 0usize;
        loop {
            match self.peek() {
                TokenKind::InterpStart => depth += 1,
                TokenKind::InterpEnd if depth == 0 => {
                    self.bump();
                    return;
                }
                TokenKind::InterpEnd => depth -= 1,
                TokenKind::Eof => return,
                _ => {}
            }
            self.bump();
        }
    }

    // ----- Types (§7, §15) -----

    /// `A or B or ...`; `of` binds more tightly than `or` (D34).
    fn type_expr(&mut self) -> PResult<TypeExpr> {
        let first = self.maybe_type()?;
        if !self.union_continues() {
            return Ok(first);
        }
        let mut members = vec![first];
        while self.union_continues() {
            self.bump();
            members.push(self.maybe_type()?);
        }
        let span = members[0].span.to(members.last().expect("two members").span);
        Ok(TypeExpr { kind: TypeExprKind::Union(members), span })
    }

    /// In `x as Int or ok`, the `or` joins expressions: a type must follow it to join types.
    fn union_continues(&self) -> bool {
        self.at_keyword(Keyword::Or)
            && (self.type_starts_at(self.pos + 1) || self.kind_at(self.pos + 1) == &TokenKind::LParen)
    }

    fn maybe_type(&mut self) -> PResult<TypeExpr> {
        if !self.at_keyword(Keyword::Maybe) {
            return self.applied_type();
        }
        let start = self.bump().span;
        let inner = self.maybe_type()?;
        Ok(TypeExpr { span: start.to(inner.span), kind: TypeExprKind::Maybe(Box::new(inner)) })
    }

    fn applied_type(&mut self) -> PResult<TypeExpr> {
        let start = self.span();
        match self.peek() {
            TokenKind::UpperIdent(_) | TokenKind::LowerIdent(_) => {
                let (module, name) = self.qualified_type_name()?;
                let mut args = Vec::new();
                if self.eat_keyword(Keyword::Of) {
                    if self.eat(&TokenKind::LParen) {
                        args = self.nested(Self::type_list)?;
                        self.expect(&TokenKind::RParen, "`)`")?;
                    } else {
                        args.push(self.applied_type()?);
                    }
                }
                let span = start.to(self.previous_span());
                Ok(TypeExpr { kind: TypeExprKind::Named { module, name, args }, span })
            }
            TokenKind::LParen => {
                self.bump();
                let mut members = self.nested(Self::type_list)?;
                let end = self.expect(&TokenKind::RParen, "`)`")?;
                if members.len() == 1 {
                    return Ok(members.pop().expect("one member"));
                }
                Ok(TypeExpr { kind: TypeExprKind::Tuple(members), span: start.to(end) })
            }
            TokenKind::Keyword(Keyword::Fun) => {
                self.bump();
                self.expect(&TokenKind::LParen, "`(`")?;
                let params =
                    if self.at(&TokenKind::RParen) { Vec::new() } else { self.nested(Self::type_list)? };
                let mut end = self.expect(&TokenKind::RParen, "`)`")?;
                let ret = if self.eat_keyword(Keyword::In) {
                    let ret = self.type_expr()?;
                    end = ret.span;
                    Some(Box::new(ret))
                } else {
                    None
                };
                Ok(TypeExpr { kind: TypeExprKind::Fun { params, ret }, span: start.to(end) })
            }
            _ => Err(self.expected("a type")),
        }
    }

    fn type_list(&mut self) -> PResult<Vec<TypeExpr>> {
        let mut types = vec![self.type_expr()?];
        while self.eat(&TokenKind::Comma) {
            types.push(self.type_expr()?);
        }
        Ok(types)
    }

    /// `Student` or `notes_data.Student` (§26 `qual_type`).
    fn qualified_type_name(&mut self) -> PResult<(Vec<Ident>, Ident)> {
        let mut module = Vec::new();
        loop {
            let span = self.span();
            match self.peek() {
                TokenKind::UpperIdent(name) => {
                    let name = name.clone();
                    self.bump();
                    return Ok((module, Ident { name, span }));
                }
                TokenKind::LowerIdent(name) if self.kind_at(self.pos + 1) == &TokenKind::Dot => {
                    module.push(Ident { name: name.clone(), span });
                    self.bump();
                    self.bump();
                }
                TokenKind::LowerIdent(name) => {
                    let error = Diagnostic::error(format!("`{name}` is not a type"))
                        .with_primary(span, "type names start with an uppercase letter (§4.2)");
                    return Err(self.error(error));
                }
                _ => return Err(self.expected("a type name")),
            }
        }
    }

    /// Whether a type starts at token `index`: an uppercase name, `maybe`, `fun`, or a
    /// module path ending with an uppercase name.
    fn type_starts_at(&self, mut index: usize) -> bool {
        loop {
            match self.kind_at(index) {
                TokenKind::UpperIdent(_) | TokenKind::Keyword(Keyword::Maybe | Keyword::Fun) => {
                    return true;
                }
                TokenKind::LowerIdent(_) if self.kind_at(index + 1) == &TokenKind::Dot => index += 2,
                _ => return false,
            }
        }
    }

    // ----- Helpers -----

    fn left_associative(
        &mut self,
        operand: fn(&mut Self) -> PResult<Expr>,
        operator: fn(&TokenKind) -> Option<BinaryOp>,
    ) -> PResult<Expr> {
        let mut lhs = operand(self)?;
        while let Some(op) = operator(self.peek()) {
            let op_span = self.bump().span;
            let rhs = operand(self)?;
            lhs = binary(op, op_span, lhs, rhs);
        }
        Ok(lhs)
    }

    /// Parses inside brackets, where `in` is never a type annotation.
    fn nested<T>(&mut self, parse: fn(&mut Self) -> PResult<T>) -> PResult<T> {
        let saved = std::mem::replace(&mut self.annotation_ends_expr, false);
        let result = parse(self);
        self.annotation_ends_expr = saved;
        result
    }

    fn peek(&self) -> &'t TokenKind {
        &self.tokens[self.pos].kind
    }

    fn kind_at(&self, index: usize) -> &'t TokenKind {
        &self.tokens[index.min(self.tokens.len() - 1)].kind
    }

    fn span(&self) -> Span {
        self.tokens[self.pos].span
    }

    fn previous_span(&self) -> Span {
        self.tokens[self.pos.saturating_sub(1)].span
    }

    fn bump(&mut self) -> &Token {
        let token = &self.tokens[self.pos];
        if token.kind != TokenKind::Eof {
            self.pos += 1;
        }
        token
    }

    fn at(&self, kind: &TokenKind) -> bool {
        self.peek() == kind
    }

    fn at_keyword(&self, keyword: Keyword) -> bool {
        self.peek() == &TokenKind::Keyword(keyword)
    }

    fn eat(&mut self, kind: &TokenKind) -> bool {
        let found = self.at(kind);
        if found {
            self.bump();
        }
        found
    }

    fn eat_keyword(&mut self, keyword: Keyword) -> bool {
        self.eat(&TokenKind::Keyword(keyword))
    }

    fn expect(&mut self, kind: &TokenKind, what: &str) -> PResult<Span> {
        if self.at(kind) { Ok(self.bump().span) } else { Err(self.expected(what)) }
    }

    fn expected(&mut self, what: &str) -> Reported {
        let token = &self.tokens[self.pos];
        let error = Diagnostic::error(format!("expected {what}, found {}", token.kind.describe()))
            .with_primary(token.span, "");
        self.error(error)
    }

    fn not_implemented(&mut self, span: Span, what: &str, section: &str) -> Reported {
        self.error(Diagnostic::not_implemented(span, what, section))
    }

    fn error(&mut self, diagnostic: Diagnostic) -> Reported {
        self.diagnostics.push(diagnostic);
        Reported
    }
}

fn binary(op: BinaryOp, op_span: Span, lhs: Expr, rhs: Expr) -> Expr {
    Expr {
        span: lhs.span.to(rhs.span),
        kind: ExprKind::Binary { op, op_span, lhs: Box::new(lhs), rhs: Box::new(rhs) },
    }
}

/// Tokens that can only continue an expression, never start a statement (§5.1).
fn is_operator(kind: &TokenKind) -> bool {
    use TokenKind::*;
    match kind {
        Plus | Minus | Star | Slash | Caret | EqEq | NotEq | Lt | Gt | Le | Ge | DotDot | Dot | Assign
        | PlusAssign | MinusAssign | StarAssign => true,
        Keyword(keyword) => matches!(
            keyword,
            crate::Keyword::And
                | crate::Keyword::Or
                | crate::Keyword::Not
                | crate::Keyword::Div
                | crate::Keyword::Mod
                | crate::Keyword::Over
                | crate::Keyword::Union
                | crate::Keyword::Inter
                | crate::Keyword::Minus
                | crate::Keyword::Subset
                | crate::Keyword::Same
                | crate::Keyword::In
                | crate::Keyword::As
        ),
        _ => false,
    }
}

fn uppercase_first(name: &str) -> String {
    let mut chars = name.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

fn lowercase_first(name: &str) -> String {
    let mut chars = name.chars();
    chars.next().map_or_else(String::new, |first| first.to_lowercase().chain(chars).collect())
}
