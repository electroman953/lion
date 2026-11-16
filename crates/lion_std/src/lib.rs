//! The modules of the standard library (spec §23), written in Lion. What only the
//! system can do is declared `foreign "lion"` and provided by the implementation.

/// The source of the standard module `name`, as `use name` loads it.
pub fn module(name: &str) -> Option<&'static str> {
    Some(match name {
        "csv" => include_str!("../std/csv.lion"),
        "url" => include_str!("../std/url.lion"),
        "regex" => include_str!("../std/regex.lion"),
        "html" => include_str!("../std/html.lion"),
        "hash" => include_str!("../std/hash.lion"),
        "base64" => include_str!("../std/base64.lion"),
        "dates" => include_str!("../std/dates.lion"),
        "files" => include_str!("../std/files.lion"),
        "json" => include_str!("../std/json.lion"),
        "math" => include_str!("../std/math.lion"),
        "net" => include_str!("../std/net.lion"),
        "random" => include_str!("../std/random.lion"),
        "sets" => include_str!("../std/sets.lion"),
        "system" => include_str!("../std/system.lion"),
        "term" => include_str!("../std/term.lion"),
        "text" => include_str!("../std/text.lion"),
        "time" => include_str!("../std/time.lion"),
        "ui" => include_str!("../std/ui.lion"),
        _ => return None,
    })
}

/// The names of the standard modules.
pub const MODULES: &[&str] = &[
    "base64", "csv", "dates", "files", "hash", "html", "json", "math", "net", "random", "regex", "sets",
    "system", "term", "text", "time", "ui", "url",
];
