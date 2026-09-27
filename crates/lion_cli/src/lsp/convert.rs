//! Between the values of Lion and those of the protocol: addresses of files, positions
//! in UTF-16 code units, and diagnostics.

use std::path::{Path, PathBuf};

use lion_diagnostics::{Diagnostic, Label, Severity, SourceFile, Span};
use lion_runtime::json::Json;

/// The path of a `file:` address; `None` for another kind of address.
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // `file:///path`, or `file://localhost/path`.
    let rest = if rest.starts_with('/') { rest } else { rest.strip_prefix("localhost")? };
    let bytes = rest.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let hex = |byte: u8| (byte as char).to_digit(16);
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => match (hex(bytes[index + 1]), hex(bytes[index + 2])) {
                (Some(high), Some(low)) => {
                    decoded.push((high * 16 + low) as u8);
                    index += 3;
                }
                _ => return None,
            },
            b'%' => return None,
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(decoded).ok().map(PathBuf::from)
}

/// The `file:` address of an absolute path.
pub fn path_to_uri(path: &Path) -> String {
    let mut uri = "file://".to_string();
    for &byte in path.display().to_string().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            uri.push(byte as char);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

pub fn position(file: &SourceFile, offset: u32) -> Json {
    let (line, character) = file.utf16_position(offset);
    Json::object([("line", Json::int(line as i64)), ("character", Json::int(character as i64))])
}

pub fn range(file: &SourceFile, span: Span) -> Json {
    offsets(file, span.start, span.end)
}

/// The range of the bytes `start..end`.
pub fn offsets(file: &SourceFile, start: u32, end: u32) -> Json {
    Json::object([("start", position(file, start)), ("end", position(file, end))])
}

/// The byte offset of a position of the protocol, `None` when it is not one.
pub fn offset(file: &SourceFile, position: &Json) -> Option<u32> {
    let line = position.get("line")?.as_int()?;
    let character = position.get("character")?.as_int()?;
    Some(file.utf16_offset(u32::try_from(line).ok()?, u32::try_from(character).ok()?))
}

/// A diagnostic of the protocol, shown on `label` (a label of `diagnostic`, or `None` for
/// the start of the file). `locate` gives the file and the address of a span, or `None`
/// for a file that the editor does not show, such as a module of the standard library.
pub fn diagnostic<'a>(
    diagnostic: &Diagnostic,
    file: &SourceFile,
    label: Option<&Label>,
    locate: &dyn Fn(Span) -> Option<(&'a SourceFile, String)>,
) -> Json {
    let severity = match diagnostic.severity {
        Severity::Error | Severity::Bug | Severity::Internal => 1,
        Severity::Warning => 2,
        Severity::Alert => 3,
    };
    let (start, end) = label.map_or((0, 0), |label| (label.span.start, label.span.end));
    let mut message = diagnostic.message.clone();
    if let Some(label) = label.filter(|label| label.primary && !label.message.is_empty()) {
        message.push('\n');
        message.push_str(&label.message);
    }
    for note in &diagnostic.notes {
        message.push('\n');
        message.push_str(note);
    }
    for help in &diagnostic.help {
        message.push_str("\nhelp: ");
        message.push_str(help);
    }
    // The other labels, where the editor can show them.
    let related: Vec<Json> = diagnostic
        .labels
        .iter()
        .filter(|other| label.is_none_or(|label| !std::ptr::eq(*other, label)))
        .filter_map(|other| {
            let (file, uri) = locate(other.span)?;
            let text = if other.message.is_empty() { "here".to_string() } else { other.message.clone() };
            Some(Json::object([
                ("location", Json::object([("uri", Json::text(uri)), ("range", range(file, other.span))])),
                ("message", Json::text(text)),
            ]))
        })
        .collect();
    let mut members = vec![
        ("range".to_string(), offsets(file, start, end)),
        ("severity".to_string(), Json::int(severity)),
        ("source".to_string(), Json::text("lion")),
        ("message".to_string(), Json::text(message)),
    ];
    if !related.is_empty() {
        members.push(("relatedInformation".to_string(), Json::List(related)));
    }
    Json::Object(members)
}

#[cfg(test)]
mod tests {
    use lion_diagnostics::SourceMap;

    use super::*;

    #[test]
    fn addresses_of_files() {
        let path = Path::new("/home/léa/mes projets/a+b.lion");
        let uri = path_to_uri(path);
        assert_eq!(uri, "file:///home/l%C3%A9a/mes%20projets/a%2Bb.lion");
        assert_eq!(uri_to_path(&uri).as_deref(), Some(path));
        assert_eq!(uri_to_path("file://localhost/a/b.lion").as_deref(), Some(Path::new("/a/b.lion")));
        assert_eq!(uri_to_path("file:///a/b%3a.lion").as_deref(), Some(Path::new("/a/b:.lion")));
        assert_eq!(uri_to_path("untitled:Untitled-1"), None);
        assert_eq!(uri_to_path("file:///a%2"), None);
    }

    #[test]
    fn diagnostics_keep_their_notes_and_other_labels() {
        let mut map = SourceMap::new();
        let text = "let é = 1\nlet x = é + \"a\"\n";
        let id = map.add("/p/t.lion", text);
        let file = map.get(id);
        let at = text.find('"').unwrap();
        let diagnostic = Diagnostic::error("mismatched types")
            .with_primary(Span::new(id, at, at + 3), "this is a Text")
            .with_secondary(Span::new(id, 4, 6), "")
            .with_note("expected: Int")
            .with_help("convert it");
        let primary = &diagnostic.labels[0];
        let locate = |_: Span| Some((file, "file:///p/t.lion".to_string()));
        let json = super::diagnostic(&diagnostic, file, Some(primary), &locate);
        assert_eq!(
            json.to_string(),
            concat!(
                r#"{"range":{"start":{"line":1,"character":12},"end":{"line":1,"character":15}},"#,
                r#""severity":1,"source":"lion","message":"mismatched types\nthis is a Text\nexpected: Int\nhelp: convert it","#,
                r#""relatedInformation":[{"location":{"uri":"file:///p/t.lion","range":{"start":{"line":0,"character":4},"end":{"line":0,"character":5}}},"message":"here"}]}"#
            )
        );
    }
}
