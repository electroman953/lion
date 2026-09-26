//! The syntax of Lion: tokens, lexer, syntax tree and parser (spec §4, §5, §26).

pub mod ast;
mod lexer;
mod parser;
mod print;
mod token;

pub use lexer::{Lexed, lex};
pub use parser::{Parsed, parse};
pub use print::{print_expr, print_module, print_stmt, print_type};
pub use token::{Keyword, Token, TokenKind, escape_text};

#[cfg(test)]
mod tests;
