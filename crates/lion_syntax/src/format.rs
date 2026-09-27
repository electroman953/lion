//! The official layout of Lion code (spec §5.3, §24, D20): four spaces of indentation
//! per block, and one more for a line that continues inside brackets. The formatter
//! only changes the whitespace at the start and at the end of the lines, and the
//! blank lines: it never changes the meaning of the program, nor its comments (C69).

use crate::token::{Keyword, Token, TokenKind};

const INDENT: usize = 4;

/// The formatted text of a file whose tokens are `tokens` and whose `/* ... */`
/// comments are at `block_comments`. The file must have no syntax error.
pub fn format(text: &str, tokens: &[Token], block_comments: &[(usize, usize)]) -> String {
    let lines: Vec<&str> = text.lines().collect();
    // The lines that continue a `/* ... */` comment keep their layout.
    let mut in_comment = vec![false; lines.len() + 2];
    for &(start, end) in block_comments {
        let first = line_of(text, start);
        let last = line_of(text, end.saturating_sub(1));
        for flag in &mut in_comment[first + 1..=last] {
            *flag = true;
        }
    }
    // The indentation of each line comes from the blocks and brackets open before it.
    let mut indent = vec![None; lines.len() + 2];
    let (mut blocks, mut brackets) = (0usize, 0usize);
    // The `then`s of `if ... then ... else` expressions on the line, which own the next
    // `else`: these do not close a block.
    let mut expressions = 0usize;
    let mut line = 0usize;
    let mut first_on_line = true;
    for token in tokens {
        let token_line = token.line as usize;
        if token_line != line {
            // The lines without tokens before this one are at the current depth.
            for between in line + 1..token_line.min(indent.len()) {
                indent[between] = Some(blocks + usize::from(brackets > 0));
            }
            line = token_line;
            first_on_line = true;
        }
        if matches!(token.kind, TokenKind::Newline | TokenKind::Eof) {
            if brackets == 0 {
                expressions = 0;
            }
            continue;
        }
        if first_on_line {
            first_on_line = false;
            let closing_bracket =
                matches!(token.kind, TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace);
            let depth = if brackets > 0 {
                blocks + usize::from(!closing_bracket || brackets > 1)
            } else {
                match token.kind {
                    TokenKind::Keyword(Keyword::Elif | Keyword::Else) | TokenKind::Semicolon => {
                        blocks.saturating_sub(1)
                    }
                    _ => blocks,
                }
            };
            indent[line] = Some(depth);
        }
        match token.kind {
            TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace | TokenKind::InterpStart => {
                brackets += 1
            }
            TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace | TokenKind::InterpEnd => {
                brackets = brackets.saturating_sub(1)
            }
            TokenKind::Colon if brackets == 0 => blocks += 1,
            TokenKind::Keyword(Keyword::Then) if brackets == 0 => expressions += 1,
            TokenKind::Keyword(Keyword::Elif | Keyword::Else) if brackets == 0 && expressions > 0 => {
                expressions -= 1
            }
            // `elif` and `else` close the branch before them; their `:` opens theirs.
            TokenKind::Keyword(Keyword::Elif | Keyword::Else) if brackets == 0 => {
                blocks = blocks.saturating_sub(1)
            }
            TokenKind::Semicolon if brackets == 0 => blocks = blocks.saturating_sub(1),
            _ => {}
        }
    }
    // A line without tokens, such as a comment, takes the depth where it stands.
    let mut depth = 0;
    for slot in &mut indent[1..=lines.len()] {
        match slot {
            Some(found) => depth = *found,
            None => *slot = Some(depth),
        }
    }
    let mut out = String::new();
    let mut blank_before = false;
    for (index, content) in lines.iter().enumerate() {
        let number = index + 1;
        if in_comment[number] {
            out.push_str(content.trim_end());
            out.push('\n');
            blank_before = false;
            continue;
        }
        let content = content.trim();
        if content.is_empty() {
            blank_before = !out.is_empty();
            continue;
        }
        if blank_before {
            out.push('\n');
            blank_before = false;
        }
        let depth = indent[number].unwrap_or(0);
        out.push_str(&" ".repeat(depth * INDENT));
        out.push_str(content);
        out.push('\n');
    }
    out
}

/// The line, from 1, of a byte of the text.
fn line_of(text: &str, byte: usize) -> usize {
    text[..byte.min(text.len())].matches('\n').count() + 1
}
