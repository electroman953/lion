//! Runs the stages of the toolchain on a file and reports their diagnostics.

use std::io::{self, BufWriter, Write};
use std::process::ExitCode;

use lion_diagnostics::{Diagnostic, SourceId, SourceMap, render};
use lion_ir as ir;

/// Exit codes of the `lion` command.
pub mod exit {
    /// The program has errors and was refused, or could not be read.
    pub const REFUSED: u8 = 1;
    /// The program stopped on a bug (spec §18.1).
    pub const BUG: u8 = 2;
    /// The command line is wrong.
    pub const USAGE: u8 = 64;
    /// A defect in the implementation.
    pub const INTERNAL: u8 = 70;
}

pub fn check(path: &str) -> ExitCode {
    let Some((sources, id)) = load(path) else { return ExitCode::from(exit::REFUSED) };
    match front_end(&sources, id) {
        Some(_) => {
            println!("no errors in `{path}`");
            ExitCode::SUCCESS
        }
        None => ExitCode::from(exit::REFUSED),
    }
}

pub fn run(path: &str) -> ExitCode {
    let Some((sources, id)) = load(path) else { return ExitCode::from(exit::REFUSED) };
    let Some(program) = front_end(&sources, id) else { return ExitCode::from(exit::REFUSED) };
    let chunk = lion_vm::compile(&program);
    let mut out = BufWriter::new(io::stdout().lock());
    let mut report_alert = |alert: lion_vm::Alert| {
        eprintln!("{}", render(&alert.to_diagnostic(), &sources));
    };
    let result = lion_vm::run(&chunk, &mut out, &mut report_alert);
    // Everything the program wrote appears before the report of a bug.
    let _ = out.flush();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(trap) => {
            eprintln!("{}", render(&trap.to_diagnostic(), &sources));
            match trap {
                lion_vm::Trap::Bug { .. } => ExitCode::from(exit::BUG),
                lion_vm::Trap::Io(_) => ExitCode::from(exit::REFUSED),
                lion_vm::Trap::Internal { .. } => ExitCode::from(exit::INTERNAL),
            }
        }
    }
}

pub fn debug(stage: &str, path: &str) -> ExitCode {
    if !matches!(stage, "tokens" | "ast" | "ir" | "bytecode") {
        eprintln!("error: unknown stage `{stage}`; the stages are tokens, ast, ir and bytecode");
        return ExitCode::from(exit::USAGE);
    }
    let Some((sources, id)) = load(path) else { return ExitCode::from(exit::REFUSED) };
    let file = sources.get(id);
    let lexed = lion_syntax::lex(id, file.text());
    let output = match stage {
        "tokens" => {
            report(&lexed.diagnostics, &sources);
            let mut out = String::new();
            for token in &lexed.tokens {
                let (line, column) = file.line_col(token.span.start);
                out.push_str(&format!("{:<7} {}\n", format!("{line}:{column}"), token.kind.dump()));
            }
            print!("{out}");
            return exit_code(&lexed.diagnostics);
        }
        "ast" => {
            let parsed = lion_syntax::parse(&lexed.tokens);
            let diagnostics: Vec<Diagnostic> =
                lexed.diagnostics.into_iter().chain(parsed.diagnostics).collect();
            report(&diagnostics, &sources);
            print!("{}", lion_syntax::print_module(&parsed.module));
            return exit_code(&diagnostics);
        }
        "ir" => front_end(&sources, id).map(|program| ir::print_program(&program)),
        _ => front_end(&sources, id).map(|program| lion_vm::disassemble(&lion_vm::compile(&program))),
    };
    match output {
        Some(output) => {
            print!("{output}");
            ExitCode::SUCCESS
        }
        None => ExitCode::from(exit::REFUSED),
    }
}

/// Reads a source file, reporting why it cannot be used.
fn load(path: &str) -> Option<(SourceMap, SourceId)> {
    let problem = if !path.ends_with(".lion") {
        Some(
            Diagnostic::error(format!("`{path}` is not a Lion source file"))
                .with_note("Lion source files have the `.lion` extension (§4.1)"),
        )
    } else {
        None
    };
    let empty = SourceMap::new();
    if let Some(problem) = problem {
        eprint!("{}", render(&problem, &empty));
        return None;
    }
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprint!("{}", render(&Diagnostic::error(format!("cannot read `{path}`: {error}")), &empty));
            return None;
        }
    };
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => {
            let offset = error.utf8_error().valid_up_to();
            let line = error.as_bytes()[..offset].iter().filter(|&&b| b == b'\n').count() + 1;
            let diagnostic = Diagnostic::error(format!("`{path}` is not valid UTF-8"))
                .with_note(format!("the first invalid byte is on line {line}"))
                .with_note("Lion source files are encoded in UTF-8 (§4.1)");
            eprint!("{}", render(&diagnostic, &empty));
            return None;
        }
    };
    let mut sources = SourceMap::new();
    let id = sources.add(path, text);
    Some((sources, id))
}

/// Lexes, parses and checks a file. Returns the program if it has no errors, after
/// reporting every diagnostic.
fn front_end(sources: &SourceMap, id: SourceId) -> Option<ir::Program> {
    let lexed = lion_syntax::lex(id, sources.get(id).text());
    let parsed = lion_syntax::parse(&lexed.tokens);
    let mut diagnostics: Vec<Diagnostic> = lexed.diagnostics.into_iter().chain(parsed.diagnostics).collect();
    // Checking a tree with syntax errors would only add confusing messages.
    let program = if diagnostics.iter().any(Diagnostic::is_fatal) {
        None
    } else {
        let checked = lion_sema::check(&parsed.module);
        diagnostics.extend(checked.diagnostics);
        checked.program
    };
    report(&diagnostics, sources);
    program
}

fn report(diagnostics: &[Diagnostic], sources: &SourceMap) {
    for diagnostic in diagnostics {
        eprintln!("{}", render(diagnostic, sources));
    }
    let errors = diagnostics.iter().filter(|d| d.is_fatal()).count();
    if errors > 0 {
        let plural = if errors == 1 { "" } else { "s" };
        eprintln!("{errors} error{plural} found");
    }
}

fn exit_code(diagnostics: &[Diagnostic]) -> ExitCode {
    if diagnostics.iter().any(Diagnostic::is_fatal) {
        ExitCode::from(exit::REFUSED)
    } else {
        ExitCode::SUCCESS
    }
}
