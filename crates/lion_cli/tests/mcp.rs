//! `lion mcp` spoken to as an AI assistant does (C108): one JSON-RPC message per line.

use std::io::Write;
use std::process::{Command, Stdio};

use lion_runtime::json::{self, Json};

/// The answers of the server to these messages, by their `id`.
fn session(messages: &[&str], gaps: &std::path::Path) -> Vec<Json> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_lion"))
        .arg("mcp")
        .env("LION_GAPS", gaps)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("the lion binary runs");
    let mut input = child.stdin.take().unwrap();
    for message in messages {
        writeln!(input, "{message}").unwrap();
    }
    drop(input);
    let output = child.wait_with_output().unwrap();
    String::from_utf8(output.stdout).unwrap().lines().map(|line| json::parse(line).unwrap()).collect()
}

/// The text of the answer to a tool, and whether it is an error.
fn tool_text(answer: &Json) -> (String, bool) {
    let result = answer.get("result").unwrap();
    let text = result.get("content").unwrap().as_list().unwrap()[0].get("text").unwrap().as_text().unwrap();
    (text.to_string(), result.get("isError") == Some(&Json::Bool(true)))
}

#[test]
fn an_assistant_checks_runs_and_reports() {
    let folder = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("mcp");
    std::fs::create_dir_all(&folder).unwrap();
    let gaps = folder.join("manques.md");
    let _ = std::fs::remove_file(&gaps);
    let answers = session(
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{}}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"lion_check","arguments":{"code":"let x = \"a\" + 1\n"}}}"#,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"lion_run","arguments":{"code":"use system\nshow(system.args())\nshow(ask(\"\"))\n","args":["a"],"stdin":"b\n"}}}"#,
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"lion_run","arguments":{"code":"while true: let x = 1 ;\n","timeout":1}}}"#,
            r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"lion_fmt","arguments":{"code":"if true:\n  show(1)\n;\n"}}}"#,
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"lion_report_gap","arguments":{"title":"Padding","need":"pad a number","example":"text.pad(\"7\", 3)"}}}"#,
            r#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"lion_report_gap","arguments":{"title":"Padding","need":"again"}}}"#,
            r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"lion_module","arguments":{"name":"text"}}}"#,
            r#"{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"lion_guide","arguments":{}}}"#,
            r#"{"jsonrpc":"2.0","id":11,"method":"unknown"}"#,
        ],
        &gaps,
    );
    assert_eq!(answers.len(), 11, "a notification gets no answer");
    let info = answers[0].get("result").unwrap();
    assert!(info.get("instructions").unwrap().as_text().unwrap().contains("lion_check"));
    assert_eq!(info.get("protocolVersion").unwrap().as_text(), Some("2025-06-18"));
    let tools = answers[1].get("result").unwrap().get("tools").unwrap().as_list().unwrap();
    assert_eq!(tools.len(), 9);

    let (check, failed) = tool_text(&answers[2]);
    assert!(failed && check.contains("`+` cannot be applied"), "{check}");
    let (run, failed) = tool_text(&answers[3]);
    assert!(!failed && run.contains("[\"a\"]\nb\n"), "{run}");
    let (stopped, failed) = tool_text(&answers[4]);
    assert!(failed && stopped.starts_with("stopped after 1 seconds"), "{stopped}");
    let (formatted, _) = tool_text(&answers[5]);
    assert_eq!(formatted, "if true:\n    show(1)\n;\n");
    assert!(tool_text(&answers[6]).0.starts_with("Noted"));
    assert!(tool_text(&answers[7]).0.contains("already in the list"));
    let list = std::fs::read_to_string(&gaps).unwrap();
    assert!(list.starts_with("# Ce qui manque à Lion"));
    assert_eq!(list.matches("— Padding").count(), 1);
    assert!(list.contains("text.pad(\"7\", 3)"));
    assert!(tool_text(&answers[8]).0.contains("fun split("));
    assert!(tool_text(&answers[9]).0.starts_with("# Writing Lion"));
    assert_eq!(answers[10].get("error").unwrap().get("code"), Some(&Json::int(-32601)));
}
