//! Hover, go to definition, find references and highlight, answered from the index of
//! names that the checker builds (C103).
//!
//! A program keeps the index of its last check that reached the checker. While a file
//! has a syntax error, the checker does not run (§22.1) and the editor still answers
//! from the names of the last text that parsed, which is what typing needs.

use std::collections::HashMap;

use lion_diagnostics::{SourceId, SourceMap, Span};
use lion_runtime::json::Json;
use lion_sema::{DefKind, Definition, Index, Occurrence};

use super::convert;

/// The names of one program, kept between checks.
pub struct Names {
    /// The key of the script, which tells one program from another.
    pub key: String,
    sources: SourceMap,
    index: Index,
    /// The address of each file the editor can open; a module of the standard library
    /// has none.
    uris: HashMap<SourceId, String>,
}

impl Names {
    pub fn new(key: String, sources: SourceMap, index: Index, uris: HashMap<SourceId, String>) -> Names {
        Names { key, sources, index, uris }
    }

    /// Whether the program has a file at this address.
    pub fn holds(&self, uri: &str) -> bool {
        self.uris.values().any(|known| known == uri)
    }

    /// The file at this address.
    fn source(&self, uri: &str) -> Option<SourceId> {
        self.uris.iter().find(|(_, known)| *known == uri).map(|(&source, _)| source)
    }

    /// The name at a position of the protocol, and what it designates.
    fn at(&self, uri: &str, position: &Json) -> Option<(&Occurrence, &Definition)> {
        let source = self.source(uri)?;
        let offset = convert::offset(self.sources.get(source), position)?;
        self.index.at(source, offset)
    }

    /// `{ "uri": ..., "range": ... }`: where a span is, for the editor.
    fn location(&self, span: Span) -> Option<Json> {
        let uri = self.uris.get(&span.source)?;
        let file = self.sources.get(span.source);
        Some(Json::object([("uri", Json::text(uri)), ("range", convert::range(file, span))]))
    }

    /// `textDocument/hover`: the declaration of the name under the cursor.
    pub fn hover(&self, uri: &str, position: &Json) -> Option<Json> {
        let (occurrence, definition) = self.at(uri, position)?;
        let file = self.sources.get(occurrence.span.source);
        let mut text = format!("```lion\n{}\n```", definition.signature);
        if let Some(note) = origin(definition, self.uris.contains_key(&definition.span.source)) {
            text.push_str("\n\n");
            text.push_str(&note);
        }
        let contents = Json::object([("kind", Json::text("markdown")), ("value", Json::text(text))]);
        Some(Json::object([("contents", contents), ("range", convert::range(file, occurrence.span))]))
    }

    /// `textDocument/definition`: where the program declares the name.
    pub fn definition(&self, uri: &str, position: &Json) -> Option<Json> {
        let (_, definition) = self.at(uri, position)?;
        self.location(definition.span)
    }

    /// `textDocument/references`: every name that designates the same declaration.
    pub fn references(&self, uri: &str, position: &Json, with_declaration: bool) -> Option<Json> {
        let (occurrence, definition) = self.at(uri, position)?;
        let declaration = definition.span;
        let places = self
            .index
            .uses(occurrence.definition)
            .filter(|span| with_declaration || *span != declaration)
            .filter_map(|span| self.location(span))
            .collect();
        Some(Json::List(places))
    }

    /// `textDocument/documentHighlight`: the same name, in the file being read.
    pub fn highlight(&self, uri: &str, position: &Json) -> Option<Json> {
        let (occurrence, _) = self.at(uri, position)?;
        let source = occurrence.span.source;
        let file = self.sources.get(source);
        let places = self
            .index
            .uses(occurrence.definition)
            .filter(|span| span.source == source)
            .map(|span| Json::object([("range", convert::range(file, span)), ("kind", Json::int(1))]))
            .collect();
        Some(Json::List(places))
    }
}

/// What to add under the signature: where a declaration the editor cannot open comes
/// from, so that hover says why "go to definition" leads nowhere.
fn origin(definition: &Definition, reachable: bool) -> Option<String> {
    if reachable {
        return None;
    }
    let what = match definition.kind {
        DefKind::Function | DefKind::Method => "function",
        DefKind::Structure => "structure",
        DefKind::Field => "field",
        DefKind::Type => "type",
        DefKind::Variable | DefKind::Parameter => "name",
    };
    Some(format!("a {what} of the standard library"))
}
