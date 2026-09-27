//! The `lion` command (spec §24). Only the commands that work are offered.

mod driver;
mod native;
mod project;

use std::panic;
use std::process::ExitCode;

use driver::exit;

const USAGE: &str = "\
usage:
  lion run <file.lion>             check a program, then run it (interpreted mode)
  lion check <file.lion>           check a program without running it
  lion build <file.lion> [-o <executable>]
                                   compile a program to native code (compiled mode)
  lion test [file.lion | folder]   run the tests of a file, or of every file of a folder
  lion new <name>                  create a project: lion.toml and main.lion
  lion add <git address | folder> [name]
                                   use a package in the project (--git for a local repository)
  lion remove <name>               stop using a package
  lion update [name]               take the latest versions that lion.toml accepts
  lion run | check | build | test  without a file, in a project: its main.lion
  lion fmt [--check] [file.lion | folder]
                                   lay out files in the official style (4 spaces per block)
  lion debug <stage> <file.lion>   show a stage of the compiler: tokens, ast, ir, bytecode
                                   or rust
  lion --version

  lion                             the interactive mode: type Lion line by line
";

fn main() -> ExitCode {
    // A panic is a defect of the implementation: report it as such, never as a
    // problem in the user's program.
    panic::set_hook(Box::new(|info| {
        let location = info.location().map(|l| format!(" at {}:{}", l.file(), l.line())).unwrap_or_default();
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_default();
        eprintln!("internal compiler error: {payload}");
        eprintln!("  = note: raised{location}");
        eprintln!(
            "  = note: this is a defect in the Lion implementation, not in your program; please report it"
        );
    }));
    let args: Vec<String> = std::env::args().skip(1).collect();
    panic::catch_unwind(|| dispatch(&args)).unwrap_or(ExitCode::from(exit::INTERNAL))
}

fn dispatch(args: &[String]) -> ExitCode {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["--help" | "-h" | "help"] => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        ["--version" | "-V"] => {
            println!("lion {} (language specification 0.1)", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        ["run", file] => driver::run(file),
        ["check", file] => driver::check(file),
        ["run"] => with_main(driver::run),
        ["check"] => with_main(driver::check),
        ["build"] => with_main(|main| driver::build(main, None)),
        ["new", name] => done(project::new_project(name)),
        ["add", "--git", source] => done(project::add(source, None, true)),
        ["add", "--git", source, name] => done(project::add(source, Some(name), true)),
        ["add", source] if !source.starts_with('-') => done(project::add(source, None, false)),
        ["add", source, name] if !source.starts_with('-') && !name.starts_with('-') => {
            done(project::add(source, Some(name), false))
        }
        ["remove", name] => done(project::remove(name)),
        ["update"] => done(project::update(None)),
        ["update", name] => done(project::update(Some(name))),
        ["debug", stage, file] => driver::debug(stage, file),
        ["test"] => match project::find_root(std::path::Path::new(".")) {
            Some(root) => {
                let current =
                    std::env::current_dir().ok().and_then(|folder| std::fs::canonicalize(folder).ok());
                let shown = match current
                    .and_then(|current| root.strip_prefix(&current).ok().map(std::path::Path::to_path_buf))
                {
                    Some(relative) if relative.as_os_str().is_empty() => ".".to_string(),
                    Some(relative) => relative.display().to_string(),
                    None => root.display().to_string(),
                };
                driver::test(&shown)
            }
            None => driver::test("."),
        },
        ["test", path] => driver::test(path),
        ["fmt"] => driver::fmt(".", false),
        ["fmt", "--check"] => driver::fmt(".", true),
        ["fmt", "--check", path] | ["fmt", path, "--check"] => driver::fmt(path, true),
        ["fmt", path] => driver::fmt(path, false),
        ["build", file] => driver::build(file, None),
        ["build", file, "-o", output] | ["build", "-o", output, file] => driver::build(file, Some(output)),
        [] => driver::interactive(),
        _ => {
            eprintln!("error: unknown command `{}`\n\n{USAGE}", args.join(" "));
            ExitCode::from(exit::USAGE)
        }
    }
}

/// A command of the project: a message, or the problem that stopped it.
fn done(result: Result<String, String>) -> ExitCode {
    match result {
        Ok(message) => {
            println!("{message}");
            ExitCode::SUCCESS
        }
        Err(problem) => {
            eprintln!("error: {problem}");
            ExitCode::from(exit::REFUSED)
        }
    }
}

/// Runs `command` on the `main.lion` of the project of the current folder.
fn with_main(command: impl FnOnce(&str) -> ExitCode) -> ExitCode {
    let Some(root) = project::find_root(std::path::Path::new(".")) else {
        eprintln!(
            "error: which file? There is no project here (no `lion.toml`): write `lion run file.lion`\n\n{USAGE}"
        );
        return ExitCode::from(exit::USAGE);
    };
    let main = root.join("main.lion");
    if !main.is_file() {
        eprintln!("error: the project has no `main.lion`, the script that `lion run` runs");
        return ExitCode::from(exit::REFUSED);
    }
    // The paths of the messages start from the current folder when they can.
    let current = std::env::current_dir().ok();
    let shown = current
        .and_then(|current| main.strip_prefix(&current).ok().map(std::path::Path::to_path_buf))
        .unwrap_or(main);
    command(&shown.display().to_string())
}
