//! Resolution of names: variables, globals, functions and the standard library,
//! with suggestions for misspelt names.

use lion_diagnostics::{Diagnostic, Span};
use lion_ir as ir;

use crate::{Checker, ContextKind, GlobalType};

/// Standard functions available without `use` (spec §23).
pub(crate) const IMPLEMENTED_FUNCTIONS: &[&str] = &["show", "sum", "error"];

/// Standard functions of the spec (§4.4, §23) that this version does not provide yet.
pub(crate) const PLANNED_FUNCTIONS: &[&str] = &["ask", "exit", "reverse", "floor", "ceil", "round", "isqrt"];

/// What a name designates at some point of the program.
pub(crate) enum Resolved {
    /// A variable or parameter of the function being checked, or of the script.
    Local(ir::LocalId),
    /// A top-level variable of the script, seen from a function.
    Global(ir::LocalId),
    /// A function declared at the top level of the file.
    Function(usize),
    /// A function of the standard library.
    Standard(&'static str),
    /// Nothing, or something already reported.
    Nothing,
}

impl Checker<'_> {
    /// Looks a name up, innermost first: the blocks of the current function, then (from
    /// a function) the globals of the script, then the functions of the file, then the
    /// standard library. Declarations may hide standard functions (C2).
    pub(crate) fn resolve(&mut self, name: &str, span: Span) -> Resolved {
        if let Some(local) = self.lookup(name) {
            return Resolved::Local(local);
        }
        if let ContextKind::Structure(_) = self.ctx.kind
            && self.globals.contains_key(name)
        {
            self.diagnostics.push(
                Diagnostic::error(format!("a structure cannot read the variable `{name}`"))
                    .with_primary(span, "")
                    .with_note("the default values and the conditions of a structure read only its fields, constants and functions (C48)"),
            );
            return Resolved::Nothing;
        }
        if self.ctx.kind != ContextKind::Script && self.globals.contains_key(name) {
            return match self.global(name, span) {
                Some(local) => Resolved::Global(local),
                None => Resolved::Nothing,
            };
        }
        if let Some(&index) = self.function_names.get(name) {
            return Resolved::Function(index);
        }
        if let Some(&standard) = IMPLEMENTED_FUNCTIONS.iter().chain(PLANNED_FUNCTIONS).find(|n| **n == name) {
            return Resolved::Standard(standard);
        }
        let error = self.unknown_name_error(name, span);
        self.diagnostics.push(error);
        Resolved::Nothing
    }

    /// Whether `name` designates anything here, without reporting.
    pub(crate) fn is_known(&self, name: &str) -> bool {
        self.lookup(name).is_some()
            || (self.ctx.kind != ContextKind::Script && self.globals.contains_key(name))
            || self.function_names.contains_key(name)
            || IMPLEMENTED_FUNCTIONS.contains(&name)
            || PLANNED_FUNCTIONS.contains(&name)
    }

    /// A global seen from a function: declared exactly once (C5), and already reached
    /// by the script when the function body is checked early (C17).
    fn global(&mut self, name: &str, span: Span) -> Option<ir::LocalId> {
        let global = &self.globals[name];
        let decl_span = global.decl.name.span;
        let Some(local) = global.local else {
            self.diagnostics.push(
                Diagnostic::error(format!(
                    "`{name}` is declared several times at the top level of the script"
                ))
                .with_primary(span, "a function cannot tell which declaration this is")
                .with_secondary(decl_span, "first declaration")
                .with_help("give the top-level declarations different names"),
            );
            return None;
        };
        match global.ty {
            GlobalType::Known(ty) => ty.map(|_| local),
            GlobalType::Unknown => {
                let mut error = Diagnostic::error(format!("`{name}` is used before the script declares it"))
                    .with_primary(span, "used here")
                    .with_secondary(decl_span, "declared here");
                if let Some(&call) = self.demands.first() {
                    error =
                        error.with_secondary(call, "the function is called here, before that declaration");
                }
                self.diagnostics.push(error.with_note(
                    "a function may use a variable declared further down, but it can only be called once the variable has a value (§6.1, §6.5)",
                ));
                None
            }
        }
    }

    /// "cannot find `x`", with a suggestion when a close name exists.
    pub(crate) fn unknown_name_error(&self, name: &str, span: Span) -> Diagnostic {
        let mut visible: Vec<&str> =
            self.ctx.scopes.iter().flat_map(|scope| scope.names.keys().map(String::as_str)).collect();
        visible.extend(self.function_names.keys().map(String::as_str));
        visible.extend(IMPLEMENTED_FUNCTIONS);
        if self.ctx.kind != ContextKind::Script {
            visible.extend(self.globals.keys().map(String::as_str));
        }
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
