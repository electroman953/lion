/// Identifies a file registered in a [`SourceMap`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SourceId(u32);

/// A half-open byte range `start..end` inside one source file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Span {
    pub source: SourceId,
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(source: SourceId, start: usize, end: usize) -> Span {
        debug_assert!(start <= end);
        Span { source, start: start as u32, end: end as u32 }
    }

    /// The smallest span covering both `self` and `other`, which must be in the same file.
    pub fn to(self, other: Span) -> Span {
        debug_assert_eq!(self.source, other.source);
        Span { source: self.source, start: self.start.min(other.start), end: self.end.max(other.end) }
    }
}

/// The text of one source file, with an index of its line starts.
pub struct SourceFile {
    name: String,
    text: String,
    line_starts: Vec<u32>,
}

impl SourceFile {
    fn new(name: String, text: String) -> SourceFile {
        let mut line_starts = vec![0];
        line_starts.extend(text.match_indices('\n').map(|(i, _)| i as u32 + 1));
        SourceFile { name, text, line_starts }
    }

    /// The name used in messages, usually the path given on the command line.
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// The 1-based line containing byte `offset`.
    pub fn line_of(&self, offset: u32) -> usize {
        match self.line_starts.binary_search(&offset) {
            Ok(index) => index + 1,
            Err(index) => index,
        }
    }

    /// The 1-based line and column of byte `offset`; columns count characters, not bytes.
    pub fn line_col(&self, offset: u32) -> (usize, usize) {
        let line = self.line_of(offset);
        let start = self.line_start(line) as usize;
        let offset = (offset as usize).clamp(start, self.text.len());
        (line, self.text[start..offset].chars().count() + 1)
    }

    /// Byte offset of the first character of a 1-based line.
    pub fn line_start(&self, line: usize) -> u32 {
        self.line_starts[line - 1]
    }

    /// The text of a 1-based line, without its line terminator.
    pub fn line_text(&self, line: usize) -> &str {
        let start = self.line_start(line) as usize;
        let end = self.line_starts.get(line).map_or(self.text.len(), |&next| next as usize - 1);
        let text = &self.text[start..end];
        text.strip_suffix('\r').unwrap_or(text)
    }
}

/// All the source files of one compilation.
#[derive(Default)]
pub struct SourceMap {
    files: Vec<SourceFile>,
}

impl SourceMap {
    pub fn new() -> SourceMap {
        SourceMap::default()
    }

    pub fn add(&mut self, name: impl Into<String>, text: impl Into<String>) -> SourceId {
        self.files.push(SourceFile::new(name.into(), text.into()));
        SourceId(self.files.len() as u32 - 1)
    }

    pub fn get(&self, id: SourceId) -> &SourceFile {
        &self.files[id.0 as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_and_columns() {
        let mut map = SourceMap::new();
        let id = map.add("t.lion", "let x = 1\nlet é = 2\r\n\nend");
        let file = map.get(id);
        assert_eq!(file.line_col(0), (1, 1));
        assert_eq!(file.line_col(4), (1, 5));
        assert_eq!(file.line_col(10), (2, 1));
        // `é` is two bytes but one column.
        assert_eq!(file.line_col(17), (2, 7));
        assert_eq!(file.line_text(2), "let é = 2");
        assert_eq!(file.line_text(3), "");
        assert_eq!(file.line_text(4), "end");
        assert_eq!(file.line_col(file.text().len() as u32), (4, 4));
    }
}
