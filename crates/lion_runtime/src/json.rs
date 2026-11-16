//! JSON texts (RFC 8259), read strictly: the module `json` of the standard library reads
//! a list of objects as rows (§23, D37, C96).

/// A JSON value. A number keeps the text written, which `as Int` or `as Float` converts.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(String),
    Text(String),
    List(Vec<Json>),
    /// The members, in the order of the text.
    Object(Vec<(String, Json)>),
}

/// The value of a JSON text, or why it is not one, with the line and the column.
pub fn parse(text: &str) -> Result<Json, String> {
    let mut parser = Parser { chars: text.chars().collect(), pos: 0 };
    parser.skip_spaces();
    let value = parser.value()?;
    parser.skip_spaces();
    if parser.pos < parser.chars.len() {
        return Err(parser.error("the text goes on after the value"));
    }
    Ok(value)
}

impl Json {
    /// The value of the member `name` of an object.
    pub fn get(&self, name: &str) -> Option<&Json> {
        match self {
            Json::Object(members) => {
                members.iter().find(|(member, _)| member == name).map(|(_, value)| value)
            }
            _ => None,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Json::Text(text) => Some(text),
            _ => None,
        }
    }

    /// A number without a fraction or an exponent that fits in 64 bits.
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Json::Number(number) => number.parse().ok(),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[Json]> {
        match self {
            Json::List(items) => Some(items),
            _ => None,
        }
    }

    /// An object of these members, in this order.
    pub fn object<const N: usize>(members: [(&str, Json); N]) -> Json {
        Json::Object(members.into_iter().map(|(name, value)| (name.to_string(), value)).collect())
    }

    pub fn text(text: impl Into<String>) -> Json {
        Json::Text(text.into())
    }

    pub fn int(value: i64) -> Json {
        Json::Number(value.to_string())
    }
}

impl Json {
    /// `null`, `bool`, `number`, `text`, `list` or `object`.
    pub fn kind(&self) -> &'static str {
        match self {
            Json::Null => "null",
            Json::Bool(_) => "bool",
            Json::Number(_) => "number",
            Json::Text(_) => "text",
            Json::List(_) => "list",
            Json::Object(_) => "object",
        }
    }

    /// The value at `path`, such as `data.items[1].name`; lists count from 1, as in
    /// Lion (§16.2).
    pub fn follow(&self, path: &str) -> Result<&Json, String> {
        let mut current = self;
        let mut walked = String::new();
        let mut rest = path.trim();
        while !rest.is_empty() {
            if let Some(after) = rest.strip_prefix('[') {
                let close = after.find(']').ok_or(format!("`[` in the path `{path}` has no `]`"))?;
                let index: usize = after[..close].trim().parse().map_err(|_| {
                    format!("`[{}]` in the path `{path}` is not a position from 1", &after[..close])
                })?;
                current = current.element(index).map_err(|problem| at(&walked, problem))?;
                walked.push_str(&format!("[{index}]"));
                rest = &after[close + 1..];
            } else {
                let rest_name = rest.strip_prefix('.').unwrap_or(rest);
                let end = rest_name.find(['.', '[']).unwrap_or(rest_name.len());
                let name = &rest_name[..end];
                if name.is_empty() {
                    return Err(format!("the path `{path}` has an empty name"));
                }
                current = current.member(name).map_err(|problem| at(&walked, problem))?;
                if !walked.is_empty() {
                    walked.push('.');
                }
                walked.push_str(name);
                rest = &rest_name[end..];
            }
        }
        Ok(current)
    }

    /// The member `name` of an object, or why there is none.
    pub fn member(&self, name: &str) -> Result<&Json, String> {
        match self {
            Json::Object(members) => members
                .iter()
                .rev()
                .find(|(member, _)| member == name)
                .map(|(_, value)| value)
                .ok_or_else(|| format!("there is no member `{name}`")),
            other => Err(format!("the value is a {}, not an object with a member `{name}`", other.kind())),
        }
    }

    /// The element at `index`, from 1, of a list, or why there is none.
    pub fn element(&self, index: usize) -> Result<&Json, String> {
        match self {
            Json::List(items) if index >= 1 && index <= items.len() => Ok(&items[index - 1]),
            Json::List(items) => Err(format!("there is no element {index} in a list of {}", items.len())),
            other => Err(format!("the value is a {}, not a list", other.kind())),
        }
    }

    /// The JSON text, indented by two spaces.
    pub fn pretty(&self) -> String {
        let mut out = String::new();
        self.write_pretty(&mut out, 0);
        out
    }

    fn write_pretty(&self, out: &mut String, depth: usize) {
        let indent = |out: &mut String, depth: usize| out.extend(std::iter::repeat_n("  ", depth));
        match self {
            Json::List(items) if !items.is_empty() => {
                out.push_str("[\n");
                for (index, item) in items.iter().enumerate() {
                    indent(out, depth + 1);
                    item.write_pretty(out, depth + 1);
                    out.push_str(if index + 1 < items.len() { ",\n" } else { "\n" });
                }
                indent(out, depth);
                out.push(']');
            }
            Json::Object(members) if !members.is_empty() => {
                out.push_str("{\n");
                for (index, (name, value)) in members.iter().enumerate() {
                    indent(out, depth + 1);
                    out.push_str(&Json::Text(name.clone()).to_string());
                    out.push_str(": ");
                    value.write_pretty(out, depth + 1);
                    out.push_str(if index + 1 < members.len() { ",\n" } else { "\n" });
                }
                indent(out, depth);
                out.push('}');
            }
            other => out.push_str(&other.to_string()),
        }
    }
}

fn at(walked: &str, problem: String) -> String {
    if walked.is_empty() { problem } else { format!("at `{walked}`: {problem}") }
}

static LAST: std::sync::Mutex<Option<(String, Json)>> = std::sync::Mutex::new(None);

/// The value of a JSON text known to be valid, read once for several uses in a row.
pub fn cached<R>(text: &str, f: impl FnOnce(&Json) -> R) -> R {
    let mut last = LAST.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if last.as_ref().is_none_or(|(source, _)| source != text) {
        let value = parse(text).unwrap_or(Json::Null);
        *last = Some((text.to_string(), value));
    }
    f(&last.as_ref().expect("read above").1)
}

/// The JSON text of a value, on one line.
impl std::fmt::Display for Json {
    fn fmt(&self, out: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Json::Null => out.write_str("null"),
            Json::Bool(value) => write!(out, "{value}"),
            Json::Number(number) => out.write_str(number),
            Json::Text(text) => write_text(out, text),
            Json::List(items) => {
                out.write_str("[")?;
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.write_str(",")?;
                    }
                    write!(out, "{item}")?;
                }
                out.write_str("]")
            }
            Json::Object(members) => {
                out.write_str("{")?;
                for (index, (name, value)) in members.iter().enumerate() {
                    if index > 0 {
                        out.write_str(",")?;
                    }
                    write_text(out, name)?;
                    write!(out, ":{value}")?;
                }
                out.write_str("}")
            }
        }
    }
}

fn write_text(out: &mut std::fmt::Formatter, text: &str) -> std::fmt::Result {
    out.write_str("\"")?;
    for c in text.chars() {
        match c {
            '"' => out.write_str("\\\"")?,
            '\\' => out.write_str("\\\\")?,
            '\n' => out.write_str("\\n")?,
            '\r' => out.write_str("\\r")?,
            '\t' => out.write_str("\\t")?,
            c if (c as u32) < 0x20 => write!(out, "\\u{:04x}", c as u32)?,
            c => write!(out, "{c}")?,
        }
    }
    out.write_str("\"")
}

/// One row of a list of objects: the names of its members, their values as texts, and,
/// for a value that is not a text, a number or a Bool, why it cannot be read.
pub struct Row {
    pub columns: Vec<String>,
    pub values: Vec<String>,
    pub problems: Vec<String>,
}

/// The rows of a JSON text that is a list of objects. A name given twice keeps its last
/// value, at the place of the first.
pub fn rows(text: &str) -> Result<Vec<Row>, String> {
    let Json::List(items) = parse(text)? else {
        return Err("the JSON text is not a list of objects".to_string());
    };
    let mut rows = Vec::new();
    for (number, item) in items.into_iter().enumerate() {
        let Json::Object(members) = item else {
            return Err(format!(
                "the element {} of the JSON list is {}, not an object",
                number + 1,
                describe(&item)
            ));
        };
        let mut row = Row { columns: Vec::new(), values: Vec::new(), problems: Vec::new() };
        for (name, value) in members {
            let (text, problem) = match &value {
                Json::Text(text) | Json::Number(text) => (text.clone(), String::new()),
                Json::Bool(value) => (value.to_string(), String::new()),
                other => (String::new(), format!("the value of `{name}` is {}, not a text", describe(other))),
            };
            match row.columns.iter().position(|column| *column == name) {
                Some(position) => {
                    row.values[position] = text;
                    row.problems[position] = problem;
                }
                None => {
                    row.columns.push(name);
                    row.values.push(text);
                    row.problems.push(problem);
                }
            }
        }
        rows.push(row);
    }
    Ok(rows)
}

fn describe(value: &Json) -> &'static str {
    match value {
        Json::Null => "null",
        Json::Bool(_) => "a Bool",
        Json::Number(_) => "a number",
        Json::Text(_) => "a text",
        Json::List(_) => "a list",
        Json::Object(_) => "an object",
    }
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    fn error(&self, what: &str) -> String {
        let before = &self.chars[..self.pos.min(self.chars.len())];
        let line = before.iter().filter(|&&c| c == '\n').count() + 1;
        let column = before.iter().rev().take_while(|&&c| c != '\n').count() + 1;
        format!("invalid JSON at line {line}, column {column}: {what}")
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn skip_spaces(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t' | '\n' | '\r')) {
            self.pos += 1;
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        match self.peek() {
            Some('{') => self.object(),
            Some('[') => self.list(),
            Some('"') => self.text().map(Json::Text),
            Some('-' | '0'..='9') => self.number(),
            Some('t') => self.word("true", Json::Bool(true)),
            Some('f') => self.word("false", Json::Bool(false)),
            Some('n') => self.word("null", Json::Null),
            Some(_) => Err(self.error("expected a value")),
            None => Err(self.error("the text ends where a value is expected")),
        }
    }

    fn word(&mut self, word: &str, value: Json) -> Result<Json, String> {
        for expected in word.chars() {
            if self.peek() != Some(expected) {
                return Err(self.error("expected a value"));
            }
            self.pos += 1;
        }
        Ok(value)
    }

    fn object(&mut self) -> Result<Json, String> {
        self.pos += 1;
        let mut members = Vec::new();
        self.skip_spaces();
        if self.peek() == Some('}') {
            self.pos += 1;
            return Ok(Json::Object(members));
        }
        loop {
            self.skip_spaces();
            if self.peek() != Some('"') {
                return Err(self.error("expected the name of a member, in quotes"));
            }
            let name = self.text()?;
            self.skip_spaces();
            if self.peek() != Some(':') {
                return Err(self.error("expected `:` after the name of a member"));
            }
            self.pos += 1;
            self.skip_spaces();
            members.push((name, self.value()?));
            self.skip_spaces();
            match self.peek() {
                Some(',') => self.pos += 1,
                Some('}') => {
                    self.pos += 1;
                    return Ok(Json::Object(members));
                }
                _ => return Err(self.error("expected `,` or `}`")),
            }
        }
    }

    fn list(&mut self) -> Result<Json, String> {
        self.pos += 1;
        let mut items = Vec::new();
        self.skip_spaces();
        if self.peek() == Some(']') {
            self.pos += 1;
            return Ok(Json::List(items));
        }
        loop {
            self.skip_spaces();
            items.push(self.value()?);
            self.skip_spaces();
            match self.peek() {
                Some(',') => self.pos += 1,
                Some(']') => {
                    self.pos += 1;
                    return Ok(Json::List(items));
                }
                _ => return Err(self.error("expected `,` or `]`")),
            }
        }
    }

    fn text(&mut self) -> Result<String, String> {
        self.pos += 1;
        let mut text = String::new();
        loop {
            let Some(c) = self.peek() else { return Err(self.error("a text is not closed by `\"`")) };
            self.pos += 1;
            match c {
                '"' => return Ok(text),
                '\\' => {
                    let Some(escape) = self.peek() else {
                        return Err(self.error("a text is not closed by `\"`"));
                    };
                    self.pos += 1;
                    match escape {
                        '"' => text.push('"'),
                        '\\' => text.push('\\'),
                        '/' => text.push('/'),
                        'b' => text.push('\u{8}'),
                        'f' => text.push('\u{c}'),
                        'n' => text.push('\n'),
                        'r' => text.push('\r'),
                        't' => text.push('\t'),
                        'u' => {
                            let first = self.hex4()?;
                            let code = if (0xD800..0xDC00).contains(&first) {
                                // A character beyond U+FFFF, written as two halves.
                                if self.peek() != Some('\\') || self.chars.get(self.pos + 1) != Some(&'u') {
                                    return Err(self.error("a `\\u` escape lacks its second half"));
                                }
                                self.pos += 2;
                                let second = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&second) {
                                    return Err(self.error("a `\\u` escape has a wrong second half"));
                                }
                                0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00)
                            } else {
                                first
                            };
                            match char::from_u32(code) {
                                Some(c) => text.push(c),
                                None => return Err(self.error("a `\\u` escape is not a character")),
                            }
                        }
                        _ => return Err(self.error("unknown escape in a text")),
                    }
                }
                c if (c as u32) < 0x20 => return Err(self.error("a text holds a control character")),
                c => text.push(c),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let mut code = 0;
        for _ in 0..4 {
            let Some(digit) = self.peek().and_then(|c| c.to_digit(16)) else {
                return Err(self.error("a `\\u` escape takes four hexadecimal digits"));
            };
            code = code * 16 + digit;
            self.pos += 1;
        }
        Ok(code)
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.pos;
        if self.peek() == Some('-') {
            self.pos += 1;
        }
        match self.peek() {
            Some('0') => self.pos += 1,
            Some('1'..='9') => self.digits(),
            _ => return Err(self.error("a number needs a digit")),
        }
        if self.peek() == Some('.') {
            self.pos += 1;
            if !matches!(self.peek(), Some('0'..='9')) {
                return Err(self.error("a number needs a digit after `.`"));
            }
            self.digits();
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some('+' | '-')) {
                self.pos += 1;
            }
            if !matches!(self.peek(), Some('0'..='9')) {
                return Err(self.error("a number needs a digit in its exponent"));
            }
            self.digits();
        }
        Ok(Json::Number(self.chars[start..self.pos].iter().collect()))
    }

    fn digits(&mut self) {
        while matches!(self.peek(), Some('0'..='9')) {
            self.pos += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_read_strictly() {
        assert_eq!(
            parse(r#"{"a": [1, -2.5e3, true, null], "b": "xé\n"}"#),
            Ok(Json::Object(vec![
                (
                    "a".to_string(),
                    Json::List(vec![
                        Json::Number("1".to_string()),
                        Json::Number("-2.5e3".to_string()),
                        Json::Bool(true),
                        Json::Null
                    ])
                ),
                ("b".to_string(), Json::Text("xé\n".to_string())),
            ]))
        );
        assert_eq!(parse(r#""🦁""#), Ok(Json::Text("🦁".to_string())));
        assert_eq!(parse("[1,]"), Err("invalid JSON at line 1, column 4: expected a value".to_string()));
        assert_eq!(
            parse("{\n  \"a\" 1}"),
            Err("invalid JSON at line 2, column 7: expected `:` after the name of a member".to_string())
        );
        assert!(parse("01").is_err());
        assert!(parse("[1] 2").is_err());
    }

    #[test]
    fn values_are_written_back() {
        let text = r#"{"a":[1,-2.5e3,true,null],"b":"x\u00e9\n\"\\\u0001","c":{}}"#;
        let value = parse(text).unwrap();
        assert_eq!(value.to_string(), "{\"a\":[1,-2.5e3,true,null],\"b\":\"xé\\n\\\"\\\\\\u0001\",\"c\":{}}");
        assert_eq!(parse(&value.to_string()), Ok(value.clone()));
        assert_eq!(value.get("a").and_then(Json::as_list).map(<[Json]>::len), Some(4));
        assert_eq!(value.get("a").unwrap().as_list().unwrap()[0].as_int(), Some(1));
        assert_eq!(value.get("z"), None);
        assert_eq!(
            Json::object([("n", Json::int(3)), ("t", Json::text("é"))]).to_string(),
            r#"{"n":3,"t":"é"}"#
        );
    }

    #[test]
    fn a_list_of_objects_gives_rows() {
        let table = rows(r#"[{"name": "Léa", "grade": 14, "tags": [], "name": "Lea"}]"#).unwrap();
        assert_eq!(table[0].columns, ["name", "grade", "tags"]);
        assert_eq!(table[0].values, ["Lea", "14", ""]);
        assert_eq!(table[0].problems[2], "the value of `tags` is a list, not a text");
        assert_eq!(rows("{}").err().unwrap(), "the JSON text is not a list of objects");
        assert_eq!(rows("[1]").err().unwrap(), "the element 1 of the JSON list is a number, not an object");
    }
}

#[cfg(test)]
mod navigation_tests {
    use super::*;

    #[test]
    fn follows_paths_and_writes_indented() {
        let value =
            parse(r#"{"data": {"items": [{"name": "Léa"}, {"name": "Max", "tags": []}]}, "n": 2}"#).unwrap();
        assert_eq!(value.follow("data.items[2].name").unwrap(), &Json::text("Max"));
        assert_eq!(value.follow("n").unwrap(), &Json::int(2));
        assert_eq!(
            value.follow("data.items[3]").unwrap_err(),
            "at `data.items`: there is no element 3 in a list of 2"
        );
        assert_eq!(
            value.follow("n.x").unwrap_err(),
            "at `n`: the value is a number, not an object with a member `x`"
        );
        assert_eq!(
            value.follow("data.items[0]").unwrap_err(),
            "at `data.items`: there is no element 0 in a list of 2"
        );
        let small = parse(r#"{"a": [1, {}], "b": []}"#).unwrap();
        assert_eq!(small.pretty(), "{\n  \"a\": [\n    1,\n    {}\n  ],\n  \"b\": []\n}");
    }
}
