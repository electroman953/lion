use std::fmt::Write;

use crate::{Diagnostic, Label, SourceFile, SourceMap};

/// Renders a diagnostic as plain text:
///
/// ```text
/// error: mismatched types
///   --> main.lion:4:13
///    |
///  4 | let x = 3 + "hello"
///    |             ^^^^^^^ this is a Text
///    |
///    = expected: Int
/// ```
pub fn render(diagnostic: &Diagnostic, sources: &SourceMap) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{}: {}", diagnostic.severity.title(), diagnostic.message);

    let primary = diagnostic.labels.iter().find(|label| label.primary);
    let Some(primary) = primary.or(diagnostic.labels.first()) else {
        render_footer(&mut out, 0, diagnostic);
        return out;
    };
    let file = sources.get(primary.span.source);
    let labels: Vec<&Label> =
        diagnostic.labels.iter().filter(|label| label.span.source == primary.span.source).collect();
    let mut lines: Vec<usize> = labels.iter().map(|label| file.line_of(label.span.start)).collect();
    lines.sort_unstable();
    lines.dedup();
    let width = lines.last().map_or(1, |line| line.to_string().len());

    let (line, column) = file.line_col(primary.span.start);
    let _ = writeln!(out, "{}--> {}:{}:{}", " ".repeat(width + 1), file.name(), line, column);
    let gutter = " ".repeat(width + 2);
    let _ = writeln!(out, "{gutter}|");
    let mut previous: Option<usize> = None;
    for &line in &lines {
        if previous.is_some_and(|previous| line > previous + 1) {
            let _ = writeln!(out, "{}...", " ".repeat(width + 1));
        }
        previous = Some(line);
        let text = file.line_text(line);
        if text.is_empty() {
            let _ = writeln!(out, " {line:>width$} |");
        } else {
            let _ = writeln!(out, " {line:>width$} | {text}");
        }
        let mut here: Vec<&&Label> =
            labels.iter().filter(|label| file.line_of(label.span.start) == line).collect();
        here.sort_by_key(|label| label.span.start);
        for label in here {
            let _ = writeln!(out, "{gutter}| {}", underline(file, line, label));
        }
    }
    render_footer(&mut out, width, diagnostic);
    out
}

/// The marker line under a label: spaces up to the label, then `^^^` or `---`.
fn underline(file: &SourceFile, line: usize, label: &Label) -> String {
    let text = file.line_text(line);
    let line_start = file.line_start(line) as usize;
    let start = (label.span.start as usize).saturating_sub(line_start).min(text.len());
    let end = (label.span.end as usize).saturating_sub(line_start).clamp(start, text.len());
    // Keep tabs so the marker lines up with the excerpt above it.
    let mut marker: String = text[..start].chars().map(|c| if c == '\t' { '\t' } else { ' ' }).collect();
    let mark = if label.primary { "^" } else { "-" };
    marker.push_str(&mark.repeat(text[start..end].chars().count().max(1)));
    if !label.message.is_empty() {
        marker.push(' ');
        marker.push_str(&label.message);
    }
    marker
}

fn render_footer(out: &mut String, width: usize, diagnostic: &Diagnostic) {
    if diagnostic.notes.is_empty() && diagnostic.help.is_empty() {
        return;
    }
    let gutter = " ".repeat(width + 2);
    if !diagnostic.labels.is_empty() {
        let _ = writeln!(out, "{gutter}|");
    }
    for note in &diagnostic.notes {
        let _ = writeln!(out, "{gutter}= {note}");
    }
    for help in &diagnostic.help {
        let _ = writeln!(out, "{gutter}= help: {help}");
    }
}

#[cfg(test)]
mod tests {
    use crate::{Diagnostic, SourceMap, Span};

    use super::render;

    #[test]
    fn renders_excerpt_with_notes() {
        let mut map = SourceMap::new();
        let text = "let a = 1\nlet x = 3 + \"hello\"\n";
        let id = map.add("main.lion", text);
        let start = text.find('"').unwrap();
        let diagnostic = Diagnostic::error("mismatched types")
            .with_primary(Span::new(id, start, start + 7), "this is a Text")
            .with_note("expected: Int")
            .with_help("convert it");
        let expected = "\
error: mismatched types
  --> main.lion:2:13
   |
 2 | let x = 3 + \"hello\"
   |             ^^^^^^^ this is a Text
   |
   = expected: Int
   = help: convert it
";
        assert_eq!(render(&diagnostic, &map), expected);
    }

    #[test]
    fn renders_secondary_labels_and_gaps() {
        let mut map = SourceMap::new();
        let text = "let e in Text\n\n\nshow(e)\n";
        let id = map.add("t.lion", text);
        let use_at = text.find("e)").unwrap();
        let diagnostic = Diagnostic::error("`e` is used before it has a value")
            .with_primary(Span::new(id, use_at, use_at + 1), "used here")
            .with_secondary(Span::new(id, 4, 5), "declared here without a value");
        let expected = "\
error: `e` is used before it has a value
  --> t.lion:4:6
   |
 1 | let e in Text
   |     - declared here without a value
  ...
 4 | show(e)
   |      ^ used here
";
        assert_eq!(render(&diagnostic, &map), expected);
    }

    #[test]
    fn renders_without_location() {
        let map = SourceMap::new();
        let diagnostic = Diagnostic::error("cannot read `x.lion`").with_note("file not found");
        assert_eq!(render(&diagnostic, &map), "error: cannot read `x.lion`\n  = file not found\n");
    }
}
