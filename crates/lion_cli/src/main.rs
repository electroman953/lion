//! The `lion` command (spec §24). Only the commands that work are offered.

mod driver;

use std::panic;
use std::process::ExitCode;

use driver::exit;

const USAGE: &str = "\
usage:
  lion run <file.lion>             check a program, then run it (interpreted mode)
  lion check <file.lion>           check a program without running it
  lion debug <stage> <file.lion>   show a stage of the compiler: tokens, ast, ir or bytecode
  lion --version

not implemented yet: `lion build`, `lion test`, `lion fmt` and the interactive mode (`lion` alone)
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
        ["debug", stage, file] => driver::debug(stage, file),
        [command @ ("build" | "test" | "fmt"), ..] => {
            eprintln!("error: `lion {command}` is not implemented yet\n\n{USAGE}");
            ExitCode::from(exit::USAGE)
        }
        [] => {
            eprintln!("error: the interactive mode is not implemented yet\n\n{USAGE}");
            ExitCode::from(exit::USAGE)
        }
        _ => {
            eprintln!("error: unknown command `{}`\n\n{USAGE}", args.join(" "));
            ExitCode::from(exit::USAGE)
        }
    }
}
