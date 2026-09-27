//! Embeds in `lion` the sources of the crates that a program compiled by `lion build`
//! needs at run time, so that the command works wherever `lion` is installed (§22.1).

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

/// The crates of the runtime of compiled programs, each after the ones it uses.
const CRATES: &[&str] = &["lion_diagnostics", "lion_runtime", "lion_ir", "lion_vm", "lion_native"];

fn main() {
    let crates = Path::new(&std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo")).join("..");
    let mut files = Vec::new();
    for name in CRATES {
        let source = crates.join(name).join("src");
        println!("cargo:rerun-if-changed={}", source.display());
        collect(&source, &mut files);
    }
    files.sort();
    let mut code = String::from("/// The sources of the runtime, by path from the folder of the crates.\n");
    code.push_str("pub const FILES: &[(&str, &str)] = &[\n");
    // A hash of the sources names the cache where they are compiled, which a new version
    // of `lion` does not share.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for file in &files {
        let relative =
            file.strip_prefix(&crates).expect("inside the crates").to_string_lossy().replace('\\', "/");
        let _ = writeln!(code, "    ({relative:?}, include_str!({:?})),", file.display().to_string());
        for byte in relative.bytes().chain(fs::read(file).expect("readable")) {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
        println!("cargo:rerun-if-changed={}", file.display());
    }
    // Their manifests, made independent of the workspace of Lion.
    let workspace = fs::read_to_string(crates.join("../Cargo.toml")).expect("the workspace manifest");
    println!("cargo:rerun-if-changed={}", crates.join("../Cargo.toml").display());
    let setting = |key: &str| {
        workspace
            .lines()
            .find_map(|line| {
                line.strip_prefix(key)?.trim_start().strip_prefix('=').map(|value| value.trim().to_string())
            })
            .unwrap_or_else(|| panic!("the workspace sets `{key}`"))
    };
    for name in CRATES {
        let path = crates.join(name).join("Cargo.toml");
        println!("cargo:rerun-if-changed={}", path.display());
        let manifest = fs::read_to_string(&path).expect("a manifest");
        let mut own = String::new();
        for line in manifest.lines() {
            let line = match line.trim().split_once(".workspace = true") {
                Some(("version" | "edition" | "rust-version", _)) => {
                    let key = line.trim().split_once('.').expect("a key").0;
                    format!("{key} = {}", setting(key))
                }
                Some((dependency, _)) => format!("{dependency} = {{ path = \"../{dependency}\" }}"),
                None => line.to_string(),
            };
            own.push_str(&line);
            own.push('\n');
        }
        let _ = writeln!(code, "    (\"{name}/Cargo.toml\", {own:?}),");
        for byte in own.bytes() {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    code.push_str("];\n\n/// A hash of the sources.\n");
    let _ = writeln!(code, "pub const HASH: &str = \"{hash:016x}\";");
    let out = Path::new(&std::env::var("OUT_DIR").expect("set by cargo")).join("runtime_sources.rs");
    fs::write(out, code).expect("the build folder is writable");
}

fn collect(folder: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}
