//! The modules of the standard library (spec §23), written in Lion. What only the
//! system can do is declared `foreign "lion"` and provided by the implementation.

/// The source of the standard module `name`, as `use name` loads it.
pub fn module(name: &str) -> Option<&'static str> {
    Some(match name {
        "csv" => include_str!("../std/csv.lion"),
        "dates" => include_str!("../std/dates.lion"),
        "files" => include_str!("../std/files.lion"),
        "json" => include_str!("../std/json.lion"),
        "math" => include_str!("../std/math.lion"),
        "random" => include_str!("../std/random.lion"),
        "sets" => include_str!("../std/sets.lion"),
        "text" => include_str!("../std/text.lion"),
        "time" => include_str!("../std/time.lion"),
        _ => return None,
    })
}

/// The names of the standard modules.
pub const MODULES: &[&str] = &["csv", "dates", "files", "json", "math", "random", "sets", "text", "time"];
