//! `lion lsp`, the language server (C102), run as an editor runs it: messages on its
//! standard input, answers and diagnostics on its standard output. The files of a
//! project are on the disk, and only some of them are open.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// `lion lsp`, with its messages read on a thread.
struct Editor {
    child: Child,
    /// Dropped when the editor goes away.
    input: Option<ChildStdin>,
    messages: mpsc::Receiver<String>,
    next_id: u32,
}

impl Editor {
    fn start(folder: &Path) -> Editor {
        let mut child = Command::new(env!("CARGO_BIN_EXE_lion"))
            .arg("lsp")
            .current_dir(folder)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("lion lsp starts");
        let input = child.stdin.take().unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let (sender, messages) = mpsc::channel();
        std::thread::spawn(move || {
            loop {
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    if output.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    if let Some(value) = line.strip_prefix("Content-Length:") {
                        length = value.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; length];
                output.read_exact(&mut body).unwrap();
                if sender.send(String::from_utf8(body).unwrap()).is_err() {
                    return;
                }
            }
        });
        let mut editor = Editor { child, input: Some(input), messages, next_id: 1 };
        let answer = editor.request("initialize", r#"{"processId":null,"rootUri":null,"capabilities":{}}"#);
        assert!(answer.contains(r#""documentFormattingProvider":true"#), "{answer}");
        editor.notify("initialized", "{}");
        editor
    }

    fn send(&mut self, message: &str) {
        let input = self.input.as_mut().expect("the editor is there");
        write!(input, "Content-Length: {}\r\n\r\n{message}", message.len()).unwrap();
        input.flush().unwrap();
    }

    fn notify(&mut self, method: &str, params: &str) {
        self.send(&format!(r#"{{"jsonrpc":"2.0","method":"{method}","params":{params}}}"#));
    }

    /// Sends a request and gives its answer; the diagnostics met on the way are dropped.
    fn request(&mut self, method: &str, params: &str) -> String {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}","params":{params}}}"#));
        let wanted = format!(r#""id":{id},"#);
        loop {
            let message = self.next();
            if message.contains(&wanted) {
                return message;
            }
        }
    }

    fn next(&self) -> String {
        self.messages.recv_timeout(Duration::from_secs(20)).expect("the server answers in time")
    }

    /// The next diagnostics sent for each of the files `uris`, in any order.
    fn diagnostics<const N: usize>(&self, uris: [&str; N]) -> [String; N] {
        let mut found: [String; N] = std::array::from_fn(|_| String::new());
        while found.iter().any(String::is_empty) {
            let message = self.next();
            if !message.contains("textDocument/publishDiagnostics") {
                continue;
            }
            for (index, uri) in uris.iter().enumerate() {
                if found[index].is_empty() && message.contains(&format!(r#""uri":"{uri}""#)) {
                    found[index] = message.clone();
                }
            }
        }
        found
    }

    fn open(&mut self, uri: &str, text: &str) {
        let text = text.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
        let params = format!(
            r#"{{"textDocument":{{"uri":"{uri}","languageId":"lion","version":1,"text":"{text}"}}}}"#
        );
        self.notify("textDocument/didOpen", &params);
    }

    fn stop(mut self) {
        let answer = self.request("shutdown", "null");
        assert!(answer.contains(r#""result":null"#), "{answer}");
        self.notify("exit", "null");
        let status = self.child.wait().unwrap();
        assert!(status.success(), "{status}");
    }
}

fn uri(path: &Path) -> String {
    format!("file://{}", path.display())
}

fn project(name: &str) -> PathBuf {
    let folder = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&folder);
    fs::create_dir_all(folder.join("shapes")).unwrap();
    let folder = fs::canonicalize(folder).unwrap();
    fs::write(
        folder.join("lion.toml"),
        "[project]\nname = \"carnet\"\nversion = \"0.1.0\"\nedition = \"0.1\"\n\n[dependencies]\n",
    )
    .unwrap();
    fs::write(folder.join("main.lion"), "use shapes.circle\nshow(circle.area(1.0))\n").unwrap();
    // `use units` in a module reaches the `units.lion` of the folder of the script (C61).
    fs::write(
        folder.join("shapes/circle.lion"),
        "use units\nfun area(r in Float) in Float = units.scale * r * r\n",
    )
    .unwrap();
    fs::write(folder.join("units.lion"), "let scale = \"3\"\n").unwrap();
    folder
}

#[test]
fn a_module_open_alone_is_checked_with_the_main_of_its_project() {
    let folder = project("lsp-project");
    let mut editor = Editor::start(&folder);
    let circle = uri(&folder.join("shapes/circle.lion"));
    let main = uri(&folder.join("main.lion"));
    editor.open(&circle, &fs::read_to_string(folder.join("shapes/circle.lion")).unwrap());
    // `units.scale` is a Text: an error in `circle.lion`, the file open, found by
    // checking `main.lion`, which is not open.
    let [in_circle, in_main] = editor.diagnostics([&circle, &main]);
    assert!(in_circle.contains(r#""message":"`*` cannot be applied to Text and Float"#), "{in_circle}");
    assert!(in_circle.contains(r#""range":{"start":{"line":1,"character":44}"#), "{in_circle}");
    assert!(in_main.contains("the module `shapes.circle` has 1 error"), "{in_main}");
    // The file changes on the disk.
    fs::write(folder.join("units.lion"), "let scale = 3.0\n").unwrap();
    let change = format!(r#"{{"changes":[{{"uri":"{}","type":2}}]}}"#, uri(&folder.join("units.lion")));
    editor.notify("workspace/didChangeWatchedFiles", &change);
    for found in editor.diagnostics([&circle, &main]) {
        assert!(found.contains(r#""diagnostics":[]"#), "{found}");
    }
    // The text of the editor wins over the file.
    editor.open(&uri(&folder.join("units.lion")), "let scale = true\n");
    let [in_circle] = editor.diagnostics([&circle]);
    assert!(in_circle.contains("cannot be applied to Bool and Float"), "{in_circle}");
    let symbols =
        editor.request("textDocument/documentSymbol", &format!(r#"{{"textDocument":{{"uri":"{circle}"}}}}"#));
    assert!(symbols.contains(r#""name":"area","detail":"(r in Float) in Float","kind":12"#), "{symbols}");
    editor.stop();
}

#[test]
fn the_server_stops_when_the_editor_goes_away() {
    let folder = project("lsp-gone");
    let mut editor = Editor::start(&folder);
    // An editor that stops closes the input of the server.
    editor.input = None;
    let status = editor.child.wait().unwrap();
    assert_eq!(status.code(), Some(1));
}
