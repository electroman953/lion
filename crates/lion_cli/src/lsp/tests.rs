//! The server, given messages in memory: what it answers and which diagnostics it sends.

use lion_runtime::json::{self, Json};

use super::rpc::Incoming;
use super::*;

/// A server that was initialized, with what it sent taken.
fn server() -> Server<Vec<u8>> {
    let mut server = Server::new(Vec::new());
    handle(&mut server, r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#);
    handle(&mut server, r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);
    sent(&mut server);
    server
}

fn handle(server: &mut Server<Vec<u8>>, message: &str) -> Option<ExitCode> {
    server.handle(Incoming::Message(json::parse(message).expect("a JSON message")))
}

/// The messages sent since the last call.
fn sent(server: &mut Server<Vec<u8>>) -> Vec<Json> {
    let output = std::mem::take(&mut server.out);
    let mut input = &output[..];
    let mut messages = Vec::new();
    while let Incoming::Message(message) = rpc::read(&mut input) {
        messages.push(message);
    }
    messages
}

fn open(server: &mut Server<Vec<u8>>, uri: &str, text: &str) {
    let item = Json::object([
        ("uri", Json::text(uri)),
        ("languageId", Json::text("lion")),
        ("version", Json::int(1)),
        ("text", Json::text(text)),
    ]);
    let message = rpc::notification("textDocument/didOpen", Json::object([("textDocument", item)]));
    handle(server, &message.to_string());
}

fn change(server: &mut Server<Vec<u8>>, uri: &str, version: i64, change: Json) {
    let item = Json::object([("uri", Json::text(uri)), ("version", Json::int(version))]);
    let params = Json::object([("textDocument", item), ("contentChanges", Json::List(vec![change]))]);
    handle(server, &rpc::notification("textDocument/didChange", params).to_string());
}

fn request(server: &mut Server<Vec<u8>>, method: &str, uri: &str) -> Json {
    ask(server, method, Json::object([("textDocument", Json::object([("uri", Json::text(uri))]))]))
}

fn ask(server: &mut Server<Vec<u8>>, method: &str, params: Json) -> Json {
    let message = Json::object([
        ("jsonrpc", Json::text("2.0")),
        ("id", Json::int(7)),
        ("method", Json::text(method)),
        ("params", params),
    ]);
    handle(server, &message.to_string());
    let mut answers = sent(server);
    // A request that checks the files first publishes their diagnostics before it
    // answers; the answer is the message that carries the id of the request.
    answers.retain(|answer| answer.get("id").is_some());
    assert_eq!(answers.len(), 1);
    answers.remove(0)
}

/// The parameters of a request about the name at a place of a file.
fn at(uri: &str, line: i64, character: i64) -> Json {
    Json::object([
        ("textDocument", Json::object([("uri", Json::text(uri))])),
        ("position", Json::object([("line", Json::int(line)), ("character", Json::int(character))])),
    ])
}

/// Checks the open files, and drops the diagnostics: the requests come next.
fn check(server: &mut Server<Vec<u8>>) {
    server.check_if_needed();
    sent(server);
}

/// What a hover shows, without its fences.
fn hovered(answer: &Json) -> String {
    let value = answer
        .get("result")
        .and_then(|result| result.get("contents"))
        .and_then(|contents| contents.get("value"))
        .and_then(Json::as_text)
        .unwrap_or_else(|| panic!("a hover, not {answer}"));
    value.trim_start_matches("```lion\n").trim_end_matches("```").trim().to_string()
}

/// A location as `(address, line, character)` of where it starts.
fn place(location: &Json) -> (String, i64, i64) {
    let uri = location.get("uri").and_then(Json::as_text).unwrap().to_string();
    let start = location.get("range").and_then(|range| range.get("start")).unwrap();
    let line = start.get("line").and_then(Json::as_int).unwrap();
    let character = start.get("character").and_then(Json::as_int).unwrap();
    (uri, line, character)
}

/// Where each answer of a list of locations or of highlights starts.
fn places(answer: &Json, uri: &str) -> Vec<(String, i64, i64)> {
    answer
        .get("result")
        .and_then(Json::as_list)
        .unwrap_or_else(|| panic!("a list, not {answer}"))
        .iter()
        .map(|item| match item.get("uri") {
            Some(_) => place(item),
            // A highlight has a range alone: it is always in the file being read.
            None => {
                let with =
                    Json::object([("uri", Json::text(uri)), ("range", item.get("range").unwrap().clone())]);
                place(&with)
            }
        })
        .collect()
}

/// A diagnostic as `(line, character, severity, message)`.
type Found = (i64, i64, i64, String);

/// The diagnostics sent after a check, by address.
fn diagnostics(server: &mut Server<Vec<u8>>) -> Vec<(String, Vec<Found>)> {
    server.check_if_needed();
    sent(server)
        .into_iter()
        .map(|message| {
            assert_eq!(
                message.get("method").and_then(Json::as_text),
                Some("textDocument/publishDiagnostics")
            );
            let params = message.get("params").unwrap();
            let uri = params.get("uri").and_then(Json::as_text).unwrap().to_string();
            let list = params
                .get("diagnostics")
                .and_then(Json::as_list)
                .unwrap()
                .iter()
                .map(|diagnostic| {
                    let start = diagnostic.get("range").and_then(|range| range.get("start")).unwrap();
                    (
                        start.get("line").and_then(Json::as_int).unwrap(),
                        start.get("character").and_then(Json::as_int).unwrap(),
                        diagnostic.get("severity").and_then(Json::as_int).unwrap(),
                        diagnostic.get("message").and_then(Json::as_text).unwrap().to_string(),
                    )
                })
                .collect();
            (uri, list)
        })
        .collect()
}

const MAIN: &str = "file:///nowhere/lion-lsp/main.lion";
const GEO: &str = "file:///nowhere/lion-lsp/geo.lion";

#[test]
fn the_server_starts_and_stops_as_the_protocol_says() {
    let mut server = Server::new(Vec::new());
    handle(&mut server, r#"{"jsonrpc":"2.0","id":1,"method":"shutdown"}"#);
    let answer = &sent(&mut server)[0];
    assert_eq!(answer.get("error").and_then(|e| e.get("code")).and_then(Json::as_int), Some(-32002));
    handle(&mut server, r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"capabilities":{}}}"#);
    let answer = &sent(&mut server)[0];
    let capabilities = answer.get("result").and_then(|result| result.get("capabilities")).unwrap();
    assert_eq!(capabilities.get("positionEncoding").and_then(Json::as_text), Some("utf-16"));
    assert_eq!(capabilities.get("documentFormattingProvider"), Some(&Json::Bool(true)));
    assert_eq!(capabilities.get("hoverProvider"), Some(&Json::Bool(true)));
    assert_eq!(capabilities.get("definitionProvider"), Some(&Json::Bool(true)));
    assert_eq!(capabilities.get("referencesProvider"), Some(&Json::Bool(true)));
    handle(&mut server, r#"{"jsonrpc":"2.0","id":3,"method":"textDocument/completion","params":{}}"#);
    let answer = &sent(&mut server)[0];
    assert_eq!(answer.get("error").and_then(|e| e.get("code")).and_then(Json::as_int), Some(-32601));
    server.handle(Incoming::Invalid("invalid JSON".to_string()));
    let answer = &sent(&mut server)[0];
    assert_eq!(answer.get("id"), Some(&Json::Null));
    assert!(handle(&mut server, r#"{"jsonrpc":"2.0","id":4,"method":"shutdown"}"#).is_none());
    assert_eq!(sent(&mut server)[0].get("result"), Some(&Json::Null));
    assert_eq!(handle(&mut server, r#"{"jsonrpc":"2.0","method":"exit"}"#), Some(ExitCode::SUCCESS));
    // Without `shutdown` first, the exit code says that something went wrong.
    let mut other = self::server();
    assert_eq!(handle(&mut other, r#"{"jsonrpc":"2.0","method":"exit"}"#), Some(ExitCode::from(1)));
}

#[test]
fn diagnostics_follow_the_changes_of_the_text() {
    let mut server = server();
    open(&mut server, MAIN, "let x = 3 + \"é\"\nshow(x)\n");
    let found = diagnostics(&mut server);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].0, MAIN);
    let (line, character, severity, message) = &found[0].1[0];
    assert_eq!((*line, *character, *severity), (0, 10, 1));
    assert!(message.starts_with("`+` cannot"), "{message}");
    // Nothing changed: nothing is sent again.
    server.changed = true;
    assert!(diagnostics(&mut server).is_empty());
    // The text of a range, in UTF-16 units: `3 + "é"` becomes `3 + 4`.
    let range =
        json::parse(r#"{"start":{"line":0,"character":12},"end":{"line":0,"character":15}}"#).unwrap();
    change(&mut server, MAIN, 2, Json::object([("range", range), ("text", Json::text("4"))]));
    assert_eq!(server.documents[0].text, "let x = 3 + 4\nshow(x)\n");
    assert_eq!(diagnostics(&mut server), vec![(MAIN.to_string(), Vec::new())]);
    // A syntax error, then the whole text again.
    change(&mut server, MAIN, 3, Json::object([("text", Json::text("if true:\n    show(1)\n"))]));
    let found = diagnostics(&mut server);
    assert_eq!(found[0].1.len(), 1);
    assert!(found[0].1[0].3.contains(';'), "{:?}", found[0].1);
    // Closing the file forgets its diagnostics.
    let close = rpc::notification(
        "textDocument/didClose",
        Json::object([("textDocument", Json::object([("uri", Json::text(MAIN))]))]),
    );
    handle(&mut server, &close.to_string());
    assert_eq!(diagnostics(&mut server), vec![(MAIN.to_string(), Vec::new())]);
}

#[test]
fn a_module_is_checked_with_the_program_that_uses_it() {
    let mut server = server();
    // The module is opened first; the script that uses it, then.
    open(&mut server, GEO, "fun area(r in Float) in Float = r * r\nlet oops in Int = \"x\"\n");
    open(&mut server, MAIN, "use geo\nshow(geo.area(2.0))\n");
    let mut found = diagnostics(&mut server);
    found.sort();
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].0, GEO);
    assert_eq!(found[0].1.len(), 1);
    assert_eq!(found[0].1[0].0, 1);
    assert_eq!(found[1].0, MAIN);
    assert_eq!(
        found[1].1,
        vec![(0, 4, 1, "the module `geo` has 1 error\nsee /nowhere/lion-lsp/geo.lion".to_string())]
    );
    // Once the module is right, both files are.
    change(
        &mut server,
        GEO,
        2,
        Json::object([("text", Json::text("fun area(r in Float) in Float = r * r\n"))]),
    );
    let mut found = diagnostics(&mut server);
    found.sort();
    assert_eq!(found, vec![(GEO.to_string(), Vec::new()), (MAIN.to_string(), Vec::new())]);
    // A module that the script cannot find.
    change(&mut server, MAIN, 3, Json::object([("text", Json::text("use geometry\n"))]));
    let found = diagnostics(&mut server);
    assert_eq!(found.len(), 1);
    assert!(found[0].1[0].3.starts_with("cannot find the module `geometry`"), "{:?}", found);
}

#[test]
fn the_server_does_not_run_compile() {
    let mut server = server();
    // Computed by `lion check`, this would be a bug at compile time (§21.1).
    open(&mut server, MAIN, "fun f(n in Int) in Int = 10 div n\nlet x = compile f(0)\nshow(x)\n");
    assert_eq!(diagnostics(&mut server), Vec::new());
}

#[test]
fn formatting_lays_the_file_out_as_lion_fmt() {
    let mut server = server();
    open(&mut server, MAIN, "if true:\nshow(1)\n;\n");
    let answer = request(&mut server, "textDocument/formatting", MAIN);
    let edits = answer.get("result").and_then(Json::as_list).unwrap();
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].get("newText").and_then(Json::as_text), Some("if true:\n    show(1)\n;\n"));
    let end = edits[0].get("range").and_then(|range| range.get("end")).unwrap();
    assert_eq!(
        (end.get("line").and_then(Json::as_int), end.get("character").and_then(Json::as_int)),
        (Some(3), Some(0))
    );
    // Already formatted: no edit. With a syntax error: nothing.
    change(&mut server, MAIN, 2, Json::object([("text", Json::text("show(1)\n"))]));
    let answer = request(&mut server, "textDocument/formatting", MAIN);
    assert_eq!(answer.get("result"), Some(&Json::List(Vec::new())));
    change(&mut server, MAIN, 3, Json::object([("text", Json::text("if true:\nshow(1)\n"))]));
    let answer = request(&mut server, "textDocument/formatting", MAIN);
    assert_eq!(answer.get("result"), Some(&Json::Null));
}

#[test]
fn the_outline_lists_the_declarations() {
    let mut server = server();
    let text = "\
use math
let limit = 10
var count = 0 in Int
struct Student:
    name in Text
    grade in Int = 0, grade >= 0
;
fun Student.passes() in Bool = self.grade >= 10
fun add(a in Int, var b, c = 1) in Int:
    return a + b + c
;
Color = {red, green}
Shape = Circle or Square
trait Named:
    name in Text
    fun label() in Text
;
test \"addition\":
    expect add(1, 2) == 4
;
";
    open(&mut server, MAIN, text);
    let answer = request(&mut server, "textDocument/documentSymbol", MAIN);
    let symbols = answer.get("result").and_then(Json::as_list).unwrap();
    let describe = |symbol: &Json| {
        let name = symbol.get("name").and_then(Json::as_text).unwrap();
        let kind = symbol.get("kind").and_then(Json::as_int).unwrap();
        let detail = symbol.get("detail").and_then(Json::as_text).unwrap_or("");
        let children = symbol.get("children").and_then(Json::as_list).map_or(0, <[Json]>::len);
        format!("{name} {kind} {detail} {children}")
    };
    let found: Vec<String> = symbols.iter().map(describe).collect();
    assert_eq!(
        found,
        [
            "limit 14  0",
            "count 13 in Int 0",
            "Student 23  2",
            "Student.passes 6 () in Bool 0",
            "add 12 (a in Int, var b, c = 1) in Int 0",
            "Color 10  2",
            "Shape 10 Circle or Square 0",
            "Named 11  2",
            "test \"addition\" 12  0",
        ]
    );
    let student = &symbols[2];
    let fields: Vec<String> =
        student.get("children").and_then(Json::as_list).unwrap().iter().map(describe).collect();
    assert_eq!(fields, ["name 8 in Text 0", "grade 8 in Int 0"]);
    // The range of a block goes to its `;`.
    let end = student.get("range").and_then(|range| range.get("end")).unwrap();
    assert_eq!(end.get("line").and_then(Json::as_int), Some(6));
}

/// The text whose names the navigation tests ask about.
const NAMES: &str = "\
let total = 1 + 2
show(total)
fun double(n in Int) in Int = n * 2
show(double(total))
";

#[test]
fn hover_gives_the_declaration_of_a_name() {
    let mut server = server();
    open(&mut server, MAIN, NAMES);
    check(&mut server);
    // A variable, at its use and at its declaration.
    assert_eq!(hovered(&ask(&mut server, "textDocument/hover", at(MAIN, 1, 5))), "let total in Int");
    assert_eq!(hovered(&ask(&mut server, "textDocument/hover", at(MAIN, 0, 4))), "let total in Int");
    // A function and its parameter.
    assert_eq!(
        hovered(&ask(&mut server, "textDocument/hover", at(MAIN, 3, 5))),
        "fun double(n in Int) in Int"
    );
    assert_eq!(hovered(&ask(&mut server, "textDocument/hover", at(MAIN, 2, 30))), "n in Int");
    // The range that the editor underlines is the name alone.
    let answer = ask(&mut server, "textDocument/hover", at(MAIN, 1, 5));
    let range = answer.get("result").and_then(|result| result.get("range")).unwrap();
    assert_eq!(range.to_string(), r#"{"start":{"line":1,"character":5},"end":{"line":1,"character":10}}"#);
    // `let` is not a name; a request without a position is refused.
    assert_eq!(ask(&mut server, "textDocument/hover", at(MAIN, 0, 0)).get("result"), Some(&Json::Null));
    let params = Json::object([("textDocument", Json::object([("uri", Json::text(MAIN))]))]);
    let answer = ask(&mut server, "textDocument/hover", params);
    assert_eq!(answer.get("error").and_then(|e| e.get("code")).and_then(Json::as_int), Some(-32602));
}

#[test]
fn going_to_a_declaration_and_finding_its_uses() {
    let mut server = server();
    open(&mut server, MAIN, NAMES);
    check(&mut server);
    let answer = ask(&mut server, "textDocument/definition", at(MAIN, 3, 12));
    assert_eq!(place(answer.get("result").unwrap()), (MAIN.to_string(), 0, 4));
    let answer = ask(&mut server, "textDocument/definition", at(MAIN, 3, 5));
    assert_eq!(place(answer.get("result").unwrap()), (MAIN.to_string(), 2, 4));
    // Every use of `total`, its declaration first.
    let answer = ask(&mut server, "textDocument/references", at(MAIN, 1, 5));
    assert_eq!(
        places(&answer, MAIN),
        [(MAIN.to_string(), 0, 4), (MAIN.to_string(), 1, 5), (MAIN.to_string(), 3, 12)]
    );
    // Without the declaration, when the editor asks for the uses alone.
    let mut params = at(MAIN, 1, 5);
    let Json::Object(members) = &mut params else { unreachable!() };
    members.push(("context".to_string(), Json::object([("includeDeclaration", Json::Bool(false))])));
    let answer = ask(&mut server, "textDocument/references", params);
    assert_eq!(places(&answer, MAIN), [(MAIN.to_string(), 1, 5), (MAIN.to_string(), 3, 12)]);
    // The same name, highlighted in the file being read.
    let answer = ask(&mut server, "textDocument/documentHighlight", at(MAIN, 2, 11));
    assert_eq!(places(&answer, MAIN), [(MAIN.to_string(), 2, 11), (MAIN.to_string(), 2, 30)]);
}

#[test]
fn a_declaration_of_another_file_is_reached() {
    let mut server = server();
    open(&mut server, GEO, "fun area(r in Float) in Float = r * r\n");
    open(&mut server, MAIN, "use geo\nshow(geo.area(2.0))\n");
    check(&mut server);
    let answer = ask(&mut server, "textDocument/definition", at(MAIN, 1, 9));
    assert_eq!(place(answer.get("result").unwrap()), (GEO.to_string(), 0, 4));
    assert_eq!(
        hovered(&ask(&mut server, "textDocument/hover", at(MAIN, 1, 9))),
        "fun area(r in Float) in Float"
    );
    // The uses of the function, in both files.
    let answer = ask(&mut server, "textDocument/references", at(GEO, 0, 4));
    assert_eq!(places(&answer, GEO), [(GEO.to_string(), 0, 4), (MAIN.to_string(), 1, 9)]);
    // A declaration of the standard library is shown, but the editor opens no file.
    open(&mut server, MAIN, "use text\nshow(text.upper(\"a\"))\n");
    check(&mut server);
    let hover = hovered(&ask(&mut server, "textDocument/hover", at(MAIN, 1, 10)));
    assert!(hover.starts_with("fun upper(") && hover.contains("standard library"), "{hover}");
    assert_eq!(ask(&mut server, "textDocument/definition", at(MAIN, 1, 10)).get("result"), Some(&Json::Null));
}

#[test]
fn the_names_of_the_last_text_that_parsed_answer_while_typing() {
    let mut server = server();
    open(&mut server, MAIN, NAMES);
    check(&mut server);
    // A half-typed line: the checker does not run on a file with a syntax error.
    let half = format!("{NAMES}show(double(\n");
    change(&mut server, MAIN, 2, Json::object([("text", Json::text(half))]));
    check(&mut server);
    assert_eq!(hovered(&ask(&mut server, "textDocument/hover", at(MAIN, 1, 5))), "let total in Int");
    assert_eq!(
        hovered(&ask(&mut server, "textDocument/hover", at(MAIN, 3, 5))),
        "fun double(n in Int) in Int"
    );
    // Once the file parses again, the names follow the new text.
    let changed = NAMES.replace("1 + 2", "\"a\"");
    change(&mut server, MAIN, 3, Json::object([("text", Json::text(changed))]));
    check(&mut server);
    assert_eq!(hovered(&ask(&mut server, "textDocument/hover", at(MAIN, 1, 5))), "let total in Text");
    // A file the server never checked knows nothing.
    assert_eq!(ask(&mut server, "textDocument/hover", at(GEO, 0, 0)).get("result"), Some(&Json::Null));
}

#[test]
fn a_request_that_comes_with_the_text_is_answered_all_the_same() {
    let mut server = server();
    // Nothing checked the file: the editor sent the text and the request together, and
    // the server reads them in one batch.
    open(&mut server, MAIN, NAMES);
    assert_eq!(hovered(&ask(&mut server, "textDocument/hover", at(MAIN, 1, 5))), "let total in Int");
    // The same, right after a change.
    let changed = NAMES.replace("1 + 2", "\"a\"");
    change(&mut server, MAIN, 2, Json::object([("text", Json::text(changed))]));
    assert_eq!(hovered(&ask(&mut server, "textDocument/hover", at(MAIN, 1, 5))), "let total in Text");
}
