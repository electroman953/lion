//! Runs the stages of the toolchain on a file and reports their diagnostics.

use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::process::ExitCode;

use lion_diagnostics::{Diagnostic, SourceId, SourceMap, render};
use lion_ir as ir;
use lion_syntax::ast;

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
    let Some((mut sources, id)) = load(path) else { return ExitCode::from(exit::REFUSED) };
    match front_end(&mut sources, id) {
        Some(_) => {
            println!("no errors in `{path}`");
            ExitCode::SUCCESS
        }
        None => ExitCode::from(exit::REFUSED),
    }
}

pub fn run(path: &str) -> ExitCode {
    let Some((mut sources, id)) = load(path) else { return ExitCode::from(exit::REFUSED) };
    let Some(program) = front_end(&mut sources, id) else { return ExitCode::from(exit::REFUSED) };
    let chunk = lion_vm::compile(&program);
    let mut out = BufWriter::new(io::stdout().lock());
    // A bug or an alert in the standard library is shown at the call that led there.
    let hidden = |span: lion_diagnostics::Span| sources.get(span.source).name().starts_with("<std>/");
    let mut report_alert = |alert: lion_vm::Alert| {
        eprintln!("{}", render(&alert.located(&hidden).to_diagnostic(), &sources));
    };
    let mut input = io::stdin().lock();
    let result = lion_vm::run(&chunk, &mut out, &mut input, &mut report_alert);
    // Everything the program wrote appears before the report of a bug.
    let _ = out.flush();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(lion_vm::Trap::Exit(code)) => ExitCode::from(code),
        Err(trap) => {
            let trap = trap.located(&hidden);
            eprintln!("{}", render(&trap.to_diagnostic(), &sources));
            match trap {
                lion_vm::Trap::Bug { .. } => ExitCode::from(exit::BUG),
                lion_vm::Trap::Io(_) | lion_vm::Trap::Failure { .. } => ExitCode::from(exit::REFUSED),
                lion_vm::Trap::Exit(code) => ExitCode::from(code),
            }
        }
    }
}

/// `lion test`: the tests of a file, or of every Lion file of a folder (§24.1).
pub fn test(path: &str) -> ExitCode {
    let files = if path.ends_with(".lion") {
        vec![std::path::PathBuf::from(path)]
    } else if Path::new(path).is_dir() {
        let mut files = Vec::new();
        collect_lion_files(Path::new(path), &mut files);
        files
    } else {
        eprintln!("error: `{path}` is neither a Lion file nor a folder");
        return ExitCode::from(exit::USAGE);
    };
    let (mut passed, mut failed, mut refused) = (0, 0, 0);
    for file in &files {
        let name = file.display().to_string();
        let Some((mut sources, id)) = load(&name) else {
            refused += 1;
            continue;
        };
        let Some(program) = front_end(&mut sources, id) else {
            refused += 1;
            continue;
        };
        let chunk = lion_vm::compile(&program);
        let hidden = |span: lion_diagnostics::Span| sources.get(span.source).name().starts_with("<std>/");
        for (index, (test, _)) in chunk.tests.iter().enumerate() {
            let mut out = BufWriter::new(io::stdout().lock());
            let mut input = io::stdin().lock();
            let mut report_alert = |alert: lion_vm::Alert| {
                let _ = io::stdout().flush();
                eprintln!("{}", render(&alert.located(&hidden).to_diagnostic(), &sources));
            };
            let result = lion_vm::run_test(&chunk, index, &mut out, &mut input, &mut report_alert);
            let _ = out.flush();
            drop(out);
            let problems: Vec<Diagnostic> = match result {
                Ok(failures) => failures
                    .into_iter()
                    .map(|failure| {
                        let mut diagnostic = Diagnostic::error(failure.message);
                        if let Some(span) = failure.span {
                            diagnostic = diagnostic.with_primary(span, "");
                        }
                        diagnostic
                    })
                    .collect(),
                Err(lion_vm::Trap::Exit(code)) => {
                    vec![Diagnostic::error(format!("the test stopped the program with `exit({code})`"))]
                }
                Err(trap) => vec![trap.located(&hidden).to_diagnostic()],
            };
            if problems.is_empty() {
                println!("test \"{test}\" in {name} ... ok");
                passed += 1;
            } else {
                println!("test \"{test}\" in {name} ... FAILED");
                for problem in &problems {
                    eprintln!("{}", render(problem, &sources));
                }
                failed += 1;
            }
        }
    }
    let plural = |count: usize, word: &str| format!("{count} {word}{}", if count == 1 { "" } else { "s" });
    let mut summary = format!("{}: {} passed, {} failed", plural(passed + failed, "test"), passed, failed);
    if refused > 0 {
        summary.push_str(&format!("; {} with errors", plural(refused, "file")));
    }
    println!("{summary}");
    if failed + refused > 0 { ExitCode::from(exit::REFUSED) } else { ExitCode::SUCCESS }
}

/// The Lion files of a folder and of its subfolders, in order; hidden folders and the
/// build folder `target` are skipped.
fn collect_lion_files(folder: &Path, files: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    let mut paths: Vec<std::path::PathBuf> =
        entries.filter_map(|entry| entry.ok().map(|entry| entry.path())).collect();
    paths.sort();
    for path in paths {
        let name = path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
        if path.is_dir() {
            if !name.starts_with('.') && name != "target" {
                collect_lion_files(&path, files);
            }
        } else if name.ends_with(".lion") {
            files.push(path);
        }
    }
}

pub fn debug(stage: &str, path: &str) -> ExitCode {
    if !matches!(stage, "tokens" | "ast" | "ir" | "bytecode") {
        eprintln!("error: unknown stage `{stage}`; the stages are tokens, ast, ir and bytecode");
        return ExitCode::from(exit::USAGE);
    }
    let Some((mut sources, id)) = load(path) else { return ExitCode::from(exit::REFUSED) };
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
            let parsed = lion_syntax::parse(file.text(), &lexed.tokens);
            let diagnostics: Vec<Diagnostic> =
                lexed.diagnostics.into_iter().chain(parsed.diagnostics).collect();
            report(&diagnostics, &sources);
            print!("{}", lion_syntax::print_module(&parsed.module));
            return exit_code(&diagnostics);
        }
        "ir" => front_end(&mut sources, id).map(|program| ir::print_program(&program)),
        _ => front_end(&mut sources, id).map(|program| lion_vm::disassemble(&lion_vm::compile(&program))),
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

/// Lexes, parses and checks a script and the modules it uses. Returns the program if
/// it has no errors, after reporting every diagnostic.
fn front_end(sources: &mut SourceMap, id: SourceId) -> Option<ir::Program> {
    let mut diagnostics = Vec::new();
    // The files of the program: the script, then each module that a file uses, once.
    let mut files: Vec<(String, SourceId, bool)> = vec![(String::new(), id, false)];
    let mut modules = Vec::new();
    let folder = Path::new(sources.get(id).name()).parent().map(Path::to_path_buf).unwrap_or_default();
    let mut index = 0;
    while index < files.len() {
        let file = files[index].1;
        let text = sources.get(file).text().to_string();
        let lexed = lion_syntax::lex(file, &text);
        let parsed = lion_syntax::parse(&text, &lexed.tokens);
        diagnostics.extend(lexed.diagnostics);
        diagnostics.extend(parsed.diagnostics);
        for stmt in &parsed.module.stmts {
            let ast::StmtKind::Use(path) = &stmt.kind else { continue };
            let parts: Vec<&str> = path.iter().map(|part| part.name.as_str()).collect();
            let name = parts.join(".");
            if files.iter().any(|(known, ..)| *known == name) {
                continue;
            }
            match find_module(&folder, &parts) {
                Ok((file_name, text, standard)) => {
                    let module = sources.add(file_name, text);
                    files.push((name, module, standard));
                }
                Err(looked_at) => {
                    let span = path[0].span.to(path[path.len() - 1].span);
                    diagnostics.push(
                        Diagnostic::error(format!("cannot find the module `{name}`"))
                            .with_primary(span, "")
                            .with_note(format!("there is no file `{looked_at}`"))
                            .with_note(format!(
                                "the modules of the standard library are {}",
                                lion_std::MODULES.join(", ")
                            )),
                    );
                }
            }
        }
        modules.push(parsed.module);
        index += 1;
    }
    // Checking a tree with syntax errors would only add confusing messages.
    let program = if diagnostics.iter().any(Diagnostic::is_fatal) {
        None
    } else {
        let files: Vec<lion_sema::Source> = files
            .iter()
            .zip(&modules)
            .map(|((name, _, standard), module)| lion_sema::Source {
                name: name.clone(),
                module,
                standard: *standard,
            })
            .collect();
        let checked = lion_sema::check_program(&files);
        diagnostics.extend(checked.diagnostics);
        checked.program
    };
    report(&diagnostics, sources);
    program
}

/// The module `a.b`: a module of the standard library, or the file `a/b.lion` in the
/// folder of the script (C61). On failure, the file that was looked for.
fn find_module(folder: &Path, parts: &[&str]) -> Result<(String, String, bool), String> {
    let name = parts.join(".");
    if let Some(text) = lion_std::module(&name) {
        return Ok((format!("<std>/{name}.lion"), text.to_string(), true));
    }
    let mut path = folder.to_path_buf();
    for part in parts {
        path.push(part);
    }
    path.set_extension("lion");
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok((path.display().to_string(), text, false)),
        Err(_) => Err(path.display().to_string()),
    }
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
