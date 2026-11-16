//! The computations of the standard modules (spec §23) that the implementation
//! provides, the same in both execution modes (§22.2).

/// SplitMix64: the state of a generator goes forward by a constant; each state gives
/// a well-mixed number. The same seed gives the same numbers on every system.
pub fn random_advance(state: i64) -> i64 {
    state.wrapping_add(0x9E37_79B9_7F4A_7C15_u64 as i64)
}

fn random_mix(state: i64) -> u64 {
    let mut z = state as u64;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A Float from 0.0 included to 1.0 excluded, from 53 random bits.
pub fn random_unit(state: i64) -> f64 {
    (random_mix(state) >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
}

/// An Int from 0 to `count - 1`, for `count >= 1`.
pub fn random_below(state: i64, count: i64) -> Option<i64> {
    if count < 1 {
        return None;
    }
    // Multiplying keeps the high bits, the best mixed ones (Lemire).
    Some(((u128::from(random_mix(state)) * count as u128) >> 64) as i64)
}

/// The lines of a text, without their ends (`\n` or `\r\n`); a final line end does
/// not start an empty line.
pub fn text_lines(text: &str) -> Vec<String> {
    text.lines().map(str::to_string).collect()
}

/// The position, from 1 and in characters, of the first occurrence of `part`.
pub fn text_find(text: &str, part: &str) -> Option<i64> {
    text.find(part).map(|byte| text[..byte].chars().count() as i64 + 1)
}

/// The values of a CSV text, line by line (RFC 4180): values separated by commas,
/// between quotes when they hold a comma, a quote (written twice) or a line end.
/// Empty lines are skipped.
pub fn csv_parse(text: &str) -> Result<Vec<Vec<String>>, String> {
    let mut rows = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut value = String::new();
    let mut chars = text.chars().peekable();
    let mut line = 1;
    let mut quoted = false;
    let mut row_started = false;
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    value.push('"');
                }
                '"' => quoted = false,
                '\n' => {
                    line += 1;
                    value.push('\n');
                }
                c => value.push(c),
            }
            continue;
        }
        match c {
            '"' if value.is_empty() => {
                quoted = true;
                row_started = true;
            }
            '"' => {
                return Err(format!(
                    "line {line}: a quote in the middle of a value must be doubled, inside quotes"
                ));
            }
            ',' => {
                row.push(std::mem::take(&mut value));
                row_started = true;
            }
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' => {
                if row_started || !value.is_empty() {
                    row.push(std::mem::take(&mut value));
                    rows.push(std::mem::take(&mut row));
                }
                row_started = false;
                line += 1;
            }
            c => {
                value.push(c);
                row_started = true;
            }
        }
    }
    if quoted {
        return Err(format!("line {line}: a quoted value is not closed"));
    }
    if row_started || !value.is_empty() {
        row.push(value);
        rows.push(row);
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_numbers_are_reproducible() {
        let state = random_advance(42);
        assert_eq!(random_unit(state), random_unit(random_advance(42)));
        assert!((0.0..1.0).contains(&random_unit(state)));
        assert!(random_below(state, 6).is_some_and(|value| (0..6).contains(&value)));
        assert_eq!(random_below(state, 0), None);
    }

    #[test]
    fn csv_values() {
        let rows = csv_parse("name,grade\nLéa,14\n\"Tom, Jr\",\"8\"\r\n\n\"a \"\"b\"\"\",\n").unwrap();
        assert_eq!(
            rows,
            vec![
                vec!["name".to_string(), "grade".to_string()],
                vec!["Léa".to_string(), "14".to_string()],
                vec!["Tom, Jr".to_string(), "8".to_string()],
                vec!["a \"b\"".to_string(), String::new()],
            ]
        );
        assert!(csv_parse("\"open").is_err());
    }

    #[test]
    fn texts() {
        assert_eq!(text_find("héllo", "l"), Some(3));
        assert_eq!(text_find("abc", "z"), None);
        assert_eq!(text_lines("a\r\nb\n"), vec!["a".to_string(), "b".to_string()]);
    }
}

/// Whether `path` matches the glob `pattern` (C106): `*` any characters but `/`, `?` one,
/// `**` any folders, `[abc]` or `[a-z]` one of them, `{jpg,png}` one of the words. A
/// pattern without `/` looks at the name of the file only.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let target = if pattern.contains('/') { path } else { path.rsplit('/').next().unwrap_or(path) };
    let target: Vec<char> = target.chars().collect();
    expand_braces(pattern).iter().any(|pattern| {
        let pattern: Vec<char> = pattern.chars().collect();
        glob_here(&pattern, &target)
    })
}

/// `a{b,c}d{e,f}` as `abde`, `abdf`, `acde`, `acdf`.
fn expand_braces(pattern: &str) -> Vec<String> {
    let Some(open) = pattern.find('{') else { return vec![pattern.to_string()] };
    let Some(close) = pattern[open..].find('}').map(|p| p + open) else { return vec![pattern.to_string()] };
    let (before, inside, after) = (&pattern[..open], &pattern[open + 1..close], &pattern[close + 1..]);
    inside.split(',').flat_map(|word| expand_braces(&format!("{before}{word}{after}"))).collect()
}

fn glob_here(pattern: &[char], target: &[char]) -> bool {
    match pattern.first() {
        None => target.is_empty(),
        // `**/`: no folder, or folders up to a `/`.
        Some('*') if pattern.get(1) == Some(&'*') && pattern.get(2) == Some(&'/') => (0..=target.len())
            .filter(|skip| *skip == 0 || target[skip - 1] == '/')
            .any(|skip| glob_here(&pattern[3..], &target[skip..])),
        Some('*') if pattern.get(1) == Some(&'*') => {
            (0..=target.len()).any(|skip| glob_here(&pattern[2..], &target[skip..]))
        }
        Some('*') => (0..=target.len())
            .take_while(|skip| *skip == 0 || target[skip - 1] != '/')
            .any(|skip| glob_here(&pattern[1..], &target[skip..])),
        Some('?') => target.first().is_some_and(|c| *c != '/') && glob_here(&pattern[1..], &target[1..]),
        Some('[') => {
            let Some(close) = pattern.iter().skip(1).position(|c| *c == ']').map(|p| p + 1) else {
                return target.first() == Some(&'[') && glob_here(&pattern[1..], &target[1..]);
            };
            let Some(&c) = target.first() else { return false };
            let set = &pattern[1..close];
            let (negated, set) = match set.first() {
                Some('!' | '^') => (true, &set[1..]),
                _ => (false, set),
            };
            let mut found = false;
            let mut i = 0;
            while i < set.len() {
                if i + 2 < set.len() && set[i + 1] == '-' {
                    found |= set[i] <= c && c <= set[i + 2];
                    i += 3;
                } else {
                    found |= set[i] == c;
                    i += 1;
                }
            }
            found != negated && c != '/' && glob_here(&pattern[close + 1..], &target[1..])
        }
        Some(&p) => target.first() == Some(&p) && glob_here(&pattern[1..], &target[1..]),
    }
}

#[cfg(test)]
mod glob_tests {
    use super::glob_match;

    #[test]
    fn globs_match_as_shells_do() {
        assert!(glob_match("*.jpg", "photos/2026/a.jpg"));
        assert!(!glob_match("*.jpg", "a.jpeg"));
        assert!(glob_match("*.{jpg,png}", "x/b.png"));
        assert!(glob_match("photos/*.jpg", "photos/a.jpg"));
        assert!(!glob_match("photos/*.jpg", "photos/2026/a.jpg"));
        assert!(glob_match("photos/**/*.jpg", "photos/2026/11/a.jpg"));
        assert!(glob_match("photos/**/*.jpg", "photos/a.jpg"));
        assert!(glob_match("**/*.txt", "a.txt"));
        assert!(glob_match("img_??.png", "img_01.png"));
        assert!(glob_match("[a-c]*.txt", "b1.txt"));
        assert!(!glob_match("[!a-c]*.txt", "b1.txt"));
        assert!(glob_match("**", "a/b/c"));
    }
}
