//! HTML pages (module `html`, C105): a tolerant reader, as browsers are, and CSS
//! selectors to find elements, without a dependency.
//!
//! The reader keeps where each element is written in the text, so an element found
//! keeps its exact HTML; searching inside an element reads that HTML again. Supported
//! selectors: `tag`, `*`, `#id`, `.class`, `[attr]`, `[attr=v]`, `[attr~=v]`,
//! `[attr^=v]`, `[attr$=v]`, `[attr*=v]`, `:first-child`, `:last-child`,
//! `:nth-child(n)`, `:not(…)`, the combinators ` ` (inside), `>` (child), `+` (just
//! after) and `~` (after), and `,`.

use std::sync::Mutex;

/// An element found: its tag, its text, its inner and outer HTML, its attributes.
#[derive(Debug, PartialEq)]
pub struct Element {
    pub tag: String,
    pub text: String,
    pub inner: String,
    pub outer: String,
    pub attributes: Vec<(String, String)>,
}

struct Node {
    tag: String,
    attributes: Vec<(String, String)>,
    parent: usize,
    children: Vec<usize>,
    start: usize,
    open_end: usize,
    inner_end: usize,
    end: usize,
}

struct Document {
    source: String,
    /// The node 0 is the document; the others are its elements, in the order of the text.
    nodes: Vec<Node>,
}

const VOID: &[&str] =
    &["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track", "wbr"];
const RAW: &[&str] = &["script", "style", "textarea", "title"];
const CLOSES_P: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "details",
    "div",
    "dl",
    "fieldset",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hr",
    "main",
    "nav",
    "ol",
    "p",
    "pre",
    "section",
    "table",
    "ul",
];
/// The tags whose text is apart from the text around them.
const BLOCK: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "br",
    "dd",
    "div",
    "dl",
    "dt",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hr",
    "li",
    "main",
    "nav",
    "ol",
    "option",
    "p",
    "pre",
    "section",
    "table",
    "td",
    "th",
    "tr",
    "ul",
];

enum Token {
    Text,
    Start { name: String, attributes: Vec<(String, String)>, closed: bool, start: usize, end: usize },
    End { name: String, start: usize, end: usize },
}

/// The tokens of the source, with the position after each.
fn tokens(source: &str, mut each: impl FnMut(Token, usize, usize)) {
    let bytes = source.as_bytes();
    let mut i = 0;
    let lower_find = |from: usize, part: &str| -> Option<usize> {
        let lower = part.as_bytes();
        bytes[from..].windows(lower.len()).position(|w| w.eq_ignore_ascii_case(lower)).map(|p| p + from)
    };
    while i < bytes.len() {
        if bytes[i] != b'<' {
            let next = source[i..].find('<').map_or(bytes.len(), |p| p + i);
            each(Token::Text, i, next);
            i = next;
            continue;
        }
        let rest = &source[i..];
        if rest.starts_with("<!--") {
            i = source[i + 4..].find("-->").map_or(bytes.len(), |p| i + 4 + p + 3);
            continue;
        }
        if rest.starts_with("<!") || rest.starts_with("<?") {
            i = source[i..].find('>').map_or(bytes.len(), |p| i + p + 1);
            continue;
        }
        let closing = rest.starts_with("</");
        let name_start = i + if closing { 2 } else { 1 };
        let name_len = bytes[name_start..]
            .iter()
            .take_while(|b| b.is_ascii_alphanumeric() || **b == b'-' || **b == b':')
            .count();
        if name_len == 0 {
            // A `<` that opens no tag is text.
            each(Token::Text, i, i + 1);
            i += 1;
            continue;
        }
        let name = source[name_start..name_start + name_len].to_ascii_lowercase();
        let (attributes, closed, end) = attributes(source, name_start + name_len);
        if closing {
            each(Token::End { name, start: i, end }, i, end);
            i = end;
            continue;
        }
        let raw = RAW.contains(&name.as_str()) && !closed;
        each(Token::Start { name: name.clone(), attributes, closed, start: i, end }, i, end);
        i = end;
        if raw {
            let close = lower_find(i, &format!("</{name}")).unwrap_or(bytes.len());
            if close > i {
                each(Token::Text, i, close);
            }
            i = close;
        }
    }
}

/// The attributes of a tag from `from`, whether it ends with `/>`, and the position
/// after its `>`.
fn attributes(source: &str, mut i: usize) -> (Vec<(String, String)>, bool, usize) {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut closed = false;
    loop {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            return (out, closed, bytes.len());
        }
        match bytes[i] {
            b'>' => return (out, closed, i + 1),
            b'/' => {
                closed = true;
                i += 1;
                continue;
            }
            _ => closed = false,
        }
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && !b"=>/".contains(&bytes[i]) {
            i += 1;
        }
        if i == start {
            i += 1;
            continue;
        }
        let name = source[start..i].to_ascii_lowercase();
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut value = String::new();
        if i < bytes.len() && bytes[i] == b'=' {
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < bytes.len() && (bytes[i] == b'"' || bytes[i] == b'\'') {
                let quote = bytes[i] as char;
                let end = source[i + 1..].find(quote).map_or(bytes.len(), |p| i + 1 + p);
                value = unescape(&source[i + 1..end]);
                i = (end + 1).min(bytes.len());
            } else {
                let start = i;
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>' {
                    i += 1;
                }
                value = unescape(&source[start..i]);
            }
        }
        if !out.iter().any(|(n, _)| *n == name) {
            out.push((name, value));
        }
    }
}

impl Document {
    fn parse(source: &str) -> Document {
        let root = Node {
            tag: String::new(),
            attributes: Vec::new(),
            parent: 0,
            children: Vec::new(),
            start: 0,
            open_end: 0,
            inner_end: source.len(),
            end: source.len(),
        };
        let mut nodes = vec![root];
        let mut stack = vec![0usize];
        let close =
            |nodes: &mut Vec<Node>, stack: &mut Vec<usize>, until: usize, inner_end: usize, end: usize| {
                while stack.len() > until {
                    let node = stack.pop().expect("open");
                    let explicit = stack.len() == until;
                    nodes[node].inner_end = inner_end;
                    nodes[node].end = if explicit { end } else { inner_end };
                }
            };
        let open = |stack: &[usize], nodes: &[Node], name: &str, stop: &[&str]| {
            stack
                .iter()
                .rposition(|n| nodes[*n].tag == name)
                .filter(|&at| !stack[at..].iter().any(|n| stop.contains(&nodes[*n].tag.as_str())))
        };
        tokens(source, |token, _, _| match token {
            Token::Text => {}
            Token::Start { name, attributes, closed, start, end } => {
                let implied: Option<usize> = match name.as_str() {
                    n if CLOSES_P.contains(&n) => open(&stack, &nodes, "p", &["button", "table", "td", "th"]),
                    "li" => open(&stack, &nodes, "li", &["ul", "ol"]),
                    "dt" | "dd" => {
                        open(&stack, &nodes, "dt", &["dl"]).or_else(|| open(&stack, &nodes, "dd", &["dl"]))
                    }
                    "tr" => open(&stack, &nodes, "tr", &["table"]),
                    "td" | "th" => open(&stack, &nodes, "td", &["tr", "table"])
                        .or_else(|| open(&stack, &nodes, "th", &["tr", "table"])),
                    "option" => open(&stack, &nodes, "option", &["select"]),
                    _ => None,
                };
                if let Some(at) = implied {
                    close(&mut nodes, &mut stack, at, start, start);
                }
                let parent = *stack.last().expect("the document");
                let index = nodes.len();
                let void = closed || VOID.contains(&name.as_str());
                nodes.push(Node {
                    tag: name,
                    attributes,
                    parent,
                    children: Vec::new(),
                    start,
                    open_end: end,
                    inner_end: end,
                    end,
                });
                nodes[parent].children.push(index);
                if !void {
                    stack.push(index);
                }
            }
            Token::End { name, start, end } => {
                if let Some(at) = stack.iter().skip(1).rposition(|n| nodes[*n].tag == name) {
                    close(&mut nodes, &mut stack, at + 1, start, end);
                }
            }
        });
        let length = source.len();
        close(&mut nodes, &mut stack, 1, length, length);
        Document { source: source.to_string(), nodes }
    }

    /// The text of a part of the source: without tags, comments, scripts and styles, its
    /// entities decoded and its spaces made one.
    fn text(&self, from: usize, to: usize) -> String {
        let part = &self.source[from..to];
        let mut raw = String::new();
        let mut skip: Option<String> = None;
        tokens(part, |token, start, end| match token {
            Token::Text => {
                if skip.is_none() {
                    raw.push_str(&unescape(&part[start..end]));
                }
            }
            Token::Start { name, closed, .. } => {
                if (name == "script" || name == "style") && !closed {
                    skip = Some(name);
                } else if BLOCK.contains(&name.as_str()) {
                    raw.push(' ');
                }
            }
            Token::End { name, .. } => {
                if skip.as_deref() == Some(name.as_str()) {
                    skip = None;
                } else if BLOCK.contains(&name.as_str()) {
                    raw.push(' ');
                }
            }
        });
        raw.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    fn element(&self, index: usize) -> Element {
        let node = &self.nodes[index];
        Element {
            tag: node.tag.clone(),
            text: self.text(node.open_end, node.inner_end),
            inner: self.source[node.open_end..node.inner_end].to_string(),
            outer: self.source[node.start..node.end].to_string(),
            attributes: node.attributes.clone(),
        }
    }

    fn attribute(&self, index: usize, name: &str) -> Option<&str> {
        self.nodes[index].attributes.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone)]
struct Compound {
    tag: Option<String>,
    tests: Vec<Test>,
}

#[derive(Debug, Clone)]
enum Test {
    Id(String),
    Class(String),
    Has(String),
    Attr { name: String, op: char, value: String },
    FirstChild,
    LastChild,
    NthChild(usize),
    Not(Box<Compound>),
}

/// A selector: compounds from left to right, each with the combinator before it
/// (`' '`, `'>'`, `'+'` or `'~'`).
type Complex = Vec<(char, Compound)>;

fn parse_selector(selector: &str) -> Result<Vec<Complex>, String> {
    let mut groups = Vec::new();
    for group in selector.split(',') {
        let chars: Vec<char> = group.trim().chars().collect();
        if chars.is_empty() {
            return Err(format!("`{selector}` has an empty part between commas"));
        }
        let mut complex = Vec::new();
        let mut i = 0;
        let mut combinator = ' ';
        while i < chars.len() {
            if chars[i].is_whitespace() {
                i += 1;
                continue;
            }
            if matches!(chars[i], '>' | '+' | '~') {
                if complex.is_empty() || combinator != ' ' {
                    return Err(format!("`{}` in `{selector}` has nothing on its left", chars[i]));
                }
                combinator = chars[i];
                i += 1;
                continue;
            }
            let (compound, next) = compound(&chars, i, selector)?;
            complex.push((combinator, compound));
            combinator = ' ';
            i = next;
        }
        if combinator != ' ' {
            return Err(format!("`{combinator}` in `{selector}` has nothing on its right"));
        }
        groups.push(complex);
    }
    Ok(groups)
}

fn compound(chars: &[char], mut i: usize, selector: &str) -> Result<(Compound, usize), String> {
    let word =
        |i: usize| chars[i..].iter().take_while(|c| c.is_alphanumeric() || **c == '-' || **c == '_').count();
    let mut out = Compound { tag: None, tests: Vec::new() };
    let start = i;
    if chars[i] == '*' {
        i += 1;
    } else {
        let n = word(i);
        if n > 0 {
            out.tag = Some(chars[i..i + n].iter().collect::<String>().to_ascii_lowercase());
            i += n;
        }
    }
    while i < chars.len() && !chars[i].is_whitespace() && !matches!(chars[i], '>' | '+' | '~' | ')') {
        let kind = chars[i];
        i += 1;
        match kind {
            '#' | '.' => {
                let n = word(i);
                if n == 0 {
                    return Err(format!("`{kind}` in `{selector}` has no name after it"));
                }
                let name: String = chars[i..i + n].iter().collect();
                out.tests.push(if kind == '#' { Test::Id(name) } else { Test::Class(name) });
                i += n;
            }
            '[' => {
                let close = chars[i..]
                    .iter()
                    .position(|c| *c == ']')
                    .ok_or(format!("`[` in `{selector}` has no `]`"))?;
                let inside: String = chars[i..i + close].iter().collect();
                i += close + 1;
                let test = match inside.find('=') {
                    None => Test::Has(inside.trim().to_ascii_lowercase()),
                    Some(at) => {
                        let (left, value) = (&inside[..at], &inside[at + 1..]);
                        let (name, op) = match left.chars().last() {
                            Some(op @ ('~' | '^' | '$' | '*')) => (&left[..left.len() - 1], op),
                            _ => (left, '='),
                        };
                        let value = value.trim();
                        let value = value
                            .strip_prefix('"')
                            .and_then(|v| v.strip_suffix('"'))
                            .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                            .unwrap_or(value);
                        Test::Attr { name: name.trim().to_ascii_lowercase(), op, value: value.to_string() }
                    }
                };
                out.tests.push(test);
            }
            ':' => {
                let rest: String = chars[i..].iter().collect();
                if let Some(after) = rest.strip_prefix("first-child") {
                    out.tests.push(Test::FirstChild);
                    i = chars.len() - after.chars().count();
                } else if let Some(after) = rest.strip_prefix("last-child") {
                    out.tests.push(Test::LastChild);
                    i = chars.len() - after.chars().count();
                } else if let Some(after) = rest.strip_prefix("not(") {
                    let inner: Vec<char> = after.chars().collect();
                    let start = inner.iter().take_while(|c| c.is_whitespace()).count();
                    if start >= inner.len() {
                        return Err(format!("`:not(` in `{selector}` has no `)`"));
                    }
                    let (negated, next) = compound(&inner, start, selector)?;
                    let close = next + inner[next..].iter().take_while(|c| c.is_whitespace()).count();
                    if inner.get(close) != Some(&')') {
                        return Err(format!(
                            "`:not(…)` in `{selector}` takes one simple selector, such as `.ad`"
                        ));
                    }
                    out.tests.push(Test::Not(Box::new(negated)));
                    i = chars.len() - (inner.len() - close - 1);
                } else if let Some(after) = rest.strip_prefix("nth-child(") {
                    let close = after.find(')').ok_or(format!("`:nth-child(` in `{selector}` has no `)`"))?;
                    let n: usize = after[..close]
                        .trim()
                        .parse()
                        .ok()
                        .filter(|n| *n > 0)
                        .ok_or(format!("`:nth-child` in `{selector}` takes a number from 1"))?;
                    out.tests.push(Test::NthChild(n));
                    i = chars.len() - after[close + 1..].chars().count();
                } else {
                    return Err(format!(
                        "`:{rest}` is not supported: the pseudo-classes are :first-child, :last-child, :nth-child(n) and :not(…)"
                    ));
                }
            }
            c => return Err(format!("`{c}` in `{selector}` is not part of a supported selector")),
        }
    }
    if i == start {
        return Err(format!("`{selector}` is not a selector"));
    }
    Ok((out, i))
}

impl Document {
    fn matches_compound(&self, index: usize, compound: &Compound) -> bool {
        if index == 0 {
            return false;
        }
        let node = &self.nodes[index];
        if compound.tag.as_ref().is_some_and(|tag| *tag != node.tag) {
            return false;
        }
        let siblings = &self.nodes[node.parent].children;
        compound.tests.iter().all(|test| match test {
            Test::Id(id) => self.attribute(index, "id") == Some(id.as_str()),
            Test::Class(class) => self
                .attribute(index, "class")
                .is_some_and(|value| value.split_whitespace().any(|c| c == class)),
            Test::Has(name) => self.attribute(index, name).is_some(),
            Test::Attr { name, op, value: wanted } => {
                self.attribute(index, name).is_some_and(|value| match op {
                    '=' => value == wanted,
                    '~' => value.split_whitespace().any(|word| word == wanted),
                    '^' => !wanted.is_empty() && value.starts_with(wanted.as_str()),
                    '$' => !wanted.is_empty() && value.ends_with(wanted.as_str()),
                    _ => !wanted.is_empty() && value.contains(wanted.as_str()),
                })
            }
            Test::FirstChild => node.parent != 0 && siblings.first() == Some(&index),
            Test::LastChild => node.parent != 0 && siblings.last() == Some(&index),
            Test::NthChild(n) => node.parent != 0 && siblings.get(n - 1) == Some(&index),
            Test::Not(inner) => !self.matches_compound(index, inner),
        })
    }

    /// Whether the element matches the compounds up to `last`, from right to left.
    fn matches(&self, index: usize, complex: &Complex, last: usize) -> bool {
        let (combinator, compound) = &complex[last];
        if !self.matches_compound(index, compound) {
            return false;
        }
        if last == 0 {
            return true;
        }
        let mut parent = self.nodes[index].parent;
        let siblings = &self.nodes[parent].children;
        let position = siblings.iter().position(|n| *n == index).unwrap_or(0);
        match combinator {
            '>' => return self.matches(parent, complex, last - 1),
            '+' => return position > 0 && self.matches(siblings[position - 1], complex, last - 1),
            '~' => return siblings[..position].iter().any(|n| self.matches(*n, complex, last - 1)),
            _ => {}
        }
        while parent != 0 {
            if self.matches(parent, complex, last - 1) {
                return true;
            }
            parent = self.nodes[parent].parent;
        }
        false
    }
}

static LAST: Mutex<Option<Document>> = Mutex::new(None);

/// The elements of `source` that match `selector`, in the order of the text. With
/// `inside`, the source is the HTML of one element, which is not itself a result.
pub fn select(source: &str, selector: &str, inside: bool) -> Result<Vec<Element>, String> {
    let groups = parse_selector(selector)?;
    let mut last = LAST.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    // The same page is often searched several times in a row.
    if last.as_ref().is_none_or(|document| document.source != source) {
        *last = Some(Document::parse(source));
    }
    let document = last.as_ref().expect("parsed above");
    let first = if inside { document.nodes[0].children.first().copied() } else { None };
    Ok((1..document.nodes.len())
        .filter(|index| Some(*index) != first)
        .filter(|index| groups.iter().any(|complex| document.matches(*index, complex, complex.len() - 1)))
        .map(|index| document.element(index))
        .collect())
}

/// The text of an HTML fragment, as `Element.text`.
pub fn text(source: &str) -> String {
    let document = Document { source: source.to_string(), nodes: Vec::new() };
    document.text(0, source.len())
}

/// `&`, `<`, `>`, `"` and `'` written as entities, to put a text in a page.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// The entities of a text decoded: `&amp;`, `&eacute;`, `&#233;`, `&#xE9;`… An unknown
/// entity stays as written.
pub fn unescape(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let end = rest[1..].find(|c: char| !(c.is_ascii_alphanumeric() || c == '#')).map(|p| p + 1);
        let decoded = end.filter(|end| rest[*end..].starts_with(';')).and_then(|end| {
            let name = &rest[1..end];
            let c = if let Some(number) = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
                u32::from_str_radix(number, 16).ok().and_then(char::from_u32)
            } else if let Some(number) = name.strip_prefix('#') {
                number.parse().ok().and_then(char::from_u32)
            } else {
                entity(name)
            };
            c.map(|c| (c, end + 1))
        });
        match decoded {
            Some((c, length)) => {
                out.push(c);
                rest = &rest[length..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn entity(name: &str) -> Option<char> {
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "hellip" => '…',
        "mdash" => '—',
        "ndash" => '–',
        "laquo" => '«',
        "raquo" => '»',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "bull" => '•',
        "middot" => '·',
        "euro" => '€',
        "pound" => '£',
        "yen" => '¥',
        "cent" => '¢',
        "deg" => '°',
        "times" => '×',
        "divide" => '÷',
        "plusmn" => '±',
        "sect" => '§',
        "para" => '¶',
        "eacute" => 'é',
        "egrave" => 'è',
        "ecirc" => 'ê',
        "euml" => 'ë',
        "agrave" => 'à',
        "aacute" => 'á',
        "acirc" => 'â',
        "auml" => 'ä',
        "ccedil" => 'ç',
        "icirc" => 'î',
        "iuml" => 'ï',
        "iacute" => 'í',
        "ocirc" => 'ô',
        "ouml" => 'ö',
        "oacute" => 'ó',
        "ugrave" => 'ù',
        "ucirc" => 'û',
        "uuml" => 'ü',
        "uacute" => 'ú',
        "ntilde" => 'ñ',
        "Eacute" => 'É',
        "Egrave" => 'È',
        "Agrave" => 'À',
        "Ccedil" => 'Ç',
        "oelig" => 'œ',
        "szlig" => 'ß',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"<!doctype html>
<html><head><title>Les &amp; nouvelles</title><style>p { color: red }</style></head>
<body>
  <div id="main" class="news list">
    <h1>Titres</h1>
    <ul>
      <li class="item"><a href="/a?x=1&amp;y=2">Premier</a> <span class=price>12 &euro;</span>
      <li class="item hot"><a href='https://ex.com/b' data-id=7>Deuxi&egrave;me</a>
    </ul>
    <p>Un<br>texte <!-- caché --> à lire
    <p>Encore<script>var x = "<p>non</p>";</script>
  </div>
  <img src="i.png">
</body></html>"#;

    fn tags(selector: &str) -> Vec<String> {
        select(PAGE, selector, false).unwrap().into_iter().map(|e| e.text).collect()
    }

    #[test]
    fn finds_elements_with_selectors() {
        assert_eq!(tags("li a"), ["Premier", "Deuxième"]);
        assert_eq!(tags("li.hot > a[data-id=7]"), ["Deuxième"]);
        assert_eq!(tags("a[href^=https]"), ["Deuxième"]);
        assert_eq!(tags("#main > h1, .price"), ["Titres", "12 €"]);
        assert_eq!(tags("p"), ["Un texte à lire", "Encore"]);
        assert_eq!(tags("ul > li:first-child .price"), ["12 €"]);
        assert_eq!(tags("li:nth-child(2) a"), ["Deuxième"]);
        assert_eq!(tags("a + span"), ["12 €"]);
        assert_eq!(tags("h1 ~ p"), ["Un texte à lire", "Encore"]);
        assert_eq!(tags("h1 + ul > li:not(.hot) > a"), ["Premier"]);
        assert_eq!(tags("title"), ["Les & nouvelles"]);
        assert_eq!(select(PAGE, "img", false).unwrap()[0].outer, r#"<img src="i.png">"#);
        let links = select(PAGE, "a", false).unwrap();
        assert_eq!(links[0].attributes, vec![("href".to_string(), "/a?x=1&y=2".to_string())]);
        assert_eq!(links[0].outer, r#"<a href="/a?x=1&amp;y=2">Premier</a>"#);
    }

    #[test]
    fn searches_inside_an_element() {
        let list = &select(PAGE, "ul", false).unwrap()[0];
        let items = select(&list.outer, "li", true).unwrap();
        assert_eq!(items.len(), 2);
        assert!(items[0].inner.ends_with("</span>\n      "), "{:?}", items[0].inner);
        assert!(select(&items[0].outer, "li", true).unwrap().is_empty());
        assert_eq!(select(&items[1].outer, "a", true).unwrap()[0].text, "Deuxième");
    }

    #[test]
    fn refuses_what_is_not_a_selector() {
        for bad in
            ["", "a,", "> a", "a >", "a:hover", "[x", ".", "a!", "a + + b", ":not(", ":not(a b)", "a ~"]
        {
            assert!(parse_selector(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn escapes_and_unescapes() {
        assert_eq!(escape("<a href=\"x\">&'"), "&lt;a href=&quot;x&quot;&gt;&amp;&#39;");
        assert_eq!(unescape("&lt;&#233;&#xE9;&eacute;&unknown;&amp"), "<ééé&unknown;&amp");
        assert_eq!(text("<p>a</p><p>b &amp; c</p>"), "a b & c");
    }
}
