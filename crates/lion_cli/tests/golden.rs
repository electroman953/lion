//! Golden tests: each `tests/<suite>/<name>.lion` file of the repository is given to
//! the real `lion` binary, and the exit code, standard output and standard error are
//! compared with `<name>.expected`.
//!
//! After an intended change of output, regenerate the expectations with
//! `LION_BLESS=1 cargo test --test golden`, then review the diff before committing.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Each suite runs one command of `lion`.
const SUITES: &[(&str, &[&str])] = &[
    ("lexer", &["debug", "tokens"]),
    ("parser", &["debug", "ast"]),
    ("typechecker", &["debug", "ir"]),
    ("runtime", &["run"]),
    ("errors", &["check"]),
    ("integration", &["run"]),
    // The programs of the spec (§27), run from their folder, where they find their files.
    ("programs", &["run"]),
];

#[test]
fn golden() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    let bless = std::env::var_os("LION_BLESS").is_some();
    let mut failures = Vec::new();
    let mut count = 0;
    for (suite, args) in SUITES {
        for file in lion_files(&root.join("tests").join(suite)) {
            count += 1;
            let relative = file.strip_prefix(&root).unwrap();
            let (folder, path) = if *suite == "programs" {
                (file.parent().unwrap().to_path_buf(), Path::new(file.file_name().unwrap()).to_path_buf())
            } else {
                (root.clone(), relative.to_path_buf())
            };
            let output = Command::new(env!("CARGO_BIN_EXE_lion"))
                .args(*args)
                .arg(&path)
                .current_dir(&folder)
                .output()
                .expect("the lion binary runs");
            let actual = transcript(&output);
            let expected_path = file.with_extension("expected");
            if bless {
                fs::write(&expected_path, &actual).unwrap();
                continue;
            }
            match fs::read_to_string(&expected_path) {
                Ok(expected) if expected == actual => {}
                Ok(expected) => failures.push(format!(
                    "{}\n----- expected\n{expected}----- actual\n{actual}",
                    relative.display()
                )),
                Err(_) => failures.push(format!(
                    "{}: no {} (create it with LION_BLESS=1)",
                    relative.display(),
                    expected_path.file_name().unwrap().to_string_lossy()
                )),
            }
        }
    }
    assert!(count > 0, "no golden test found");
    assert!(
        failures.is_empty(),
        "{} of {count} golden tests failed:\n\n{}",
        failures.len(),
        failures.join("\n")
    );
}

fn lion_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .map(|entries| entries.map(|entry| entry.unwrap().path()).collect())
        .unwrap_or_default();
    files.retain(|path| path.extension().is_some_and(|ext| ext == "lion"));
    files.sort();
    files
}

fn transcript(output: &Output) -> String {
    let code = output.status.code().map_or("none".to_string(), |code| code.to_string());
    format!(
        "exit: {code}\n----- stdout\n{}----- stderr\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
