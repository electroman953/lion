//! Regular expressions (module `regex`, C105), without a dependency: the pattern is
//! compiled to a program run by a Pike machine, whose time grows with the length of the
//! text times the size of the pattern, never more: no pattern makes it run for ever.
//!
//! The syntax is the common one: `.`, `[a-z]`, `[^…]`, `\d \w \s \D \W \S`, `\b \B`,
//! `^ $`, `( )`, `(?: )`, `(?<name> )`, `|`, `* + ? {n} {n,} {n,m}` and their lazy
//! forms with `?`, and the flags `(?i)` (case), `(?m)` (`^ $` at each line) and `(?s)`
//! (`.` takes line ends too) at the start of the pattern.

/// A compiled pattern.
#[derive(Debug)]
pub struct Regex {
    program: Vec<Inst>,
    classes: Vec<Class>,
    /// The number of groups, the whole match included.
    groups: usize,
    names: Vec<(String, usize)>,
    insensitive: bool,
    multiline: bool,
}

/// A match: the byte ranges of the whole match and of each group.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub groups: Vec<Option<(usize, usize)>>,
}

#[derive(Debug, Clone)]
enum Inst {
    Char(char),
    Any { line_ends: bool },
    Class(usize),
    Split(usize, usize),
    Jump(usize),
    Save(usize),
    Assert(Assertion),
    Match,
}

#[derive(Debug, Clone, Copy)]
enum Assertion {
    Start,
    End,
    Word,
    NotWord,
}

#[derive(Debug, Clone)]
struct Class {
    ranges: Vec<(char, char)>,
    negated: bool,
    /// Also every letter and digit of Unicode: `\w` takes `é`.
    letters: bool,
}

#[derive(Debug, Clone)]
enum Node {
    Empty,
    Char(char),
    Any,
    Class(Class),
    Assert(Assertion),
    Group(Box<Node>, Option<usize>),
    Concat(Vec<Node>),
    Alt(Vec<Node>),
    Repeat { node: Box<Node>, min: u32, max: Option<u32>, greedy: bool },
}

/// The largest count of a repetition `{n,m}`.
const MAX_REPEAT: u32 = 1000;
/// The largest program, so that a pattern cannot fill the memory.
const MAX_PROGRAM: usize = 100_000;

impl Regex {
    /// The pattern compiled, or why it is not one.
    pub fn new(pattern: &str) -> Result<Regex, String> {
        let mut parser = Parser { chars: pattern.chars().collect(), pos: 0, groups: 1, names: Vec::new() };
        let (mut insensitive, mut multiline, mut dot_all) = (false, false, false);
        while parser.chars[parser.pos..].starts_with(&['(', '?'])
            && parser.chars.get(parser.pos + 2).is_some_and(|c| "ims".contains(*c))
        {
            let mut i = parser.pos + 2;
            while let Some(&flag) = parser.chars.get(i) {
                match flag {
                    'i' => insensitive = true,
                    'm' => multiline = true,
                    's' => dot_all = true,
                    ')' => break,
                    _ => return Err(format!("`{flag}` is not a flag: the flags are i, m and s")),
                }
                i += 1;
            }
            if parser.chars.get(i) != Some(&')') {
                return Err("the flags `(?…` have no `)`".to_string());
            }
            parser.pos = i + 1;
        }
        let node = parser.alternation()?;
        if parser.pos < parser.chars.len() {
            return Err(format!("the `)` at position {} closes no group", parser.pos + 1));
        }
        let mut compiler = Compiler { program: Vec::new(), classes: Vec::new(), dot_all };
        compiler.program.push(Inst::Save(0));
        compiler.node(&node)?;
        compiler.program.push(Inst::Save(1));
        compiler.program.push(Inst::Match);
        Ok(Regex {
            program: compiler.program,
            classes: compiler.classes,
            groups: parser.groups,
            names: parser.names,
            insensitive,
            multiline,
        })
    }

    /// The number of groups, the whole match included.
    pub fn group_count(&self) -> usize {
        self.groups
    }

    /// The number of the group called `name`.
    pub fn group_named(&self, name: &str) -> Option<usize> {
        self.names.iter().find(|(n, _)| n == name).map(|(_, i)| *i)
    }

    /// The first match that starts at byte `from` or after.
    pub fn find_at(&self, text: &str, from: usize) -> Option<Found> {
        let slots = self.groups * 2;
        let mut current = Threads::new(self.program.len(), slots);
        let mut next = Threads::new(self.program.len(), slots);
        let mut found: Option<Vec<Option<usize>>> = None;
        let mut position = from;
        loop {
            if found.is_none() {
                let mut caps = vec![None; slots];
                self.add(&mut current, 0, position, text, &mut caps);
            }
            let c = text[position..].chars().next();
            if current.list.is_empty() && (found.is_some() || c.is_none()) {
                break;
            }
            for index in 0..current.list.len() {
                let pc = current.list[index];
                match &self.program[pc] {
                    Inst::Match => {
                        found = Some(current.caps[pc].clone());
                        // The threads after this one have a lower priority.
                        break;
                    }
                    inst => {
                        let Some(c) = c else { continue };
                        let accepted = match inst {
                            Inst::Char(expected) => self.same(*expected, c),
                            Inst::Any { line_ends } => *line_ends || c != '\n',
                            Inst::Class(class) => self.in_class(&self.classes[*class], c),
                            _ => false,
                        };
                        if accepted {
                            let mut caps = current.caps[pc].clone();
                            self.add(&mut next, pc + 1, position + c.len_utf8(), text, &mut caps);
                        }
                    }
                }
            }
            let Some(c) = c else { break };
            position += c.len_utf8();
            std::mem::swap(&mut current, &mut next);
            next.clear();
            if found.is_some() && current.list.is_empty() {
                break;
            }
        }
        found.map(|caps| Found { groups: caps.chunks(2).map(|pair| pair[0].zip(pair[1])).collect() })
    }

    /// Every match, from left to right, without overlaps.
    pub fn find_all(&self, text: &str) -> Vec<Found> {
        let mut all = Vec::new();
        let mut from = 0;
        while from <= text.len() {
            let Some(found) = self.find_at(text, from) else { break };
            let (start, end) = found.groups[0].expect("a match has a range");
            from = if end > start {
                end
            } else {
                // An empty match: the next one starts after the next character.
                end + text[end..].chars().next().map_or(1, char::len_utf8)
            };
            all.push(found);
        }
        all
    }

    /// Follows the instructions that consume nothing from `pc`, and adds the threads
    /// that wait for a character.
    fn add(
        &self,
        threads: &mut Threads,
        pc: usize,
        position: usize,
        text: &str,
        caps: &mut Vec<Option<usize>>,
    ) {
        if threads.seen[pc] {
            return;
        }
        threads.seen[pc] = true;
        match &self.program[pc] {
            Inst::Jump(to) => self.add(threads, *to, position, text, caps),
            Inst::Split(first, second) => {
                self.add(threads, *first, position, text, caps);
                self.add(threads, *second, position, text, caps);
            }
            Inst::Save(slot) => {
                let old = caps[*slot];
                caps[*slot] = Some(position);
                self.add(threads, pc + 1, position, text, caps);
                caps[*slot] = old;
            }
            Inst::Assert(assertion) => {
                if self.holds(*assertion, text, position) {
                    self.add(threads, pc + 1, position, text, caps);
                }
            }
            _ => {
                threads.list.push(pc);
                threads.caps[pc].clone_from(caps);
            }
        }
    }

    fn holds(&self, assertion: Assertion, text: &str, position: usize) -> bool {
        let before = text[..position].chars().next_back();
        let after = text[position..].chars().next();
        match assertion {
            Assertion::Start => position == 0 || (self.multiline && before == Some('\n')),
            Assertion::End => position == text.len() || (self.multiline && after == Some('\n')),
            Assertion::Word | Assertion::NotWord => {
                let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
                let boundary = word(before) != word(after);
                boundary == matches!(assertion, Assertion::Word)
            }
        }
    }

    fn same(&self, expected: char, c: char) -> bool {
        expected == c || (self.insensitive && lower(expected) == lower(c))
    }

    fn in_class(&self, class: &Class, c: char) -> bool {
        let inside = |c: char| {
            (class.letters && c.is_alphanumeric())
                || class.ranges.iter().any(|(low, high)| *low <= c && c <= *high)
        };
        let found = inside(c) || (self.insensitive && (inside(lower(c)) || inside(upper(c))));
        found != class.negated
    }
}

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn upper(c: char) -> char {
    c.to_uppercase().next().unwrap_or(c)
}

struct Threads {
    list: Vec<usize>,
    seen: Vec<bool>,
    caps: Vec<Vec<Option<usize>>>,
}

impl Threads {
    fn new(size: usize, slots: usize) -> Threads {
        Threads { list: Vec::new(), seen: vec![false; size], caps: vec![vec![None; slots]; size] }
    }

    fn clear(&mut self) {
        for pc in self.list.drain(..) {
            self.seen[pc] = false;
        }
        // The instructions that consume nothing were marked too.
        self.seen.iter_mut().for_each(|seen| *seen = false);
    }
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
    groups: usize,
    names: Vec<(String, usize)>,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn alternation(&mut self) -> Result<Node, String> {
        let mut options = vec![self.concat()?];
        while self.peek() == Some('|') {
            self.pos += 1;
            options.push(self.concat()?);
        }
        Ok(if options.len() == 1 { options.pop().expect("one") } else { Node::Alt(options) })
    }

    fn concat(&mut self) -> Result<Node, String> {
        let mut items = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            let atom = self.atom()?;
            items.push(self.repeat(atom)?);
        }
        Ok(match items.len() {
            0 => Node::Empty,
            1 => items.pop().expect("one"),
            _ => Node::Concat(items),
        })
    }

    fn repeat(&mut self, atom: Node) -> Result<Node, String> {
        let (min, max) = match self.peek() {
            Some('*') => {
                self.pos += 1;
                (0, None)
            }
            Some('+') => {
                self.pos += 1;
                (1, None)
            }
            Some('?') => {
                self.pos += 1;
                (0, Some(1))
            }
            // `counts` passes `{…}` itself.
            Some('{') => match self.counts() {
                Some(counts) => counts,
                None => return Ok(atom),
            },
            _ => return Ok(atom),
        };
        if matches!(atom, Node::Assert(_) | Node::Empty) {
            return Err(format!("nothing to repeat before position {}", self.pos));
        }
        if min > MAX_REPEAT || max.is_some_and(|max| max > MAX_REPEAT) {
            return Err(format!("a repetition goes up to {MAX_REPEAT}"));
        }
        if max.is_some_and(|max| max < min) {
            return Err(format!(
                "the repetition {{{min},{}}} has its numbers in the wrong order",
                max.unwrap_or(0)
            ));
        }
        let greedy = if self.peek() == Some('?') {
            self.pos += 1;
            false
        } else {
            true
        };
        if matches!(self.peek(), Some('*' | '+' | '?')) {
            return Err(format!("two repetitions follow each other at position {}", self.pos + 1));
        }
        Ok(Node::Repeat { node: Box::new(atom), min, max, greedy })
    }

    /// `{n}`, `{n,}` or `{n,m}`, read and passed; otherwise the `{` is a character.
    fn counts(&mut self) -> Option<(u32, Option<u32>)> {
        let rest: String = self.chars[self.pos + 1..].iter().take_while(|c| **c != '}').collect();
        if self.pos + 1 + rest.len() >= self.chars.len() {
            return None;
        }
        let number = |t: &str| {
            if t.is_empty() || !t.bytes().all(|b| b.is_ascii_digit()) { None } else { t.parse().ok() }
        };
        let counts = match rest.split_once(',') {
            None => {
                let n = number(&rest)?;
                (n, Some(n))
            }
            Some((low, "")) => (number(low)?, None),
            Some((low, high)) => (number(low)?, Some(number(high)?)),
        };
        self.pos += rest.len() + 2;
        Some(counts)
    }

    fn atom(&mut self) -> Result<Node, String> {
        let c = self.peek().expect("a character");
        self.pos += 1;
        Ok(match c {
            '.' => Node::Any,
            '^' => Node::Assert(Assertion::Start),
            '$' => Node::Assert(Assertion::End),
            '[' => Node::Class(self.class()?),
            '(' => {
                let number = if self.chars[self.pos..].starts_with(&['?', ':']) {
                    self.pos += 2;
                    None
                } else if self.chars[self.pos..].starts_with(&['?', '<'])
                    || self.chars[self.pos..].starts_with(&['?', 'P', '<'])
                {
                    self.pos += if self.chars[self.pos + 1] == 'P' { 3 } else { 2 };
                    let name: String = self.chars[self.pos..].iter().take_while(|c| **c != '>').collect();
                    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                        return Err("a group name is a word, as in `(?<year>\\d+)`".to_string());
                    }
                    self.pos += name.chars().count() + 1;
                    self.names.push((name, self.groups));
                    self.groups += 1;
                    Some(self.groups - 1)
                } else if self.peek() == Some('?') {
                    return Err(format!(
                        "`(?` at position {} is not supported: use `(?:`, `(?<name>` or flags at the start",
                        self.pos
                    ));
                } else {
                    self.groups += 1;
                    Some(self.groups - 1)
                };
                let inner = self.alternation()?;
                if self.peek() != Some(')') {
                    return Err("a group `(` has no `)`".to_string());
                }
                self.pos += 1;
                Node::Group(Box::new(inner), number)
            }
            '*' | '+' | '?' => {
                return Err(format!("nothing to repeat before `{c}` at position {}", self.pos));
            }
            '\\' => self.escape(false)?,
            c => Node::Char(c),
        })
    }

    /// After `\`: a character, a class, or an assertion outside a class.
    fn escape(&mut self, in_class: bool) -> Result<Node, String> {
        let Some(c) = self.peek() else { return Err("the pattern ends with `\\`".to_string()) };
        self.pos += 1;
        let digits = vec![('0', '9')];
        let spaces = vec![(' ', ' '), ('\t', '\r'), ('\u{a0}', '\u{a0}')];
        let class = |ranges, negated| Node::Class(Class { ranges, negated, letters: false });
        let letters = |negated| Node::Class(Class { ranges: vec![('_', '_')], negated, letters: true });
        Ok(match c {
            'd' => class(digits, false),
            'D' => class(digits, true),
            'w' => letters(false),
            'W' => letters(true),
            's' => class(spaces, false),
            'S' => class(spaces, true),
            'b' if !in_class => Node::Assert(Assertion::Word),
            'B' if !in_class => Node::Assert(Assertion::NotWord),
            'n' => Node::Char('\n'),
            't' => Node::Char('\t'),
            'r' => Node::Char('\r'),
            '1'..='9' => return Err("back references such as `\\1` are not supported".to_string()),
            c if c.is_alphanumeric() => return Err(format!("`\\{c}` is not an escape")),
            c => Node::Char(c),
        })
    }

    fn class(&mut self) -> Result<Class, String> {
        let negated = self.peek() == Some('^');
        if negated {
            self.pos += 1;
        }
        let mut ranges = Vec::new();
        let mut negated_parts = Vec::new();
        let mut letters = false;
        let mut first = true;
        loop {
            let Some(c) = self.peek() else { return Err("a class `[` has no `]`".to_string()) };
            self.pos += 1;
            if c == ']' && !first {
                break;
            }
            first = false;
            let low = if c == '\\' {
                match self.escape(true)? {
                    Node::Char(c) => c,
                    Node::Class(class) if class.letters && class.negated => {
                        return Err("`\\W` is not supported inside `[…]`".to_string());
                    }
                    Node::Class(class) if class.negated => {
                        negated_parts.push(class);
                        continue;
                    }
                    Node::Class(class) => {
                        letters |= class.letters;
                        ranges.extend(class.ranges);
                        continue;
                    }
                    _ => unreachable!("an escape in a class"),
                }
            } else {
                c
            };
            if self.peek() == Some('-') && self.chars.get(self.pos + 1).is_some_and(|c| *c != ']') {
                self.pos += 1;
                let mut high = self.peek().expect("checked");
                self.pos += 1;
                if high == '\\' {
                    match self.escape(true)? {
                        Node::Char(c) => high = c,
                        _ => return Err("a range ends with a class".to_string()),
                    }
                }
                if high < low {
                    return Err(format!("the range `{low}-{high}` goes backwards"));
                }
                ranges.push((low, high));
            } else {
                ranges.push((low, low));
            }
        }
        if !negated_parts.is_empty() {
            // `[\W\d]`: what is outside the negated classes, plus the rest.
            let mut complement = Vec::new();
            for class in negated_parts {
                complement.extend(invert(&class.ranges));
            }
            ranges.extend(complement);
        }
        Ok(Class { ranges, negated, letters })
    }
}

/// The characters outside the ranges.
fn invert(ranges: &[(char, char)]) -> Vec<(char, char)> {
    let mut sorted = ranges.to_vec();
    sorted.sort();
    let mut out = Vec::new();
    let mut next = 0u32;
    for (low, high) in sorted {
        if (low as u32) > next
            && let (Some(a), Some(b)) = (char::from_u32(next), char::from_u32(low as u32 - 1))
        {
            out.push((a, b));
        }
        next = next.max(high as u32 + 1);
    }
    if let Some(a) = char::from_u32(next) {
        out.push((a, char::MAX));
    }
    out
}

struct Compiler {
    program: Vec<Inst>,
    classes: Vec<Class>,
    dot_all: bool,
}

impl Compiler {
    fn node(&mut self, node: &Node) -> Result<(), String> {
        if self.program.len() > MAX_PROGRAM {
            return Err("the pattern is too large".to_string());
        }
        match node {
            Node::Empty => {}
            Node::Char(c) => self.program.push(Inst::Char(*c)),
            Node::Any => self.program.push(Inst::Any { line_ends: self.dot_all }),
            Node::Class(class) => {
                self.classes.push(class.clone());
                self.program.push(Inst::Class(self.classes.len() - 1));
            }
            Node::Assert(assertion) => self.program.push(Inst::Assert(*assertion)),
            Node::Group(inner, number) => {
                if let Some(number) = number {
                    self.program.push(Inst::Save(number * 2));
                }
                self.node(inner)?;
                if let Some(number) = number {
                    self.program.push(Inst::Save(number * 2 + 1));
                }
            }
            Node::Concat(items) => {
                for item in items {
                    self.node(item)?;
                }
            }
            Node::Alt(options) => {
                let mut jumps = Vec::new();
                for (i, option) in options.iter().enumerate() {
                    if i + 1 < options.len() {
                        let split = self.program.len();
                        self.program.push(Inst::Split(split + 1, 0));
                        self.node(option)?;
                        jumps.push(self.program.len());
                        self.program.push(Inst::Jump(0));
                        let after = self.program.len();
                        self.program[split] = Inst::Split(split + 1, after);
                    } else {
                        self.node(option)?;
                    }
                }
                let end = self.program.len();
                for jump in jumps {
                    self.program[jump] = Inst::Jump(end);
                }
            }
            Node::Repeat { node, min, max, greedy } => {
                for _ in 0..*min {
                    self.node(node)?;
                }
                match max {
                    None => {
                        // L: split body, out; body; jump L
                        let split = self.program.len();
                        self.program.push(Inst::Split(0, 0));
                        self.node(node)?;
                        self.program.push(Inst::Jump(split));
                        let out = self.program.len();
                        self.program[split] = self.split(split + 1, out, *greedy);
                    }
                    Some(max) => {
                        let mut splits = Vec::new();
                        for _ in *min..*max {
                            splits.push(self.program.len());
                            self.program.push(Inst::Split(0, 0));
                            self.node(node)?;
                        }
                        let out = self.program.len();
                        for split in splits {
                            self.program[split] = self.split(split + 1, out, *greedy);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn split(&self, body: usize, out: usize, greedy: bool) -> Inst {
        if greedy { Inst::Split(body, out) } else { Inst::Split(out, body) }
    }
}

/// `with`, where `$1` is the text of the group 1, `$0` the whole match, `${name}` a
/// named group, and `$$` a `$`.
pub fn expand(regex: &Regex, text: &str, found: &Found, with: &str) -> String {
    let group = |i: usize| found.groups.get(i).copied().flatten().map_or("", |(a, b)| &text[a..b]);
    let mut out = String::new();
    let mut chars = with.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('$') => {
                chars.next();
                out.push('$');
            }
            Some(d) if d.is_ascii_digit() => {
                let d = chars.next().expect("peeked").to_digit(10).expect("a digit") as usize;
                out.push_str(group(d));
            }
            Some('{') => {
                chars.next();
                let name: String = chars.by_ref().take_while(|c| *c != '}').collect();
                let number = name.parse::<usize>().ok().or_else(|| regex.group_named(&name));
                out.push_str(number.map_or("", group));
            }
            _ => out.push('$'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(pattern: &str, text: &str) -> Vec<String> {
        let regex = Regex::new(pattern).unwrap();
        regex
            .find_all(text)
            .iter()
            .map(|found| {
                let (a, b) = found.groups[0].unwrap();
                text[a..b].to_string()
            })
            .collect()
    }

    #[test]
    fn finds_what_common_patterns_describe() {
        assert_eq!(all(r"\d+", "a1 b22 c333"), ["1", "22", "333"]);
        assert_eq!(
            all(r"[a-z]+@[a-z]+\.(com|fr)", "x léa@site.fr, max@web.com!"),
            ["a@site.fr", "max@web.com"]
        );
        assert_eq!(all(r"(?i)lion", "Lion LION lion"), ["Lion", "LION", "lion"]);
        assert_eq!(all(r"\bcat\b", "cat concat cat."), ["cat", "cat"]);
        assert_eq!(all(r"<.+?>", "<a><b>"), ["<a>", "<b>"]);
        assert_eq!(all(r"<.+>", "<a><b>"), ["<a><b>"]);
        assert_eq!(all(r"a{2,3}", "aaaaaaa"), ["aaa", "aaa"]);
        assert_eq!(all(r"(?m)^\w+", "un deux\ntrois"), ["un", "trois"]);
        assert_eq!(all(r"x*", "ab"), ["", "", ""]);
        assert_eq!(all(r"[^\d\s]+", "ab 12 cd"), ["ab", "cd"]);
        assert_eq!(all(r"é+", "ééa"), ["éé"]);
        assert_eq!(all(r"\w+", "Léa, Zoë_2!"), ["Léa", "Zoë_2"]);
        assert_eq!(all(r"[\w-]+", "porte-clé x"), ["porte-clé", "x"]);
        assert_eq!(all(r"\W+", "a, b"), [", "]);
        assert_eq!(all(r"a|ab", "ab"), ["a"]);
        assert_eq!(all(r"\$\d+\.\d{2}", "cost $12.50 or $3.5"), ["$12.50"]);
    }

    #[test]
    fn captures_groups() {
        let regex = Regex::new(r"(?<year>\d{4})-(\d\d)(-(\d\d))?").unwrap();
        let text = "on 2026-11 and 2025-01-31";
        let found = regex.find_all(text);
        assert_eq!(found[0].groups[1], Some((3, 7)));
        assert_eq!(found[0].groups[3], None);
        assert_eq!(expand(&regex, text, &found[1], "${year}/$2/$4 $$"), "2025/01/31 $");
        assert_eq!(regex.group_count(), 5);
    }

    #[test]
    fn a_pattern_that_explodes_elsewhere_runs_fast() {
        let text = "a".repeat(5000);
        assert!(Regex::new("(a*)*b").unwrap().find_at(&text, 0).is_none());
    }

    #[test]
    fn refuses_what_is_not_a_pattern() {
        for bad in ["(a", "a)", "[a", "*a", r"\1", "a{3,1}", "a**", r"\q", "(?x)", "[z-a]"] {
            assert!(Regex::new(bad).is_err(), "{bad}");
        }
    }
}
