//! The index of the names of a program: what each one designates, where it is declared
//! and where it is written (C103).
//!
//! The checker fills it as it checks, so it exists even when the program has errors and
//! no IR is built: `lion lsp` answers hover, go to definition and find references with
//! it, while the file is being typed.
//!
//! Each declaration is one entry, found again by the span of its name; a declaration is
//! also a use of itself, so the editor answers from the declaration as from a use. The
//! body of a generic function is checked once per instance (C1): the first one checked
//! gives the types of its names.

use std::collections::HashMap;

use lion_diagnostics::{SourceId, Span};
use lion_ir::{self as ir, Type};

use crate::{Checker, GlobalType};

/// What a name designates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefKind {
    /// A variable or a constant of a body (§6).
    Variable,
    /// A parameter (§11.2).
    Parameter,
    Function,
    /// A function called on a value: `fun Student.passes()` (§12.4).
    Method,
    Structure,
    Field,
    /// An enumeration, a union or a trait.
    Type,
}

/// A declaration, and how the editor shows it.
#[derive(Clone, Debug)]
pub struct Definition {
    pub name: String,
    pub kind: DefKind,
    /// The name in its declaration.
    pub span: Span,
    /// The declaration as the editor shows it: `var total in Int`, `fun add(a in Int,
    /// b in Int) in Int`.
    pub signature: String,
}

/// A name written in the program, and what it designates.
#[derive(Clone, Copy, Debug)]
pub struct Occurrence {
    pub span: Span,
    /// Its declaration, by its position in [`Index::definitions`].
    pub definition: usize,
}

/// The names of a program, by their position in the files.
#[derive(Default)]
pub struct Index {
    definitions: Vec<Definition>,
    /// In the order of the files once the check is over, to be searched by position.
    occurrences: Vec<Occurrence>,
    /// The declarations already recorded, by the span of their name.
    declared: HashMap<Span, usize>,
    /// The names already recorded, so that the first check of a body wins.
    written: HashMap<Span, usize>,
}

impl Index {
    pub fn definitions(&self) -> &[Definition] {
        &self.definitions
    }

    pub fn occurrences(&self) -> &[Occurrence] {
        &self.occurrences
    }

    pub fn definition(&self, position: usize) -> &Definition {
        &self.definitions[position]
    }

    /// The name written at `offset` of `source`, and what it designates. A cursor at
    /// either end of the name counts as inside it.
    pub fn at(&self, source: SourceId, offset: u32) -> Option<(&Occurrence, &Definition)> {
        let after = self
            .occurrences
            .partition_point(|written| (written.span.source, written.span.start) <= (source, offset));
        let occurrence = self.occurrences[..after].last()?;
        if occurrence.span.source != source || offset > occurrence.span.end {
            return None;
        }
        Some((occurrence, &self.definitions[occurrence.definition]))
    }

    /// Every name that designates the declaration `definition`, in the order of the
    /// files, including the declaration itself.
    pub fn uses(&self, definition: usize) -> impl Iterator<Item = Span> + '_ {
        self.occurrences.iter().filter(move |written| written.definition == definition).map(|it| it.span)
    }

    /// The declaration whose name is written at `span`, when it is already recorded.
    fn known(&self, span: Span) -> Option<usize> {
        self.declared.get(&span).copied()
    }

    /// Records a declaration, or writes the text of the one already recorded: the
    /// signatures are written again once every body is checked, since a function
    /// without a written return type knows it only then.
    fn redefine(&mut self, kind: DefKind, name: &str, span: Span, signature: String) -> usize {
        if let Some(known) = self.known(span) {
            self.definitions[known].signature = signature;
            return known;
        }
        self.define(kind, name, span, || signature)
    }

    /// Records a declaration; gives it, or the one already recorded for that name.
    fn define(&mut self, kind: DefKind, name: &str, span: Span, signature: impl FnOnce() -> String) -> usize {
        if let Some(known) = self.known(span) {
            return known;
        }
        let position = self.definitions.len();
        self.definitions.push(Definition { name: name.to_string(), kind, span, signature: signature() });
        self.declared.insert(span, position);
        // A declaration is a use of itself: the editor answers from its own name too.
        self.record(span, position);
        position
    }

    /// Records that the name written at `span` designates `definition`.
    fn record(&mut self, span: Span, definition: usize) {
        if self.written.contains_key(&span) {
            return;
        }
        self.written.insert(span, self.occurrences.len());
        self.occurrences.push(Occurrence { span, definition });
    }

    /// Puts the names in the order of the files, once the check is over.
    pub(crate) fn finish(&mut self) {
        self.occurrences.sort_by_key(|written| (written.span.source, written.span.start));
        self.declared = HashMap::new();
        self.written = HashMap::new();
    }
}

impl Checker<'_> {
    /// The name at `span` is the variable `local` of the function being checked. A
    /// variable that the function captures is the one the program declares (§11.5).
    pub(crate) fn index_local(&mut self, local: ir::LocalId, span: Span) {
        let info = &self.ctx.locals[local.index()];
        if info.temporary {
            return;
        }
        let decl = info.outer.unwrap_or(info.decl_span);
        let (name, ty, mutable) = (info.name.clone(), info.ty, info.mutable);
        let definition = self.index.define(DefKind::Variable, &name, decl, || variable(&name, ty, mutable));
        self.index.record(span, definition);
    }

    /// The name at `span` is the parameter `local` of the function being checked.
    pub(crate) fn index_parameter(&mut self, local: ir::LocalId, span: Span) {
        let info = &self.ctx.locals[local.index()];
        let (name, ty, by_reference) = (info.name.clone(), info.ty, info.by_reference);
        let signature = || {
            let keyword = if by_reference { "var " } else { "" };
            match ty {
                Some(ty) => format!("{keyword}{name} in {ty}"),
                None => format!("{keyword}{name}"),
            }
        };
        let definition = self.index.define(DefKind::Parameter, &name, span, signature);
        self.index.record(span, definition);
    }

    /// The name at `span` is the global `local`, seen from a function (§20.2).
    pub(crate) fn index_global(&mut self, local: ir::LocalId, span: Span) {
        let global = self.global_info(local);
        let decl = global.decl.name.span;
        let name = global.decl.name.name.clone();
        let mutable = global.decl.mutable;
        let ty = match global.ty {
            GlobalType::Known(ty) => ty,
            GlobalType::Unknown => None,
        };
        let definition = self.index.define(DefKind::Variable, &name, decl, || variable(&name, ty, mutable));
        self.index.record(span, definition);
    }

    /// The name at `span` is the function or the method `function`.
    pub(crate) fn index_function(&mut self, function: usize, span: Span) {
        let info = &self.functions[function];
        // A `test` block is named by its text, not by a name the program writes.
        if info.test.is_some() {
            return;
        }
        let decl = info.decl.name.span;
        let kind = if info.receiver.is_some() || info.collection.is_some() {
            DefKind::Method
        } else {
            DefKind::Function
        };
        let definition = match self.index.known(decl) {
            Some(known) => known,
            None => {
                let name = self.functions[function].decl.name.name.clone();
                let signature = self.function_signature(function);
                self.index.define(kind, &name, decl, || signature)
            }
        };
        self.index.record(span, definition);
    }

    /// `fun Student.passes(mark in Int) in Bool`, as the editor shows it.
    fn function_signature(&self, function: usize) -> String {
        let info = &self.functions[function];
        let mut text = "fun ".to_string();
        if let Some(receiver) = &info.decl.receiver {
            text.push_str(&receiver.name);
            text.push('.');
        }
        text.push_str(&info.decl.name.name);
        // The signature once resolved, which knows the types; else the names written.
        let params: Vec<String> = match &info.signature {
            Some(params) => params
                .iter()
                .filter(|param| param.name != "self")
                .map(|param| {
                    let keyword = if param.by_reference { "var " } else { "" };
                    match param.ty {
                        Some(ty) => format!("{keyword}{} in {ty}", param.name),
                        None => format!("{keyword}{}", param.name),
                    }
                })
                .collect(),
            None => info.decl.params.iter().map(|param| param.name.name.clone()).collect(),
        };
        text.push('(');
        text.push_str(&params.join(", "));
        text.push(')');
        if let Some(ret) = info.declared_ret.or_else(|| self.returned_type(function)) {
            text.push_str(" in ");
            text.push_str(&ret.to_string());
        }
        text
    }

    /// The type that the only instance of a function gives back, once it is known.
    fn returned_type(&self, function: usize) -> Option<Type> {
        let instance = self.functions[function].instance?;
        self.instances.get(instance).and_then(crate::functions::Instance::returned)
    }

    /// The name at `span` is the structure `structure`, by its position in `structs`.
    pub(crate) fn index_struct(&mut self, structure: usize, span: Span) {
        let decl = self.structs[structure].decl.name.span;
        let name = self.structs[structure].decl.name.name.clone();
        let definition = self.index.define(DefKind::Structure, &name, decl, || format!("structure {name}"));
        self.index.record(span, definition);
    }

    /// The name at `span` is the field `position` of the structure `structure`.
    pub(crate) fn index_field(&mut self, structure: usize, position: usize, span: Span) {
        let info = &self.structs[structure];
        let owner = info.decl.name.name.clone();
        let Some(field) = info.fields.get(position) else { return };
        let (name, decl, ty) = (field.name.clone(), field.span, field.ty);
        let signature = || match ty {
            Some(ty) => format!("{owner}.{name} in {ty}"),
            None => format!("{owner}.{name}"),
        };
        let definition = self.index.define(DefKind::Field, &name, decl, signature);
        self.index.record(span, definition);
    }

    /// Records the functions and the structures that no name reached, and writes the
    /// signatures again now that every body is checked.
    pub(crate) fn index_declarations(&mut self) {
        for function in 0..self.functions.len() {
            let info = &self.functions[function];
            if info.test.is_some() || info.closure.is_some() {
                continue;
            }
            let decl = info.decl.name.span;
            let name = info.decl.name.name.clone();
            let kind = if info.receiver.is_some() || info.collection.is_some() {
                DefKind::Method
            } else {
                DefKind::Function
            };
            let signature = self.function_signature(function);
            self.index.redefine(kind, &name, decl, signature);
        }
        for structure in 0..self.structs.len() {
            // An instance of a generic structure shares the declaration of its template.
            if !self.structs[structure].type_args.is_empty() {
                continue;
            }
            let decl = self.structs[structure].decl.name.span;
            let name = self.structs[structure].decl.name.name.clone();
            self.index.redefine(DefKind::Structure, &name, decl, format!("structure {name}"));
            for position in 0..self.structs[structure].fields.len() {
                let field = &self.structs[structure].fields[position];
                let span = field.span;
                self.index_field(structure, position, span);
            }
        }
    }

    /// The name at `span` is the type declared at `decl`: an enumeration, a union or a
    /// trait of the file being checked.
    pub(crate) fn index_type(&mut self, name: &str, decl: Span, span: Span) {
        let definition = self.index.define(DefKind::Type, name, decl, || format!("type {name}"));
        self.index.record(span, definition);
    }
}

/// `let total in Int`, `var seen in Set of Text`.
fn variable(name: &str, ty: Option<Type>, mutable: bool) -> String {
    let keyword = if mutable { "var" } else { "let" };
    match ty {
        Some(ty) => format!("{keyword} {name} in {ty}"),
        None => format!("{keyword} {name}"),
    }
}

#[cfg(test)]
mod tests {
    use lion_diagnostics::SourceMap;

    use super::*;

    /// The index of a program of one file, and that file.
    fn indexed(text: &str) -> (Index, SourceId) {
        let mut map = SourceMap::new();
        let id = map.add("t.lion", text);
        let lexed = lion_syntax::lex(id, text);
        let parsed = lion_syntax::parse(text, &lexed.tokens);
        assert!(lexed.diagnostics.is_empty() && parsed.diagnostics.is_empty(), "syntax error in {text:?}");
        (crate::check(&parsed.module).index, id)
    }

    /// Where `name` starts for the `nth` time in `text`, counted from 1.
    fn place(text: &str, name: &str, nth: usize) -> u32 {
        text.match_indices(name).nth(nth - 1).expect("that many names").0 as u32
    }

    /// What the editor shows for the `nth` `name` of `text`.
    fn shown(text: &str, name: &str, nth: usize) -> String {
        let (index, file) = indexed(text);
        let (_, definition) = index.at(file, place(text, name, nth)).expect("a name here");
        definition.signature.clone()
    }

    /// Where every name that designates the `nth` `name` of `text` starts.
    fn uses(text: &str, name: &str, nth: usize) -> Vec<u32> {
        let (index, file) = indexed(text);
        let (occurrence, _) = index.at(file, place(text, name, nth)).expect("a name here");
        index.uses(occurrence.definition).map(|span| span.start).collect()
    }

    #[test]
    fn a_variable_gives_its_type_its_declaration_and_its_uses() {
        let text = "let total = 1 + 2\nshow(total)\nshow(total + 1)\n";
        let (index, file) = indexed(text);
        let (_, definition) = index.at(file, place(text, "total", 2)).expect("a name here");
        assert_eq!(definition.signature, "let total in Int");
        assert_eq!(definition.kind, DefKind::Variable);
        assert_eq!(definition.span.start, place(text, "total", 1));
        assert_eq!(
            uses(text, "total", 2),
            [place(text, "total", 1), place(text, "total", 2), place(text, "total", 3)]
        );
        assert_eq!(shown("var seen = 0\nseen += 1\n", "seen", 2), "var seen in Int");
    }

    #[test]
    fn a_cursor_lands_on_the_name_it_is_in() {
        let text = "let total = 1\nshow(total)\n";
        let (index, file) = indexed(text);
        let start = place(text, "total", 2);
        // Anywhere in the name, and just after it, as the editor sends it.
        for offset in start..=start + 5 {
            assert!(index.at(file, offset).is_some(), "nothing at {offset}");
        }
        assert!(index.at(file, start - 1).is_none(), "the `(` before the name");
    }

    #[test]
    fn a_function_gives_its_signature() {
        assert_eq!(
            shown("fun double(n in Int) in Int = n * 2\nshow(double(3))\n", "double", 2),
            "fun double(n in Int) in Int"
        );
        // A return type that the checker infers is known once the body is checked (C12).
        assert_eq!(
            shown("fun double(n in Int) = n * 2\nshow(double(3))\n", "double", 1),
            "fun double(n in Int) in Int"
        );
        // A function that no call reaches is indexed too.
        assert_eq!(
            shown("fun greet(who in Text) in Text = who\n", "greet", 1),
            "fun greet(who in Text) in Text"
        );
        assert_eq!(shown("fun double(n in Int) = n * 2\nshow(double(3))\n", "n", 2), "n in Int");
    }

    #[test]
    fn a_method_a_structure_and_its_fields() {
        let text = concat!(
            "struct Student:\n    name in Text\n    grade in Float\n;\n",
            "fun Student.passes() in Bool = self.grade >= 10.0\n",
            "let s = Student(\"Lea\", 12.0)\nshow(s.passes())\nshow(s.grade)\n"
        );
        assert_eq!(shown(text, "Student", 1), "structure Student");
        assert_eq!(shown(text, "grade", 1), "Student.grade in Float");
        assert_eq!(shown(text, "grade", 2), "Student.grade in Float");
        assert_eq!(shown(text, "passes", 2), "fun Student.passes() in Bool");
        // `self.grade`, `s.grade` and the declaration are the same field.
        assert_eq!(uses(text, "grade", 1).len(), 3);
    }

    #[test]
    fn a_captured_variable_is_the_one_the_program_declares() {
        let text = "let step = 2\nfun add(n in Int) in Int = n + step\nshow(add(1))\n";
        let (index, file) = indexed(text);
        let (occurrence, definition) = index.at(file, place(text, "step", 2)).expect("a name here");
        assert_eq!(definition.span.start, place(text, "step", 1));
        assert_eq!(index.uses(occurrence.definition).count(), 2);
    }

    #[test]
    fn a_program_with_errors_still_has_an_index() {
        let text = "let total = 1\nshow(totl)\nshow(total)\n";
        let mut map = SourceMap::new();
        let id = map.add("t.lion", text);
        let lexed = lion_syntax::lex(id, text);
        let parsed = lion_syntax::parse(text, &lexed.tokens);
        let checked = crate::check(&parsed.module);
        assert!(checked.program.is_none(), "the program does not compile");
        let (_, definition) = checked.index.at(id, place(text, "total", 2)).expect("a name here");
        assert_eq!(definition.signature, "let total in Int");
        // The misspelt name designates nothing.
        assert!(checked.index.at(id, place(text, "totl", 1)).is_none());
    }
}
