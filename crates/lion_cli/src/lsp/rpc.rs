//! The messages of the Language Server Protocol: JSON texts, each after a header that
//! gives its length in bytes (JSON-RPC 2.0).

use std::io::{self, BufRead, Write};

use lion_runtime::json::{self, Json};

/// What the editor sent.
pub enum Incoming {
    Message(Json),
    /// A message whose text is not JSON; the next one can still be read.
    Invalid(String),
    /// The input is closed, or its headers cannot be read: nothing more will come.
    End,
}

/// Reads the next message.
pub fn read(input: &mut impl BufRead) -> Incoming {
    let mut length = None;
    loop {
        let mut line = String::new();
        match input.read_line(&mut line) {
            Ok(0) | Err(_) => return Incoming::End,
            Ok(_) => {}
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            // Empty lines before the headers are tolerated.
            if length.is_some() {
                break;
            }
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            eprintln!("lion lsp: a header without `:`: `{line}`");
            return Incoming::End;
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            match value.trim().parse::<usize>() {
                Ok(value) => length = Some(value),
                Err(_) => {
                    eprintln!("lion lsp: a wrong `Content-Length`: `{line}`");
                    return Incoming::End;
                }
            }
        }
    }
    let mut body = vec![0; length.unwrap_or(0)];
    if input.read_exact(&mut body).is_err() {
        return Incoming::End;
    }
    let Ok(text) = String::from_utf8(body) else {
        return Incoming::Invalid("the message is not valid UTF-8".to_string());
    };
    match json::parse(&text) {
        Ok(message) => Incoming::Message(message),
        Err(problem) => Incoming::Invalid(problem),
    }
}

/// Writes a message.
pub fn write(output: &mut impl Write, message: &Json) -> io::Result<()> {
    let body = message.to_string();
    write!(output, "Content-Length: {}\r\n\r\n{body}", body.len())?;
    output.flush()
}

/// The answer to the request `id`.
pub fn response(id: Json, result: Json) -> Json {
    Json::object([("jsonrpc", Json::text("2.0")), ("id", id), ("result", result)])
}

/// The error that answers the request `id`.
pub fn error(id: Json, code: i64, message: impl Into<String>) -> Json {
    let error = Json::object([("code", Json::int(code)), ("message", Json::text(message))]);
    Json::object([("jsonrpc", Json::text("2.0")), ("id", id), ("error", error)])
}

pub fn notification(method: &str, params: Json) -> Json {
    Json::object([("jsonrpc", Json::text("2.0")), ("method", Json::text(method)), ("params", params)])
}

/// The error codes of JSON-RPC and of the protocol.
pub mod code {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const SERVER_NOT_INITIALIZED: i64 = -32002;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_framed_by_their_length() {
        let mut out = Vec::new();
        write(&mut out, &Json::object([("a", Json::text("é"))])).unwrap();
        assert_eq!(String::from_utf8(out.clone()).unwrap(), "Content-Length: 10\r\n\r\n{\"a\":\"é\"}");
        let mut two = out.clone();
        two.extend_from_slice(b"Content-Length: 3\r\nContent-Type: application/vscode-jsonrpc\r\n\r\n[1]");
        two.extend_from_slice(b"Content-Length: 2\r\n\r\n{]");
        let mut input = &two[..];
        assert!(
            matches!(read(&mut input), Incoming::Message(m) if m.get("a").and_then(Json::as_text) == Some("é"))
        );
        assert!(matches!(read(&mut input), Incoming::Message(Json::List(items)) if items.len() == 1));
        assert!(matches!(read(&mut input), Incoming::Invalid(_)));
        assert!(matches!(read(&mut input), Incoming::End));
        let mut broken = &b"Content-Length: x\r\n\r\n"[..];
        assert!(matches!(read(&mut broken), Incoming::End));
    }
}
