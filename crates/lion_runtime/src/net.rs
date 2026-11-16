//! The network (§23, module `net`, C104): a small HTTP/1.1 server on `std::net`, and
//! requests made by the `curl` program, which brings HTTPS, redirections and proxies
//! without a dependency, as `git` brings the packages (C101).
//!
//! The server reads the requests on threads of its own and gives them in the order they
//! were completed; each answer closes its connection. A request that
//! the server cannot read gets its answer here (400, 411, 413…) and never reaches the
//! program.

use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The largest head of a request: its first line and its headers.
const MAX_HEAD: usize = 64 * 1024;
/// The largest body of a request.
const MAX_BODY: usize = 64 * 1024 * 1024;
/// How long a client may take to send its request, or to read the answer.
const PATIENCE: Duration = Duration::from_secs(10);

static NEXT: AtomicI64 = AtomicI64::new(1);
static SERVERS: Mutex<Option<HashMap<i64, Server>>> = Mutex::new(None);
static CONNECTIONS: Mutex<Option<HashMap<i64, Connection>>> = Mutex::new(None);

/// The most requests read at the same time; the next ones get 503.
const MAX_READING: usize = 64;

struct Server {
    port: i64,
    /// The requests read, in the order they were completed.
    ready: Arc<Mutex<Receiver<Request>>>,
}

/// A request waiting for its answer, and the files sent with it.
struct Connection {
    stream: TcpStream,
    uploads: Vec<PathBuf>,
}

/// A file sent with a form (`multipart/form-data`), saved in a temporary file that is
/// deleted once the request has its answer.
#[derive(Debug, PartialEq, Clone)]
pub struct Upload {
    pub field: String,
    pub filename: String,
    pub content_type: String,
    pub path: String,
    pub size: i64,
}

/// A request received by a server.
#[derive(Debug, PartialEq)]
pub struct Request {
    /// The number that `respond` takes to answer it.
    pub connection: i64,
    pub method: String,
    /// The path, decoded, without the query.
    pub path: String,
    /// The query after `?`, as written.
    pub query: String,
    /// The names, lowercase, and the values of the headers.
    pub headers: Vec<(String, String)>,
    /// The body; for a form with files, its other fields, as `a=1&b=2`.
    pub body: String,
    pub uploads: Vec<Upload>,
}

/// An answer to a request made with `fetch`.
#[derive(Debug, PartialEq)]
pub struct Response {
    pub status: i64,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// Starts a server on `host:port`; port 0 lets the system choose a free one. Gives the
/// number of the server. Connections are accepted and read on threads of their own, so
/// that a slow client delays no one.
pub fn listen(host: &str, port: i64) -> Result<i64, String> {
    let Ok(port) = u16::try_from(port) else {
        return Err(format!("{port} is not a port: a port goes from 0 to 65535"));
    };
    let listener = TcpListener::bind((host, port))
        .map_err(|problem| format!("cannot listen on {host}:{port}: {}", describe(&problem)))?;
    let port = listener.local_addr().map_or(0, |address| i64::from(address.port()));
    let (sender, receiver) = channel();
    std::thread::spawn(move || {
        let reading = Arc::new(AtomicUsize::new(0));
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let _ = stream.set_read_timeout(Some(PATIENCE));
            let _ = stream.set_write_timeout(Some(PATIENCE));
            if reading.fetch_add(1, Ordering::SeqCst) >= MAX_READING {
                reading.fetch_sub(1, Ordering::SeqCst);
                let _ = write_response(&mut stream, 503, &text_type(), b"too many requests at once\n");
                continue;
            }
            let (sender, reading) = (sender.clone(), reading.clone());
            std::thread::spawn(move || {
                let read = read_request(&mut stream);
                reading.fetch_sub(1, Ordering::SeqCst);
                match read {
                    Ok((mut request, uploads)) => {
                        let id = NEXT.fetch_add(1, Ordering::Relaxed);
                        request.connection = id;
                        CONNECTIONS
                            .lock()
                            .expect("never poisoned")
                            .get_or_insert_with(HashMap::new)
                            .insert(id, Connection { stream, uploads });
                        let _ = sender.send(request);
                    }
                    Err(Some((status, message))) => {
                        let _ = write_response(
                            &mut stream,
                            status,
                            &text_type(),
                            format!("{message}\n").as_bytes(),
                        );
                    }
                    Err(None) => {}
                }
            });
        }
    });
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let server = Server { port, ready: Arc::new(Mutex::new(receiver)) };
    SERVERS.lock().expect("never poisoned").get_or_insert_with(HashMap::new).insert(id, server);
    Ok(id)
}

fn text_type() -> [(String, String); 1] {
    [("Content-Type".to_string(), "text/plain; charset=utf-8".to_string())]
}

/// The port on which the server listens.
pub fn port(server: i64) -> i64 {
    let servers = SERVERS.lock().expect("never poisoned");
    servers.as_ref().and_then(|servers| servers.get(&server)).expect("a server made by `listen`").port
}

/// Waits for the next request that could be read.
pub fn accept(server: i64) -> Result<Request, String> {
    let ready = {
        let servers = SERVERS.lock().expect("never poisoned");
        servers
            .as_ref()
            .and_then(|servers| servers.get(&server))
            .expect("a server made by `listen`")
            .ready
            .clone()
    };
    let ready = ready.lock().expect("never poisoned");
    ready.recv().map_err(|_| "the server stopped listening".to_string())
}

/// Answers the request `connection`, then closes it and deletes the files sent with it.
/// With `file`, the body is the content of that file, of any kind.
pub fn respond(
    connection: i64,
    status: i64,
    headers: &[(String, String)],
    body: &str,
    file: &str,
) -> Result<(), String> {
    let Some(Connection { mut stream, uploads }) =
        CONNECTIONS.lock().expect("never poisoned").as_mut().and_then(|open| open.remove(&connection))
    else {
        return Err("this request already has its answer".to_string());
    };
    let answered = answer(&mut stream, status, headers, body, file);
    for upload in uploads {
        let _ = std::fs::remove_file(upload);
    }
    answered
}

fn answer(
    stream: &mut TcpStream,
    status: i64,
    headers: &[(String, String)],
    body: &str,
    file: &str,
) -> Result<(), String> {
    if !(100..=999).contains(&status) {
        let _ = write_response(stream, 500, &[], b"");
        return Err(format!("{status} is not an HTTP status: a status has three digits"));
    }
    if let Some((name, value)) = headers.iter().find(|(name, value)| !valid_header(name, value)) {
        let _ = write_response(stream, 500, &[], b"");
        return Err(format!(
            "`{name}: {value}` is not a header: a name is a word, and no line end is allowed"
        ));
    }
    let written = if file.is_empty() {
        write_response(stream, status, headers, body.as_bytes())
    } else {
        match std::fs::read(file) {
            Ok(content) => {
                let mut headers = headers.to_vec();
                if !headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("content-type")) {
                    headers.push(("Content-Type".to_string(), media_type(file).to_string()));
                }
                write_response(stream, status, &headers, &content)
            }
            Err(problem) => {
                let _ = write_response(stream, 404, &text_type(), b"not found\n");
                return Err(format!("cannot send `{file}`: {}", describe(&problem)));
            }
        }
    };
    written.map_err(|problem| format!("cannot answer the request: {}", describe(&problem)))
}

/// The media type of a file, from its extension.
pub fn media_type(path: &str) -> &'static str {
    let extension = path.rsplit_once('.').map_or("", |(_, e)| e).to_ascii_lowercase();
    match extension.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "txt" | "md" | "lion" => "text/plain; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "wasm" => "application/wasm",
        "mp3" => "audio/mpeg",
        "mp4" => "video/mp4",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

type Refusal = Option<(i64, &'static str)>;

/// A request read from the stream, and the temporary files of its uploads; or the
/// answer to give instead (`None`: none, the client left).
fn read_request(stream: &mut TcpStream) -> Result<(Request, Vec<PathBuf>), Refusal> {
    let mut data = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        if let Some(end) = find(&data, b"\r\n\r\n") {
            break end;
        }
        if data.len() > MAX_HEAD {
            return Err(Some((431, "the headers of the request are too long")));
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return Err(None),
            Ok(count) => data.extend_from_slice(&chunk[..count]),
        }
    };
    let mut body = data.split_off(head_end + 4);
    data.truncate(head_end);
    let head = String::from_utf8(data).map_err(|_| Some((400, "the request is not readable")))?;
    let mut lines = head.split("\r\n");
    let first = lines.next().unwrap_or_default();
    let mut words = first.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (words.next(), words.next(), words.next(), words.next())
    else {
        return Err(Some((400, "the first line of the request is not `METHOD /path HTTP/1.1`")));
    };
    if !version.starts_with("HTTP/1.") || method.is_empty() || !method.bytes().all(|b| b.is_ascii_uppercase())
    {
        return Err(Some((400, "the first line of the request is not `METHOD /path HTTP/1.1`")));
    }
    let mut headers = Vec::new();
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            return Err(Some((400, "a header of the request has no `:`")));
        };
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
    }
    let header = |name: &str| headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone());
    if header("transfer-encoding").is_some() {
        return Err(Some((411, "send the body with a Content-Length")));
    }
    let length = match header("content-length") {
        None => 0,
        Some(length) => {
            length.parse::<usize>().map_err(|_| Some((400, "the Content-Length is not a number")))?
        }
    };
    if length > MAX_BODY {
        return Err(Some((413, "the body of the request is too long")));
    }
    if header("expect").is_some_and(|value| value.eq_ignore_ascii_case("100-continue")) && body.len() < length
    {
        let _ = stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n");
    }
    while body.len() < length {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return Err(None),
            Ok(count) => body.extend_from_slice(&chunk[..count]),
        }
    }
    body.truncate(length);
    let content_type = header("content-type").unwrap_or_default();
    let (body, uploads) = match boundary(&content_type) {
        Some(boundary) => multipart(&body, &boundary)?,
        None => (
            String::from_utf8(body).map_err(|_| Some((400, "the body of the request is not UTF-8 text")))?,
            Vec::new(),
        ),
    };
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let path = decode(path, false).ok_or(Some((400, "the path of the request is not UTF-8 text")))?;
    let paths = uploads.iter().map(|upload: &Upload| PathBuf::from(&upload.path)).collect();
    let request = Request {
        connection: 0,
        method: method.to_string(),
        path,
        query: query.to_string(),
        headers,
        body,
        uploads,
    };
    Ok((request, paths))
}

/// The boundary of a `multipart/form-data` body.
fn boundary(content_type: &str) -> Option<String> {
    let (kind, parameters) = content_type.split_once(';')?;
    if !kind.trim().eq_ignore_ascii_case("multipart/form-data") {
        return None;
    }
    parameters.split(';').find_map(|parameter| {
        let (name, value) = parameter.split_once('=')?;
        name.trim().eq_ignore_ascii_case("boundary").then(|| value.trim().trim_matches('"').to_string())
    })
}

/// The fields of a form with files: the text fields as `a=1&b=2`, and the files saved
/// in temporary files.
fn multipart(body: &[u8], boundary: &str) -> Result<(String, Vec<Upload>), Refusal> {
    let unreadable = Some((400, "the form of the request is not readable"));
    let delimiter = format!("--{boundary}");
    let mut fields = Vec::new();
    let mut uploads = Vec::new();
    let mut rest = &body[find(body, delimiter.as_bytes()).ok_or(unreadable)? + delimiter.len()..];
    loop {
        if rest.starts_with(b"--") {
            break;
        }
        rest = rest.strip_prefix(b"\r\n").ok_or(unreadable)?;
        let head_end = find(rest, b"\r\n\r\n").ok_or(unreadable)?;
        let head = String::from_utf8_lossy(&rest[..head_end]).to_string();
        rest = &rest[head_end + 4..];
        let end = find(rest, format!("\r\n{delimiter}").as_bytes()).ok_or(unreadable)?;
        let content = &rest[..end];
        rest = &rest[end + 2 + delimiter.len()..];
        let mut name = String::new();
        let mut filename = None;
        let mut content_type = String::from("application/octet-stream");
        for line in head.split("\r\n") {
            let Some((header, value)) = line.split_once(':') else { continue };
            if header.trim().eq_ignore_ascii_case("content-type") {
                content_type = value.trim().to_string();
            } else if header.trim().eq_ignore_ascii_case("content-disposition") {
                for parameter in value.split(';') {
                    if let Some((key, value)) = parameter.split_once('=') {
                        let value = value.trim().trim_matches('"').to_string();
                        match key.trim() {
                            "name" => name = value,
                            "filename" => filename = Some(value),
                            _ => {}
                        }
                    }
                }
            }
        }
        match filename {
            Some(filename) => {
                let path = std::env::temp_dir().join(format!(
                    "lion-upload-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
                std::fs::write(&path, content)
                    .map_err(|_| Some((500, "the server cannot keep the file sent")))?;
                uploads.push(Upload {
                    field: name,
                    // Only the name: a client chooses no folder of the server.
                    filename: filename.rsplit(['/', '\\']).next().unwrap_or_default().to_string(),
                    content_type,
                    path: path.to_string_lossy().into_owned(),
                    size: content.len() as i64,
                });
            }
            None => {
                let value = std::str::from_utf8(content)
                    .map_err(|_| Some((400, "a field of the form is not UTF-8 text")))?;
                fields.push(format!("{}={}", encode(&name), encode(value)));
            }
        }
    }
    Ok((fields.join("&"), uploads))
}

fn write_response(
    stream: &mut TcpStream,
    status: i64,
    headers: &[(String, String)],
    body: &[u8],
) -> std::io::Result<()> {
    let mut out = format!("HTTP/1.1 {status} {}\r\n", reason(status));
    for (name, value) in headers {
        if !name.eq_ignore_ascii_case("content-length") && !name.eq_ignore_ascii_case("connection") {
            out.push_str(&format!("{name}: {value}\r\n"));
        }
    }
    out.push_str(&format!("Content-Length: {}\r\nConnection: close\r\n\r\n", body.len()));
    stream.write_all(out.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()?;
    let _ = stream.shutdown(std::net::Shutdown::Write);
    Ok(())
}

fn valid_header(name: &str, value: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
        && !value.contains(['\r', '\n'])
}

fn reason(status: i64) -> &'static str {
    match status {
        100 => "Continue",
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        411 => "Length Required",
        413 => "Content Too Large",
        422 => "Unprocessable Content",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

fn find(data: &[u8], part: &[u8]) -> Option<usize> {
    data.windows(part.len()).position(|window| window == part)
}

/// The text with its `%XX` decoded, and `+` as a space in a form; `None` when the result
/// is not UTF-8.
pub fn decode(text: &str, form: bool) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                (Some(high), Some(low)) => {
                    out.push((high * 16 + low) as u8);
                    i += 3;
                    continue;
                }
                _ => out.push(b'%'),
            },
            b'+' if form => out.push(b' '),
            byte => out.push(byte),
        }
        i += 1;
    }
    String::from_utf8(out).ok()
}

/// The pairs `name=value` of a query or a form, decoded. A pair that is not UTF-8 is
/// left out.
pub fn form_pairs(text: &str) -> Vec<(String, String)> {
    text.split('&')
        .filter(|pair| !pair.is_empty())
        .filter_map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            Some((decode(name, true)?, decode(value, true)?))
        })
        .collect()
}

/// The text with every byte other than a letter, a digit, `-`, `.`, `_` and `~` written
/// `%XX`, to put it in an address (RFC 3986).
pub fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Makes a request with `curl`. An answer with any status is a `Response`; an address
/// that cannot be reached is an Error.
pub fn fetch(
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: &str,
    files: &[(String, String)],
    timeout: f64,
    jar: Option<&str>,
) -> Result<(Response, Option<String>), String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(format!("`{url}` is not a web address: it starts with http:// or https://"));
    }
    if method.is_empty() || !method.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err(format!("`{method}` is not an HTTP method, such as GET or POST"));
    }
    if let Some((name, value)) = headers.iter().find(|(name, value)| !valid_header(name, value)) {
        return Err(format!(
            "`{name}: {value}` is not a header: a name is a word, and no line end is allowed"
        ));
    }
    if !(timeout > 0.0 && timeout.is_finite()) {
        return Err("the timeout is a number of seconds above 0".to_string());
    }
    let mut command = Command::new("curl");
    command
        .args(["--silent", "--show-error", "--location", "--max-redirs", "10"])
        .args(["--proto", "=http,https", "--proto-redir", "=http,https"])
        .args(["--max-time", &format!("{timeout}"), "--dump-header", "-"]);
    // GET and POST are curl's own: a POST answered by 303 then goes on with a GET, as in a
    // browser, which `--request POST` would prevent.
    if method != "GET" && method != "POST" {
        command.args(["--request", method]);
    }
    // A request says it comes from Lion, unless the program says otherwise.
    if !headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("user-agent")) {
        command.args(["--user-agent", concat!("lion/", env!("CARGO_PKG_VERSION"))]);
    }
    for (name, value) in headers {
        command.arg("--header").arg(if value.is_empty() {
            format!("{name};")
        } else {
            format!("{name}: {value}")
        });
    }
    // With files, the body is a form (`a=1&b=2`) sent with them as `multipart/form-data`.
    let sends = files.is_empty() && (!body.is_empty() || matches!(method, "POST" | "PUT" | "PATCH"));
    if sends {
        command.args(["--data-binary", "@-"]);
    }
    if !files.is_empty() {
        for (name, value) in form_pairs(body) {
            command.arg("--form-string").arg(format!("{name}={value}"));
        }
        for (field, path) in files {
            if field.is_empty() || field.contains(['=', ';', '"', '\r', '\n']) {
                return Err(format!("`{field}` is not a field name: a name has no `=`, `;` or quote"));
            }
            if !std::path::Path::new(path).is_file() {
                return Err(format!("cannot send `{path}`: the file does not exist"));
            }
            let quoted = path.replace('\\', "\\\\").replace('"', "\\\"");
            command.arg("--form").arg(format!("{field}=@\"{quoted}\""));
        }
    }
    // The cookies of a session go through a file of curl's own format, read back after.
    let jar_file = match jar {
        Some(jar) => {
            let path = std::env::temp_dir().join(format!(
                "lion-cookies-{}-{}.txt",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::write(&path, jar).map_err(|problem| format!("cannot keep the cookies: {problem}"))?;
            command.arg("--cookie").arg(&path).arg("--cookie-jar").arg(&path);
            Some(path)
        }
        None => None,
    };
    command.arg("--url").arg(url).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let output = run_curl(command, sends, body);
    let jar = jar_file.map(|path| {
        let kept = std::fs::read_to_string(&path).unwrap_or_default();
        let _ = std::fs::remove_file(&path);
        kept
    });
    let output = output?;
    if !output.status.success() {
        return Err(format!("cannot reach `{url}`: {}", curl_message(&output.stderr)));
    }
    let response = parse_response(&output.stdout).map_err(|problem| format!("`{url}`: {problem}"))?;
    Ok((response, jar))
}

fn run_curl(mut command: Command, sends: bool, body: &str) -> Result<std::process::Output, String> {
    let mut child = command.spawn().map_err(|problem| curl_missing(&problem))?;
    let mut stdin = child.stdin.take().expect("piped");
    let body = body.to_string();
    let writer = std::thread::spawn(move || {
        if sends {
            let _ = stdin.write_all(body.as_bytes());
        }
    });
    let output = child.wait_with_output().map_err(|problem| format!("`curl` failed: {problem}"));
    let _ = writer.join();
    output
}

/// The answer that `curl --dump-header -` writes: the heads of the redirections and of
/// the proxy, then the last head and the body.
fn parse_response(mut data: &[u8]) -> Result<Response, String> {
    loop {
        let Some(end) = find(data, b"\r\n\r\n") else { return Err("the answer has no headers".to_string()) };
        let head = String::from_utf8_lossy(&data[..end]);
        data = &data[end + 4..];
        let mut lines = head.split("\r\n");
        let first = lines.next().unwrap_or_default();
        let mut words = first.splitn(3, ' ');
        let status = match (words.next(), words.next()) {
            (Some(version), Some(status)) if version.starts_with("HTTP/") => status.parse::<i64>().ok(),
            _ => None,
        };
        let Some(status) = status else { return Err(format!("the answer starts with `{first}`")) };
        let headers: Vec<(String, String)> = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
            .collect();
        let established = status == 200 && first.to_ascii_lowercase().contains("connection established");
        let followed = (300..400).contains(&status) && headers.iter().any(|(name, _)| name == "location");
        let more = data.starts_with(b"HTTP/");
        if more && ((100..200).contains(&status) || established || followed) {
            continue;
        }
        let body = String::from_utf8(data.to_vec())
            .map_err(|_| "the answer is not UTF-8 text: save it in a file with `net.download`".to_string())?;
        return Ok(Response { status, headers, body });
    }
}

/// Saves what the address gives in the file `path`; a status from 400 up is an Error.
pub fn download(url: &str, path: &str, timeout: f64) -> Result<(), String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(format!("`{url}` is not a web address: it starts with http:// or https://"));
    }
    if !(timeout > 0.0 && timeout.is_finite()) {
        return Err("the timeout is a number of seconds above 0".to_string());
    }
    let output = Command::new("curl")
        .args(["--silent", "--show-error", "--fail", "--location", "--max-redirs", "10"])
        .args(["--proto", "=http,https", "--proto-redir", "=http,https"])
        .args(["--max-time", &format!("{timeout}"), "--output", path, "--url", url])
        .stdin(Stdio::null())
        .output()
        .map_err(|problem| curl_missing(&problem))?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!("cannot download `{url}`: {}", curl_message(&output.stderr)))
}

fn curl_missing(problem: &std::io::Error) -> String {
    match problem.kind() {
        ErrorKind::NotFound => {
            "`net` makes its requests with the program `curl`, which is not installed".to_string()
        }
        _ => format!("cannot start `curl`: {problem}"),
    }
}

/// What `curl` says on failure, without its prefix: `curl: (6) Could not resolve host: x`
/// gives `Could not resolve host: x`.
fn curl_message(stderr: &[u8]) -> String {
    let message = String::from_utf8_lossy(stderr);
    let message = message.trim();
    let message = message.strip_prefix("curl: ").unwrap_or(message);
    message.split_once(") ").filter(|(code, _)| code.starts_with('(')).map_or(message, |(_, m)| m).to_string()
}

fn describe(problem: &std::io::Error) -> String {
    match problem.kind() {
        ErrorKind::AddrInUse => "the port is already taken".to_string(),
        ErrorKind::PermissionDenied => {
            "permission denied (a port below 1024 needs special rights)".to_string()
        }
        _ => problem.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_and_encodes_addresses() {
        assert_eq!(decode("/a%20b/%C3%A9", false).as_deref(), Some("/a b/é"));
        assert_eq!(decode("a+b%2", true).as_deref(), Some("a b%2"));
        assert_eq!(decode("%", false).as_deref(), Some("%"));
        assert_eq!(decode("%zz", false).as_deref(), Some("%zz"));
        assert_eq!(decode("%FF", false), None);
        assert_eq!(encode("é a/b"), "%C3%A9%20a%2Fb");
        assert_eq!(
            form_pairs("name=L%C3%A9a&x&&age=12+ans"),
            vec![
                ("name".to_string(), "Léa".to_string()),
                ("x".to_string(), String::new()),
                ("age".to_string(), "12 ans".to_string())
            ]
        );
    }

    #[test]
    fn reads_the_last_head_of_an_answer() {
        let data = b"HTTP/1.1 301 Moved\r\nLocation: /b\r\n\r\nHTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nHTTP/ body";
        let response = parse_response(data).unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.headers, vec![("content-type".to_string(), "text/plain".to_string())]);
        assert_eq!(response.body, "HTTP/ body");
        let not_found = parse_response(b"HTTP/2 404 \r\n\r\n").unwrap();
        assert_eq!((not_found.status, not_found.body.as_str()), (404, ""));
    }

    #[test]
    fn a_server_answers_a_request() {
        let server = listen("127.0.0.1", 0).unwrap();
        let address = format!("127.0.0.1:{}", port(server));
        let client = std::thread::spawn(move || {
            let mut stream = TcpStream::connect(address).unwrap();
            stream
                .write_all(b"POST /a%20b?x=1 HTTP/1.1\r\nHost: h\r\nContent-Length: 5\r\n\r\nhello")
                .unwrap();
            let mut answer = String::new();
            stream.read_to_string(&mut answer).unwrap();
            answer
        });
        let request = accept(server).unwrap();
        assert_eq!((request.method.as_str(), request.path.as_str()), ("POST", "/a b"));
        assert_eq!((request.query.as_str(), request.body.as_str()), ("x=1", "hello"));
        assert_eq!(request.headers[0], ("host".to_string(), "h".to_string()));
        respond(request.connection, 201, &[("X-A".to_string(), "b".to_string())], "done", "").unwrap();
        assert!(respond(request.connection, 200, &[], "", "").is_err());
        let answer = client.join().unwrap();
        assert!(answer.starts_with("HTTP/1.1 201 Created\r\nX-A: b\r\nContent-Length: 4\r\n"), "{answer}");
        assert!(answer.ends_with("\r\n\r\ndone"));
    }
}
