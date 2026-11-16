//! `lion mcp`: a server of the Model Context Protocol (C108), so that any AI assistant
//! that speaks it, such as Claude Code, writes Lion well: a guide of the language, the
//! API of each standard module, the commands `check`, `run`, `test`, `fmt` and `build`,
//! and a list of what Lion lacks, which assistants fill and the authors read.
//!
//! The messages are JSON-RPC 2.0, one per line, on the standard input and output. Each
//! command runs `lion` itself in a process of its own, with a time limit: a program that
//! never ends or crashes leaves the server as it was.

use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lion_runtime::json::{self, Json};

const GUIDE: &str = include_str!("../../../docs/ai-guide.md");

/// The most characters of an output given back; the rest is cut.
const MAX_OUTPUT: usize = 30_000;

const INSTRUCTIONS: &str = "Lion is a statically typed language with value semantics (files `.lion`). \
Before writing Lion, read `lion_guide` once: it lists the syntax and the mistakes to avoid. \
Read the API of a standard module with `lion_module` before using it. \
After every change, call `lion_check` (fast, precise diagnostics with fixes), then `lion_run` or `lion_test`. \
For speed, `lion_build` makes a native executable with the same results. \
When Lion lacks something you need (a function, a module, a construct, a clearer message), \
work around it and call `lion_report_gap`: the authors read that list to improve Lion. \
Read `lion_gaps` first to avoid reporting the same thing twice.";

/// Serves on the standard input and output until the input ends.
pub fn serve() -> ExitCode {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let answer = match json::parse(&line) {
            Ok(message) => handle(&message),
            Err(problem) => Some(failure(Json::Null, -32700, &format!("not JSON: {problem}"))),
        };
        if let Some(answer) = answer
            && writeln!(stdout, "{answer}").and_then(|()| stdout.flush()).is_err()
        {
            break;
        }
    }
    ExitCode::SUCCESS
}

/// The answer to a message; none to a notification.
fn handle(message: &Json) -> Option<Json> {
    let id = message.get("id").cloned()?;
    let method = message.get("method").and_then(Json::as_text).unwrap_or_default();
    let params = message.get("params").cloned().unwrap_or(Json::Null);
    let result = match method {
        "initialize" => {
            let version = params.get("protocolVersion").and_then(Json::as_text).unwrap_or("2025-06-18");
            Json::object([
                ("protocolVersion", Json::text(version)),
                ("capabilities", Json::object([("tools", Json::object([]))])),
                (
                    "serverInfo",
                    Json::object([
                        ("name", Json::text("lion")),
                        ("version", Json::text(env!("CARGO_PKG_VERSION"))),
                    ]),
                ),
                ("instructions", Json::text(INSTRUCTIONS)),
            ])
        }
        "ping" => Json::object([]),
        "tools/list" => Json::object([("tools", Json::List(tools()))]),
        "tools/call" => {
            let name = params.get("name").and_then(Json::as_text).unwrap_or_default();
            let arguments = params.get("arguments").cloned().unwrap_or(Json::object([]));
            let (text, is_error) = match call(name, &arguments) {
                Ok(text) => (text, false),
                Err(text) => (text, true),
            };
            Json::object([
                (
                    "content",
                    Json::List(vec![Json::object([
                        ("type", Json::text("text")),
                        ("text", Json::text(text)),
                    ])]),
                ),
                ("isError", Json::Bool(is_error)),
            ])
        }
        other => return Some(failure(id, -32601, &format!("unknown method `{other}`"))),
    };
    Some(Json::object([("jsonrpc", Json::text("2.0")), ("id", id), ("result", result)]))
}

fn failure(id: Json, code: i64, message: &str) -> Json {
    Json::object([
        ("jsonrpc", Json::text("2.0")),
        ("id", id),
        ("error", Json::object([("code", Json::int(code)), ("message", Json::text(message))])),
    ])
}

/// A property of the arguments of a tool.
fn property(name: &str, kind: &str, description: &str) -> (String, Json) {
    let mut schema =
        vec![("type".to_string(), Json::text(kind)), ("description".to_string(), Json::text(description))];
    if kind == "array" {
        schema.push(("items".to_string(), Json::object([("type", Json::text("string"))])));
    }
    (name.to_string(), Json::Object(schema))
}

fn tool(name: &str, description: &str, properties: Vec<(String, Json)>, required: &[&str]) -> Json {
    Json::object([
        ("name", Json::text(name)),
        ("description", Json::text(description)),
        (
            "inputSchema",
            Json::object([
                ("type", Json::text("object")),
                ("properties", Json::Object(properties)),
                ("required", Json::List(required.iter().map(|r| Json::text(*r)).collect())),
            ]),
        ),
    ])
}

fn tools() -> Vec<Json> {
    let path = || property("path", "string", "A .lion file (absolute, or from cwd).");
    let code =
        || property("code", "string", "Lion source to use instead of a file (saved in a temporary file).");
    let cwd = || property("cwd", "string", "The folder to run in; by default, the folder of the server.");
    vec![
        tool(
            "lion_guide",
            "The guide to write correct Lion: syntax, types, errors, modules, performance, and the common mistakes. Read it once before writing Lion.",
            vec![],
            &[],
        ),
        tool(
            "lion_module",
            "The source of a standard module of Lion, with the documentation of each function: its exact API. Without a name, the list of the modules.",
            vec![property(
                "name",
                "string",
                "A module: text, math, random, files, csv, json, sets, dates, time, net, html, regex, url, base64, hash, system, term, ui.",
            )],
            &[],
        ),
        tool(
            "lion_check",
            "Checks a Lion program without running it (types, names, errors), in milliseconds. Give `path` or `code`.",
            vec![path(), code(), cwd()],
            &[],
        ),
        tool(
            "lion_run",
            "Runs a Lion program (interpreted mode) and gives its exit code, output and errors. Give `path` or `code`.",
            vec![
                path(),
                code(),
                cwd(),
                property("args", "array", "The words given to the program (system.args())."),
                property("stdin", "string", "What the program reads at the keyboard (ask)."),
                property("timeout", "number", "Seconds before the program is stopped; 30 by default."),
            ],
            &[],
        ),
        tool(
            "lion_test",
            "Runs the `test` blocks of a file or of every file of a folder.",
            vec![path(), cwd()],
            &["path"],
        ),
        tool(
            "lion_fmt",
            "Lays out Lion in the official style. With `path`, the file is rewritten; with `code`, the laid-out code is given back.",
            vec![path(), code(), cwd()],
            &[],
        ),
        tool(
            "lion_build",
            "Compiles a Lion program to a native executable (3 to 13 times faster, same results). Needs cargo; the first build takes a minute.",
            vec![path(), cwd(), property("output", "string", "The executable to write.")],
            &["path"],
        ),
        tool(
            "lion_report_gap",
            "Notes something Lion lacks or does badly, met while writing a program: a missing function or module, a construct, a confusing message, a slow operation. The authors read this list to improve Lion. Read lion_gaps first.",
            vec![
                property("title", "string", "A short name: `No string padding function`."),
                property("need", "string", "What you wanted to do, and why Lion did not allow it."),
                property("workaround", "string", "What you did instead, if anything."),
                property("example", "string", "The Lion code you wished you could write."),
            ],
            &["title", "need"],
        ),
        tool("lion_gaps", "The list of what Lion lacks, as reported so far.", vec![], &[]),
    ]
}

fn text_argument<'a>(arguments: &'a Json, name: &str) -> Option<&'a str> {
    arguments.get(name).and_then(Json::as_text).filter(|value| !value.trim().is_empty())
}

fn call(name: &str, arguments: &Json) -> Result<String, String> {
    match name {
        "lion_guide" => Ok(GUIDE.to_string()),
        "lion_module" => match text_argument(arguments, "name") {
            None => Ok(format!("The standard modules: {}.", lion_std::MODULES.join(", "))),
            Some(module) => lion_std::module(module).map(str::to_string).ok_or_else(|| {
                format!(
                    "there is no standard module `{module}`; the modules are {}",
                    lion_std::MODULES.join(", ")
                )
            }),
        },
        "lion_check" | "lion_run" | "lion_fmt" => {
            let (file, temporary) = source(arguments)?;
            let folder = folder(arguments);
            let result = match name {
                "lion_check" => run(&["check".into(), file.display().to_string()], &folder, "", 60.0),
                "lion_fmt" => {
                    let formatted = run(&["fmt".into(), file.display().to_string()], &folder, "", 60.0);
                    if temporary && formatted.is_ok() {
                        std::fs::read_to_string(&file).map_err(|problem| problem.to_string())
                    } else {
                        formatted
                    }
                }
                _ => {
                    let mut words = vec!["run".to_string(), file.display().to_string()];
                    if let Some(Json::List(args)) = arguments.get("args") {
                        words.extend(args.iter().filter_map(Json::as_text).map(str::to_string));
                    }
                    let stdin = text_argument(arguments, "stdin").unwrap_or_default();
                    let timeout = match arguments.get("timeout") {
                        Some(Json::Number(n)) => n.parse::<f64>().unwrap_or(30.0).clamp(1.0, 600.0),
                        _ => 30.0,
                    };
                    run(&words, &folder, stdin, timeout)
                }
            };
            if temporary {
                let _ = std::fs::remove_file(&file);
            }
            result
        }
        "lion_test" | "lion_build" => {
            let path = text_argument(arguments, "path").ok_or("give `path`")?;
            let file = resolve(arguments, path);
            let folder = folder(arguments);
            let mut words = vec![
                if name == "lion_test" { "test" } else { "build" }.to_string(),
                file.display().to_string(),
            ];
            if let Some(output) = text_argument(arguments, "output") {
                words.extend(["-o".to_string(), output.to_string()]);
            }
            run(&words, &folder, "", if name == "lion_build" { 600.0 } else { 120.0 })
        }
        "lion_report_gap" => report_gap(arguments),
        "lion_gaps" => {
            let path = gaps_file();
            Ok(std::fs::read_to_string(&path)
                .unwrap_or_else(|_| format!("Nothing reported yet ({}).", path.display())))
        }
        other => Err(format!("there is no tool `{other}`")),
    }
}

/// The file to give `lion`: `path`, or `code` saved in a temporary file.
fn source(arguments: &Json) -> Result<(PathBuf, bool), String> {
    if let Some(code) = arguments.get("code").and_then(Json::as_text) {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let folder = std::env::temp_dir().join(format!("lion-mcp-{}", std::process::id()));
        std::fs::create_dir_all(&folder).map_err(|problem| format!("cannot write the code: {problem}"))?;
        let number = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let file = folder.join(format!("snippet{number}.lion"));
        std::fs::write(&file, code).map_err(|problem| format!("cannot write the code: {problem}"))?;
        return Ok((file, true));
    }
    let path = text_argument(arguments, "path").ok_or("give `path` (a .lion file) or `code`")?;
    Ok((resolve(arguments, path), false))
}

fn resolve(arguments: &Json, path: &str) -> PathBuf {
    let path = Path::new(path);
    match text_argument(arguments, "cwd") {
        Some(cwd) if path.is_relative() => Path::new(cwd).join(path),
        _ => path.to_path_buf(),
    }
}

/// Where the program runs: `cwd`, or the folder of the server.
fn folder(arguments: &Json) -> PathBuf {
    text_argument(arguments, "cwd")
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Runs `lion words…` and gives what it did.
fn run(words: &[String], folder: &Path, stdin: &str, timeout: f64) -> Result<String, String> {
    let lion = std::env::current_exe().map_err(|problem| format!("cannot find lion: {problem}"))?;
    let mut child = Command::new(lion)
        .args(words)
        .current_dir(folder)
        .env("LION_UI", "headless")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|problem| format!("cannot run lion: {problem}"))?;
    let mut input = child.stdin.take().expect("piped");
    let stdin = stdin.to_string();
    std::thread::spawn(move || {
        let _ = input.write_all(stdin.as_bytes());
    });
    let reader = |stream: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut stream) = stream {
                let _ = stream.read_to_end(&mut bytes);
            }
            String::from_utf8_lossy(&bytes).into_owned()
        })
    };
    let out = reader(child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let err = reader(child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let started = Instant::now();
    let mut stopped = false;
    let status = loop {
        if let Ok(Some(status)) = child.try_wait() {
            break Some(status);
        }
        if started.elapsed() > Duration::from_secs_f64(timeout) {
            let _ = child.kill();
            stopped = true;
            break child.wait().ok();
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let (out, err) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
    let code = status.and_then(|status| status.code());
    let mut report = String::new();
    if stopped {
        report.push_str(&format!("stopped after {timeout} seconds\n"));
    }
    report.push_str(&format!("exit code: {}\n", code.map_or("none".to_string(), |code| code.to_string())));
    if !out.is_empty() {
        report.push_str(&format!("--- output\n{}", cut(&out)));
    }
    if !err.is_empty() {
        report.push_str(&format!("--- errors\n{}", cut(&err)));
    }
    if code == Some(0) && !stopped { Ok(report) } else { Err(report) }
}

fn cut(text: &str) -> String {
    if text.len() <= MAX_OUTPUT {
        return text.to_string();
    }
    let mut end = MAX_OUTPUT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… ({} more bytes cut)\n", &text[..end], text.len() - end)
}

/// The list of gaps: `LION_GAPS`, else `docs/manques.md` of the Lion repository this
/// `lion` was built from, else `~/.local/share/lion/manques.md`.
fn gaps_file() -> PathBuf {
    if let Some(path) = std::env::var_os("LION_GAPS") {
        return PathBuf::from(path);
    }
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs");
    if repository.is_dir() {
        return repository.join("manques.md");
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join(".local/share/lion/manques.md")
}

const GAPS_HEADER: &str = "# Ce qui manque à Lion\n\n\
Les assistants qui écrivent du Lion notent ici, par l'outil `lion_report_gap` de `lion mcp` (C108), \
ce que le langage ou sa bibliothèque ne leur a pas permis de faire simplement. \
Chaque entrée est à trier : l'ajouter à Lion (et la rayer ici avec le numéro de la décision), ou expliquer pourquoi non.\n";

fn report_gap(arguments: &Json) -> Result<String, String> {
    let title = text_argument(arguments, "title").ok_or("give `title`")?.trim().replace('\n', " ");
    let need = text_argument(arguments, "need").ok_or("give `need`")?.trim().to_string();
    let path = gaps_file();
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if existing.lines().any(|line| line.starts_with("## ") && line.ends_with(&format!(" — {title}"))) {
        return Ok(format!("`{title}` is already in the list ({}).", path.display()));
    }
    let mut entry = String::new();
    if existing.is_empty() {
        entry.push_str(GAPS_HEADER);
    }
    entry.push_str(&format!("\n## {} — {title}\n\n- **Besoin** : {need}\n", today()));
    if let Some(workaround) = text_argument(arguments, "workaround") {
        entry.push_str(&format!("- **Contournement** : {}\n", workaround.trim()));
    }
    if let Ok(project) = std::env::current_dir() {
        entry.push_str(&format!("- **Projet** : `{}`\n", project.display()));
    }
    if let Some(example) = text_argument(arguments, "example") {
        entry.push_str(&format!("- **Exemple souhaité** :\n\n```lion\n{}\n```\n", example.trim_end()));
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|problem| format!("cannot write {}: {problem}", path.display()))?;
    file.write_all(entry.as_bytes())
        .map_err(|problem| format!("cannot write {}: {problem}", path.display()))?;
    Ok(format!("Noted in {}. Thank you: the authors read this list.", path.display()))
}

/// Today in UTC, `2026-11-16`.
fn today() -> String {
    let days = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() / 86_400) as i64;
    // From the days since 1970-01-01 to the civil date (H. Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}
