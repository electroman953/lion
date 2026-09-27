//! `lion build` (spec §22.1): the Rust code of a program, made by `lion_codegen`, is
//! compiled by cargo and rustc, and so by LLVM, with the runtime of compiled programs.
//!
//! The sources of that runtime are part of `lion`. The first build writes them to a
//! cache and compiles them once; the next builds compile only the program. Each program
//! has its own package in the cache, named after the path of the executable, and all
//! share the folder of the compiled code.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/runtime_sources.rs"));
}

/// Why a program could not be compiled to native code.
pub enum BuildError {
    /// No Rust toolchain was found.
    NoCargo,
    /// A file of the cache or the executable could not be written.
    Io(String),
    /// The Rust code did not compile: a defect of the implementation.
    Rust(String),
}

/// Compiles `code`, the Rust code of a program, into the executable `output`.
pub fn build(code: &str, output: &Path) -> Result<(), BuildError> {
    let cargo = find_cargo().ok_or(BuildError::NoCargo)?;
    let cache = cache_folder().join(format!("runtime-{}-{}", env!("CARGO_PKG_VERSION"), embedded::HASH));
    let io = |what: &Path, error: std::io::Error| {
        BuildError::Io(format!("cannot write `{}`: {error}", what.display()))
    };
    // The runtime, written once.
    let ready = cache.join("crates").join("ready");
    if !ready.exists() {
        for (path, text) in embedded::FILES {
            let path = cache.join("crates").join(path);
            if let Some(folder) = path.parent() {
                fs::create_dir_all(folder).map_err(|error| io(folder, error))?;
            }
            fs::write(&path, text).map_err(|error| io(&path, error))?;
        }
        fs::write(&ready, "").map_err(|error| io(&ready, error))?;
        eprintln!("compiling the runtime of Lion programs; this happens only once");
    }
    // The package of the program.
    let absolute =
        env::current_dir().map(|folder| folder.join(output)).unwrap_or_else(|_| output.to_path_buf());
    let name = format!("p{:016x}", fnv(absolute.to_string_lossy().as_bytes()));
    let package = cache.join("programs").join(&name);
    let sources = package.join("src");
    fs::create_dir_all(&sources).map_err(|error| io(&sources, error))?;
    let manifest = format!(
        "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
         [dependencies]\nlion_native = {{ path = \"../../crates/lion_native\" }}\n\n\
         [workspace]\n\n[profile.release]\ndebug = false\n"
    );
    let main = "mod program;\n\nfn main() -> std::process::ExitCode {\n    program::main()\n}\n";
    for (path, text) in [
        (package.join("Cargo.toml"), manifest.as_str()),
        (sources.join("main.rs"), main),
        (sources.join("program.rs"), code),
    ] {
        // An unchanged file keeps its date, so that cargo does not compile it again.
        if fs::read_to_string(&path).ok().as_deref() != Some(text) {
            fs::write(&path, text).map_err(|error| io(&path, error))?;
        }
    }
    let target = cache.join("target");
    let result = Command::new(&cargo)
        .args(["build", "--release", "--offline", "--quiet", "--manifest-path"])
        .arg(package.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", &target)
        .output()
        .map_err(|error| BuildError::Io(format!("cannot run `{}`: {error}", cargo.display())))?;
    if !result.status.success() {
        return Err(BuildError::Rust(String::from_utf8_lossy(&result.stderr).into_owned()));
    }
    let executable = target.join("release").join(format!("{name}{}", env::consts::EXE_SUFFIX));
    fs::copy(&executable, output).map_err(|error| io(output, error))?;
    // What was used now is marked; what was not used for a long time is removed (C99).
    let _ = fs::write(package.join(USED), "");
    let _ = fs::write(cache.join(USED), "");
    clean(&cache_folder(), &cache, &target);
    Ok(())
}

/// The file whose date says when a runtime or a program of the cache was last used.
const USED: &str = ".used";

/// How long a runtime or a program of the cache is kept without being used.
const KEPT: std::time::Duration = std::time::Duration::from_secs(30 * 24 * 60 * 60);

/// Removes from the cache the runtimes of other versions of `lion` and the programs that
/// no build used for `KEPT` (C99). A failure only leaves files behind.
fn clean(cache: &Path, current: &Path, target: &Path) {
    for runtime in stale(cache, "runtime-") {
        if runtime != current {
            let _ = fs::remove_dir_all(runtime);
        }
    }
    for program in stale(&current.join("programs"), "p") {
        if let Some(name) = program.file_name() {
            let executable = format!("{}{}", name.to_string_lossy(), env::consts::EXE_SUFFIX);
            let _ = fs::remove_file(target.join("release").join(executable));
        }
        let _ = fs::remove_dir_all(program);
    }
}

/// The folders of `folder` whose name starts with `prefix` and that were not used for
/// `KEPT`, by the date of their mark, or of the folder when it has none.
fn stale(folder: &Path, prefix: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(folder) else { return Vec::new() };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir() && path.file_name().is_some_and(|name| name.to_string_lossy().starts_with(prefix))
        })
        .filter(|path| {
            let date = fs::metadata(path.join(USED))
                .or_else(|_| fs::metadata(path))
                .and_then(|meta| meta.modified());
            date.ok().and_then(|date| date.elapsed().ok()).is_some_and(|age| age > KEPT)
        })
        .collect()
}

/// The cargo of the Rust toolchain: `LION_CARGO`, then the one of the `PATH`, then the
/// one that rustup installs.
fn find_cargo() -> Option<PathBuf> {
    if let Some(cargo) = env::var_os("LION_CARGO") {
        return Some(PathBuf::from(cargo));
    }
    let name = format!("cargo{}", env::consts::EXE_SUFFIX);
    let mut candidates: Vec<PathBuf> = env::var_os("PATH")
        .map(|paths| env::split_paths(&paths).map(|folder| folder.join(&name)).collect())
        .unwrap_or_default();
    if let Some(home) = env::var_os("CARGO_HOME") {
        candidates.push(PathBuf::from(home).join("bin").join(&name));
    }
    if let Some(home) = env::var_os("HOME") {
        candidates.push(PathBuf::from(home).join(".cargo").join("bin").join(&name));
    }
    candidates.into_iter().find(|candidate| candidate.is_file())
}

/// Where compiled runtimes and programs are kept: `LION_CACHE`, or the cache folder of
/// the user.
pub fn cache_folder() -> PathBuf {
    if let Some(folder) = env::var_os("LION_CACHE") {
        return PathBuf::from(folder);
    }
    if let Some(folder) = env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(folder).join("lion");
    }
    match env::var_os("HOME") {
        Some(home) => PathBuf::from(home).join(".cache").join("lion"),
        None => env::temp_dir().join("lion-cache"),
    }
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(0xcbf2_9ce4_8422_2325, |hash, byte| (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_unused_for_a_long_time_are_stale() {
        let folder = env::temp_dir().join(format!("lion-clean-test-{}", std::process::id()));
        let (old, recent, other) = (folder.join("p-old"), folder.join("p-recent"), folder.join("q-old"));
        for path in [&old, &recent, &other] {
            fs::create_dir_all(path).unwrap();
            fs::write(path.join(USED), "").unwrap();
        }
        let long_ago = std::time::SystemTime::now() - KEPT - std::time::Duration::from_secs(60);
        for path in [&old, &other] {
            fs::File::options().write(true).open(path.join(USED)).unwrap().set_modified(long_ago).unwrap();
        }
        assert_eq!(stale(&folder, "p"), [old]);
        let _ = fs::remove_dir_all(folder);
    }
}
