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
    fn a_list_of_objects_gives_rows() {
        let table = rows(r#"[{"name": "Léa", "grade": 14, "tags": [], "name": "Lea"}]"#).unwrap();
        assert_eq!(table[0].columns, ["name", "grade", "tags"]);
        assert_eq!(table[0].values, ["Lea", "14", ""]);
        assert_eq!(table[0].problems[2], "the value of `tags` is a list, not a text");
        assert_eq!(rows("{}").err().unwrap(), "the JSON text is not a list of objects");
        assert_eq!(rows("[1]").err().unwrap(), "the element 1 of the JSON list is a number, not an object");
    }
}
