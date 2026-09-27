//! `lion lsp`: the language server of editors (Language Server Protocol 3.17), on the
//! standard input and output (C102). It checks the files open in the editor as `lion
//! check` does, without running any code of the program, and gives their diagnostics,
//! their layout by `lion fmt` and their outline.

mod convert;
mod rpc;
mod symbols;

use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc;

use lion_diagnostics::{Diagnostic, Label, SourceMap};
use lion_runtime::json::Json;

use crate::driver::{self, Analysis, Options};
use crate::project::{self, Resolution};
use rpc::{Incoming, code};

pub fn serve() -> ExitCode {
    // The messages are read on a thread of their own, so that those that arrive while a
    // program is checked are handled together: typing fast checks the files once.
    let (sender, messages) = mpsc::channel();
    std::thread::spawn(move || {
        let mut input = io::stdin().lock();
        loop {
            let message = rpc::read(&mut input);
            let end = matches!(message, Incoming::End);
            if sender.send(message).is_err() || end {
                break;
            }
        }
    });
    let mut server = Server::new(io::stdout());
    loop {
        let Ok(first) = messages.recv() else { return ExitCode::from(1) };
        let mut batch = vec![first];
        batch.extend(messages.try_iter());
        for message in batch {
            if let Some(code) = server.handle(message) {
                return code;
            }
        }
        server.check_if_needed();
    }
}

/// A file open in the editor.
struct Document {
    uri: String,
    /// Its path made canonical, or its address when it is not a file.
    key: String,
    /// The name given to the checker: its path, or its address.
    name: String,
    version: Option<i64>,
    text: String,
}

struct Server<W: Write> {
    out: W,
    initialized: bool,
    shutdown: bool,
    /// In the order in which they were opened.
    documents: Vec<Document>,
    /// Whether a change since the last check may change the diagnostics.
    changed: bool,
    /// The packages of each project, resolved once (C101).
    projects: HashMap<PathBuf, Result<Resolution, String>>,
    /// The diagnostics last sent, by address.
    published: HashMap<String, Vec<Json>>,
}

impl<W: Write> Server<W> {
    fn new(out: W) -> Server<W> {
        Server {
            out,
            initialized: false,
            shutdown: false,
            documents: Vec::new(),
            changed: false,
            projects: HashMap::new(),
            published: HashMap::new(),
        }
    }

    fn send(&mut self, message: &Json) {
        if let Err(error) = rpc::write(&mut self.out, message) {
            eprintln!("lion lsp: cannot write to the editor: {error}");
        }
    }

    /// Handles a message; gives the exit code once the editor says to stop.
    fn handle(&mut self, message: Incoming) -> Option<ExitCode> {
        let message = match message {
            Incoming::Message(message) => message,
            Incoming::Invalid(problem) => {
                self.send(&rpc::error(Json::Null, code::PARSE_ERROR, problem));
                return None;
            }
            // The editor stopped without `exit`.
            Incoming::End => return Some(ExitCode::from(1)),
        };
        let params = message.get("params").cloned().unwrap_or(Json::Null);
        let method = message.get("method").and_then(Json::as_text).map(str::to_string);
        match (method, message.get("id").cloned()) {
            (Some(method), Some(id)) => {
                let answer = self.request(&method, &params);
                let reply = match answer {
                    Ok(result) => rpc::response(id, result),
                    Err((code, text)) => rpc::error(id, code, text),
                };
                self.send(&reply);
                None
            }
            (Some(method), None) => self.notification(&method, &params),
            // An answer to a request of the server: it sends none.
            (None, _) => None,
        }
    }

    fn request(&mut self, method: &str, params: &Json) -> Result<Json, (i64, String)> {
        if method == "initialize" {
            if self.initialized {
                return Err((code::INVALID_REQUEST, "the server is already initialized".to_string()));
            }
            self.initialized = true;
            return Ok(capabilities());
        }
        if !self.initialized {
            return Err((code::SERVER_NOT_INITIALIZED, "the server is not initialized yet".to_string()));
        }
        if self.shutdown {
            return Err((code::INVALID_REQUEST, "the server is shutting down".to_string()));
        }
        match method {
            "shutdown" => {
                self.shutdown = true;
                Ok(Json::Null)
            }
            "textDocument/formatting" => Ok(self.format(document_uri(params)?).unwrap_or(Json::Null)),
            "textDocument/documentSymbol" => Ok(self.symbols(document_uri(params)?).unwrap_or(Json::Null)),
            _ => Err((code::METHOD_NOT_FOUND, format!("`{method}` is not supported"))),
        }
    }

    fn notification(&mut self, method: &str, params: &Json) -> Option<ExitCode> {
        match method {
            "exit" => return Some(if self.shutdown { ExitCode::SUCCESS } else { ExitCode::from(1) }),
            // A notification without the members it needs is ignored.
            "textDocument/didOpen" => _ = self.open(params),
            "textDocument/didChange" => _ = self.change(params),
            "textDocument/didClose" => {
                let uri = params.get("textDocument").and_then(|item| item.get("uri")).and_then(Json::as_text);
                self.documents.retain(|document| Some(document.uri.as_str()) != uri);
                self.changed = true;
            }
            // Files changed on the disk: modules, `lion.toml` or `lion.lock`.
            "workspace/didChangeWatchedFiles" => {
                let changes = params.get("changes").and_then(Json::as_list).unwrap_or(&[]);
                let project = changes.iter().any(|change| {
                    change
                        .get("uri")
                        .and_then(Json::as_text)
                        .is_some_and(|uri| uri.ends_with(project::MANIFEST) || uri.ends_with(project::LOCK))
                });
                if project {
                    self.projects.clear();
                }
                self.changed = true;
            }
            _ => {}
        }
        None
    }

    /// `didOpen`: the file is the text of the editor from now on.
    fn open(&mut self, params: &Json) -> Option<()> {
        let item = params.get("textDocument")?;
        let uri = item.get("uri").and_then(Json::as_text)?;
        let text = item.get("text").and_then(Json::as_text)?;
        let version = item.get("version").and_then(Json::as_int);
        self.documents.retain(|document| document.uri != uri);
        self.documents.push(document(uri, text.to_string(), version));
        self.changed = true;
        Some(())
    }

    /// `didChange`: the whole text, or the texts of ranges.
    fn change(&mut self, params: &Json) -> Option<()> {
        let item = params.get("textDocument")?;
        let uri = item.get("uri").and_then(Json::as_text)?;
        let document = self.documents.iter_mut().find(|document| document.uri == uri)?;
        for change in params.get("contentChanges").and_then(Json::as_list).unwrap_or(&[]) {
            apply_change(&mut document.text, change);
        }
        document.version = item.get("version").and_then(Json::as_int);
        self.changed = true;
        Some(())
    }

    fn document(&self, uri: &str) -> Option<&Document> {
        self.documents.iter().find(|document| document.uri == uri)
    }

    /// The edits of `lion fmt` (§24, D20); none for a file with syntax errors, whose
    /// blocks are not known.
    fn format(&self, uri: &str) -> Option<Json> {
        let document = self.document(uri)?;
        let mut sources = SourceMap::new();
        let id = sources.add(document.name.clone(), document.text.clone());
        let file = sources.get(id);
        let lexed = lion_syntax::lex(id, file.text());
        let parsed = lion_syntax::parse(file.text(), &lexed.tokens);
        if lexed.diagnostics.iter().chain(&parsed.diagnostics).any(Diagnostic::is_fatal) {
            return None;
        }
        let formatted = lion_syntax::format(file.text(), &lexed.tokens, &lexed.block_comments);
        if formatted == file.text() {
            return Some(Json::List(Vec::new()));
        }
        let whole = convert::offsets(file, 0, file.text().len() as u32);
        Some(Json::List(vec![Json::object([("range", whole), ("newText", Json::text(formatted))])]))
    }

    fn symbols(&self, uri: &str) -> Option<Json> {
        let document = self.document(uri)?;
        let mut sources = SourceMap::new();
        let id = sources.add(document.name.clone(), document.text.clone());
        let file = sources.get(id);
        let lexed = lion_syntax::lex(id, file.text());
        let parsed = lion_syntax::parse(file.text(), &lexed.tokens);
        Some(symbols::document_symbols(file, &parsed.module))
    }

    /// Checks the programs of the open files again, if something changed, and sends
    /// the diagnostics that differ from those sent before.
    fn check_if_needed(&mut self) {
        if !std::mem::take(&mut self.changed) || !self.initialized {
            return;
        }
        let diagnostics = self.check();
        let mut stale: Vec<String> =
            self.published.keys().filter(|uri| !diagnostics.contains_key(*uri)).cloned().collect();
        stale.sort();
        for uri in stale {
            self.publish(&uri, Vec::new());
        }
        let mut uris: Vec<&String> = diagnostics.keys().collect();
        uris.sort();
        for uri in uris {
            let list = &diagnostics[uri];
            if self.published.get(uri).map_or(!list.is_empty(), |sent| sent != list) {
                self.publish(uri, list.clone());
            }
        }
    }

    fn publish(&mut self, uri: &str, diagnostics: Vec<Json>) {
        let mut params = vec![("uri".to_string(), Json::text(uri))];
        if let Some(version) = self.document(uri).and_then(|document| document.version) {
            params.push(("version".to_string(), Json::int(version)));
        }
        params.push(("diagnostics".to_string(), Json::List(diagnostics.clone())));
        self.send(&rpc::notification("textDocument/publishDiagnostics", Json::Object(params)));
        if diagnostics.is_empty() {
            self.published.remove(uri);
        } else {
            self.published.insert(uri.to_string(), diagnostics);
        }
    }

    /// The diagnostics of every file of the programs of the open files, by address.
    ///
    /// Each open file is the script of a program, unless an earlier program already
    /// reaches it: the `main.lion` of its project, then the files in the order they were
    /// opened. A module is thus checked with the program that uses it, and its `use`
    /// are read from the folder of that program (C61).
    fn check(&mut self) -> HashMap<String, Vec<Json>> {
        let mut scripts: Vec<(String, String)> = Vec::new();
        for document in &self.documents {
            let Some(folder) = uri_folder(&document.uri) else { continue };
            let Some(root) = project::find_root(&folder) else { continue };
            let main = root.join("main.lion");
            if main.is_file() {
                let key = canonical(&main);
                if !scripts.iter().any(|(known, _)| *known == key) {
                    scripts.push((key, main.display().to_string()));
                }
            }
        }
        for document in &self.documents {
            if !scripts.iter().any(|(known, _)| *known == document.key) {
                scripts.push((document.key.clone(), document.name.clone()));
            }
        }
        let mut reached: HashSet<String> = HashSet::new();
        let mut diagnostics: HashMap<String, Vec<Json>> = HashMap::new();
        for (key, name) in scripts {
            if reached.contains(&key) {
                continue;
            }
            for (uri, file_key, list) in self.check_program(&key, &name) {
                if reached.insert(file_key) {
                    diagnostics.insert(uri, list);
                }
            }
        }
        diagnostics
    }

    /// Checks the program whose script is `name`: for each file that is not of the
    /// standard library, its address, its key and its diagnostics.
    fn check_program(&mut self, key: &str, name: &str) -> Vec<(String, String, Vec<Json>)> {
        let text = match self.documents.iter().find(|document| document.key == key) {
            Some(document) => document.text.clone(),
            None => match std::fs::read_to_string(name) {
                Ok(text) => text,
                Err(_) => return Vec::new(),
            },
        };
        let mut sources = SourceMap::new();
        let id = sources.add(name, text);
        let folder = Path::new(name).parent().map(Path::to_path_buf).unwrap_or_default();
        let packages = self.packages(&folder);
        let open: HashMap<String, String> =
            self.documents.iter().map(|document| (document.key.clone(), document.text.clone())).collect();
        let read = |path: &Path| open.get(&canonical(path)).cloned().or_else(|| driver::read_file(path));
        let checked = catch_unwind(AssertUnwindSafe(|| {
            let (packages, problem) = match &packages {
                Ok(packages) => (packages.clone(), None),
                Err(problem) => (Resolution::default(), Some(Diagnostic::error(problem.clone()))),
            };
            let mut analysis =
                driver::analyze(&mut sources, id, &packages, &Options { read: &read, evaluate: false });
            analysis.diagnostics.extend(problem);
            analysis
        }));
        let analysis = match checked {
            Ok(analysis) => analysis,
            Err(payload) => {
                let text = payload
                    .downcast_ref::<&str>()
                    .map(|text| text.to_string())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_default();
                let problem = Diagnostic::internal(format!("the checker stopped on this file: {text}"));
                let file = sources.get(id);
                let uri = self.uri_of(key);
                let list = vec![convert::diagnostic(&problem, file, None, &|_| None)];
                return uri.map(|uri| vec![(uri, key.to_string(), list)]).unwrap_or_default();
            }
        };
        self.file_diagnostics(&sources, &analysis)
    }

    /// The diagnostics of an analysis, file by file. A module with errors is reported at
    /// the `use` that reaches it, so the script shows that it cannot run.
    fn file_diagnostics(&self, sources: &SourceMap, analysis: &Analysis) -> Vec<(String, String, Vec<Json>)> {
        let files = &analysis.files;
        let user = |source: lion_diagnostics::SourceId| {
            files.iter().position(|file| file.source == source && !file.standard)
        };
        let uris: Vec<Option<String>> =
            files.iter().map(|file| if file.standard { None } else { self.uri_of(&file.key) }).collect();
        let locate = |span: lion_diagnostics::Span| {
            let index = user(span.source)?;
            Some((sources.get(span.source), uris[index].clone()?))
        };
        let mut lists: Vec<Vec<Json>> = vec![Vec::new(); files.len()];
        let mut errors = vec![0usize; files.len()];
        for diagnostic in &analysis.diagnostics {
            // The primary label, or the first one in a file of the program; else the
            // start of the script.
            let primary = diagnostic.labels.iter().find(|label| label.primary);
            let label: Option<&Label> = primary
                .filter(|label| user(label.span.source).is_some())
                .or_else(|| diagnostic.labels.iter().find(|label| user(label.span.source).is_some()));
            let index = label.and_then(|label| user(label.span.source)).unwrap_or(0);
            let file = sources.get(files[index].source);
            lists[index].push(convert::diagnostic(diagnostic, file, label, &locate));
            if diagnostic.is_fatal() {
                errors[index] += 1;
            }
        }
        // The modules are reached after the files that use them.
        for index in (1..files.len()).rev() {
            let Some((user_index, span)) = files[index].used_by else { continue };
            if errors[index] == 0 || files[index].standard {
                continue;
            }
            let file = sources.get(files[user_index].source);
            let name = file.text().get(span.start as usize..span.end as usize).unwrap_or("");
            let plural = if errors[index] == 1 { "" } else { "s" };
            let problem =
                Diagnostic::error(format!("the module `{name}` has {} error{plural}", errors[index]))
                    .with_primary(span, "")
                    .with_note(format!("see {}", sources.get(files[index].source).name()));
            lists[user_index].push(convert::diagnostic(&problem, file, problem.labels.first(), &locate));
            errors[user_index] += 1;
        }
        files
            .iter()
            .zip(lists)
            .zip(uris)
            .filter_map(|((file, list), uri)| Some((uri?, file.key.clone(), list)))
            .collect()
    }

    /// The packages that a script of `folder` may use, resolved once per project.
    fn packages(&mut self, folder: &Path) -> Result<Resolution, String> {
        let Some(root) = project::find_root(folder) else { return Ok(Resolution::default()) };
        self.projects.entry(root.clone()).or_insert_with(|| project::resolve(&root, None)).clone()
    }

    /// The address of a file, by its key: that of the editor when it is open.
    fn uri_of(&self, key: &str) -> Option<String> {
        if let Some(document) = self.documents.iter().find(|document| document.key == key) {
            return Some(document.uri.clone());
        }
        Path::new(key).is_absolute().then(|| convert::path_to_uri(Path::new(key)))
    }
}

/// What the server can do, for `initialize`.
fn capabilities() -> Json {
    let sync = Json::object([
        ("openClose", Json::Bool(true)),
        // The whole text at each change.
        ("change", Json::int(1)),
    ]);
    Json::object([
        (
            "capabilities",
            Json::object([
                ("positionEncoding", Json::text("utf-16")),
                ("textDocumentSync", sync),
                ("documentFormattingProvider", Json::Bool(true)),
                ("documentSymbolProvider", Json::Bool(true)),
            ]),
        ),
        (
            "serverInfo",
            Json::object([("name", Json::text("lion")), ("version", Json::text(env!("CARGO_PKG_VERSION")))]),
        ),
    ])
}

fn document(uri: &str, text: String, version: Option<i64>) -> Document {
    let (key, name) = match convert::uri_to_path(uri) {
        Some(path) => (canonical(&path), path.display().to_string()),
        None => (uri.to_string(), uri.to_string()),
    };
    Document { uri: uri.to_string(), key, name, version, text }
}

/// Applies a change of `didChange`: the whole text, or the text of a range.
fn apply_change(text: &mut String, change: &Json) {
    let Some(new) = change.get("text").and_then(Json::as_text) else { return };
    let Some(range) = change.get("range") else {
        *text = new.to_string();
        return;
    };
    let mut sources = SourceMap::new();
    let id = sources.add("", std::mem::take(text));
    let file = sources.get(id);
    let start = range.get("start").and_then(|position| convert::offset(file, position));
    let end = range.get("end").and_then(|position| convert::offset(file, position));
    let mut updated = file.text().to_string();
    if let (Some(start), Some(end)) = (start, end)
        && start <= end
    {
        updated.replace_range(start as usize..end as usize, new);
    }
    *text = updated;
}

/// The address of the document of a request.
fn document_uri(params: &Json) -> Result<&str, (i64, String)> {
    params
        .get("textDocument")
        .and_then(|item| item.get("uri"))
        .and_then(Json::as_text)
        .ok_or((code::INVALID_PARAMS, "the request has no `textDocument.uri`".to_string()))
}

/// The folder of a file given by its address.
fn uri_folder(uri: &str) -> Option<PathBuf> {
    convert::uri_to_path(uri)?.parent().map(Path::to_path_buf)
}

/// The key of a file: its path made canonical, when it exists.
fn canonical(path: &Path) -> String {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()).display().to_string()
}

#[cfg(test)]
mod tests;
