//! The compiled mode gives the same results and the same bugs as the interpreted mode
//! (spec §22.2). Each golden program that `lion run` runs (`tests/runtime`,
//! `tests/integration` and `tests/programs`) is compiled to native code, then its exit
//! code and output are compared with the same `.expected` file, without the alerts,
//! which only the interpreted mode reports (§22.3).
//!
//! The programs are compiled together, as the modules of one executable that runs the
//! program it is given: one compilation instead of one per program. A second test runs
//! `lion build` itself. Both need cargo, as `lion build` does.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// The suites whose programs `lion run` runs, and whether a program runs from its folder.
const SUITES: &[(&str, bool)] = &[("runtime", false), ("integration", false), ("programs", true)];

#[test]
fn compiled_programs_behave_as_interpreted() {
    let root = root();
    let suite = Path::new(env!("CARGO_TARGET_TMPDIR")).join("native-suite");
    fs::create_dir_all(suite.join("src")).unwrap();
    // Each program: its folder, its path from there, and what `lion` reported about it.
    let mut programs: Vec<(PathBuf, PathBuf, String)> = Vec::new();
    let mut main = String::new();
    let mut dispatch = String::new();
    for (name, from_folder) in SUITES {
        for file in lion_files(&root.join("tests").join(name)) {
            let (folder, path) = if *from_folder {
                (file.parent().unwrap().to_path_buf(), PathBuf::from(file.file_name().unwrap()))
            } else {
                (root.clone(), file.strip_prefix(&root).unwrap().to_path_buf())
            };
            let output = Command::new(env!("CARGO_BIN_EXE_lion"))
                .args(["debug", "rust"])
                .arg(&path)
                .current_dir(&folder)
                .output()
                .expect("the lion binary runs");
            assert!(output.status.success(), "{} is refused:\n{}", path.display(), text(&output.stderr));
            let number = programs.len();
            write_if_changed(&suite.join("src").join(format!("p{number}.rs")), &text(&output.stdout));
            main.push_str(&format!("#[path = \"p{number}.rs\"]\nmod p{number};\n"));
            dispatch.push_str(&format!("        Some(\"{number}\") => p{number}::main(),\n"));
            programs.push((folder, path, text(&output.stderr)));
        }
    }
    main.push_str(&format!(
        "\nfn main() -> std::process::ExitCode {{\n    match std::env::args().nth(1).as_deref() {{\n{dispatch}        \
         _ => std::process::ExitCode::from(64),\n    }}\n}}\n"
    ));
    write_if_changed(&suite.join("src").join("main.rs"), &main);
    let runtime = root.join("crates").join("lion_native");
    write_if_changed(
        &suite.join("Cargo.toml"),
        &format!(
            "[package]\nname = \"native-suite\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [dependencies]\nlion_native = {{ path = {:?} }}\n\n[workspace]\n\n[profile.release]\ndebug = false\n",
            runtime.display().to_string()
        ),
    );
    let built = Command::new(cargo())
        .args(["build", "--release", "--offline", "--quiet"])
        .current_dir(&suite)
        .env("CARGO_TARGET_DIR", suite.join("target"))
        .output()
        .expect("cargo runs");
    assert!(
        built.status.success(),
        "the Rust code of the programs does not compile:\n{}",
        text(&built.stderr)
    );
    let executable = suite.join("target").join("release").join("native-suite");

    let mut failures = Vec::new();
    for (number, (folder, path, reported)) in programs.iter().enumerate() {
        let mut command = Command::new(&executable);
        command.arg(number.to_string()).current_dir(folder).stdin(Stdio::null());
        // The interface runs without a screen, with the events written next to the program.
        command.env("LION_UI", "headless").env_remove("LION_UI_EVENTS").env_remove("LION_UI_SNAPSHOT");
        let events = folder.join(path).with_extension("events");
        if events.exists() {
            command.env("LION_UI_EVENTS", &events);
        }
        let output = command.output().expect("the program runs");
        // `lion run` reports the warnings of the program before it runs it.
        let actual = transcript(&output, reported);
        let expected_path = folder.join(path).with_extension("expected");
        let expected = without_alerts(&fs::read_to_string(&expected_path).unwrap());
        if actual != expected {
            failures.push(format!("{}\n----- expected\n{expected}----- actual\n{actual}", path.display()));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} compiled programs differ from the interpreted mode:\n\n{}",
        failures.len(),
        programs.len(),
        failures.join("\n")
    );
}

#[test]
fn lion_build_makes_an_executable() {
    let root = root();
    let folder = Path::new(env!("CARGO_TARGET_TMPDIR")).join("lion-build");
    fs::create_dir_all(&folder).unwrap();
    let executable = folder.join("gcd");
    let _ = fs::remove_file(&executable);
    let output = Command::new(env!("CARGO_BIN_EXE_lion"))
        .args(["build", "tests/integration/gcd.lion", "-o"])
        .arg(&executable)
        .current_dir(&root)
        .env("LION_CACHE", folder.join("cache"))
        .env("LION_CARGO", cargo())
        .output()
        .expect("the lion binary runs");
    assert!(output.status.success(), "lion build failed:\n{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        format!("built `{}` from `tests/integration/gcd.lion`\n", executable.display())
    );
    let run = Command::new(&executable).current_dir(&root).stdin(Stdio::null()).output().expect("it runs");
    let expected = fs::read_to_string(root.join("tests/integration/gcd.expected")).unwrap();
    assert_eq!(transcript(&run, ""), without_alerts(&expected));
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

/// The cargo that runs the tests.
fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string())
}

/// The programs of a folder: its `.lion` files, and each `name/name.lion` of its
/// folders, which holds a program with its modules.
fn lion_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter_map(|path| {
            if path.is_dir() {
                let name = path.file_name()?.to_owned();
                let program = path.join(name).with_extension("lion");
                program.exists().then_some(program)
            } else {
                path.extension().is_some_and(|ext| ext == "lion").then_some(path)
            }
        })
        .collect();
    files.sort();
    files
}

/// Writes the file unless it has this content already, so that cargo does not compile
/// it again.
fn write_if_changed(path: &Path, content: &str) {
    if fs::read_to_string(path).ok().as_deref() != Some(content) {
        fs::write(path, content).unwrap();
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn transcript(output: &Output, reported: &str) -> String {
    let code = output.status.code().map_or("none".to_string(), |code| code.to_string());
    format!(
        "exit: {code}\n----- stdout\n{}----- stderr\n{reported}{}",
        text(&output.stdout),
        text(&output.stderr)
    )
}

/// The transcript without its alerts: each goes from its `alert:` line to the empty
/// line that ends it.
fn without_alerts(transcript: &str) -> String {
    let mut kept = String::new();
    let mut in_alert = false;
    for line in transcript.split_inclusive('\n') {
        if line.starts_with("alert:") {
            in_alert = true;
        }
        if in_alert {
            in_alert = line != "\n";
            continue;
        }
        kept.push_str(line);
    }
    kept
}
