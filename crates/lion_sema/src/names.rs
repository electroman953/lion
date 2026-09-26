//! Names that are not declared by the program: the standard library, and
//! suggestions for misspelt names.

use lion_diagnostics::{Diagnostic, Span};
use lion_syntax::ast;

use crate::Checker;

/// Standard functions available without `use` (spec §23).
pub(crate) const IMPLEMENTED_FUNCTIONS: &[&str] = &["show"];

/// Standard functions of the spec (§4.4, §23) that this version does not provide yet.
pub(crate) const PLANNED_FUNCTIONS: &[&str] =
    &["ask", "error", "exit", "sum", "reverse", "floor", "ceil", "round", "isqrt"];

impl Checker {
    /// Declaring a name of the standard library is refused: Lion 0.1 does not say
    /// whether the declaration would hide the function (see docs/implementation-notes.md).
    pub(crate) fn check_not_standard_name(&mut self, name: &ast::Ident) {
        let name_text = name.name.as_str();
        if !IMPLEMENTED_FUNCTIONS.contains(&name_text) && !PLANNED_FUNCTIONS.contains(&name_text) {
            return;
        }
        self.diagnostics.push(
            Diagnostic::error(format!("`{name_text}` is the name of a standard function"))
                .with_primary(name.span, "")
                .with_note("Lion 0.1 does not define whether a declaration may hide a standard function, so it is refused")
                .with_help("choose another name"),
        );
    }

    /// "cannot find `x`", with a suggestion when a close name exists.
    pub(crate) fn unknown_name_error(&self, name: &str, span: Span) -> Diagnostic {
        let visible = self
            .scopes
            .iter()
            .flat_map(|scope| scope.names.keys().map(String::as_str))
            .chain(IMPLEMENTED_FUNCTIONS.iter().copied());
        let mut error =
            Diagnostic::error(format!("cannot find `{name}` in this scope")).with_primary(span, "not found");
        if let Some(help) = other_language_help(name) {
            error = error.with_help(help);
        } else if let Some(close) = closest(name, visible) {
            error = error.with_help(format!("a similar name exists: `{close}`"));
        }
        error
    }
}

/// Names that other languages use for a standard function of Lion.
fn other_language_help(name: &str) -> Option<&'static str> {
    match name {
        "print" | "println" | "printf" | "puts" | "echo" | "display" | "writeln" | "log" => {
            Some("Lion writes a value with `show(value)` (§23)")
        }
        "input" | "readline" | "readln" | "scanf" | "prompt" => {
            Some("Lion reads a line with `ask(question)` (§23), which is not implemented yet")
        }
        _ => None,
    }
}

/// The candidate closest to `name`, if it is close enough to be a likely typo.
pub(crate) fn closest<'a>(name: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let limit = (name.chars().count() / 3).max(1);
    candidates
        .into_iter()
        .filter(|&candidate| candidate != name)
        .map(|candidate| (edit_distance(name, candidate), candidate))
        .filter(|&(distance, _)| distance <= limit)
        .min()
        .map(|(_, candidate)| candidate)
}

/// Levenshtein distance, counted in characters.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut current = vec![i + 1];
        for (j, &cb) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(ca != cb);
            current.push(substitution.min(previous[j + 1] + 1).min(current[j] + 1));
        }
        previous = current;
    }
    previous[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distances() {
        assert_eq!(edit_distance("total", "total"), 0);
        assert_eq!(edit_distance("totl", "total"), 1);
        assert_eq!(edit_distance("shwo", "show"), 2);
        assert_eq!(edit_distance("", "abc"), 3);
    }

    #[test]
    fn suggestions() {
        assert_eq!(closest("totl", ["total", "count"]), Some("total"));
        assert_eq!(closest("x", ["y", "z"]), Some("y"));
        assert_eq!(closest("grade", ["name"]), None);
        assert_eq!(closest("Strng", ["Int", "Text", "String"]), Some("String"));
    }
}
