//! Servers written in Lion, reached by raw TCP clients (C106): paths that try to leave
//! the folder of `files`, a client that sends nothing, and answers given in parallel.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn folder(name: &str) -> PathBuf {
    let folder = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(folder.join("public")).unwrap();
    folder
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// A server, stopped when the test ends, even by a failure.
struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Starts `lion run program port`, and waits until the port answers.
fn start(program: &Path, port: u16) -> Server {
    let mut child = Command::new(env!("CARGO_BIN_EXE_lion"))
        .arg("run")
        .arg(program)
        .arg(port.to_string())
        .stdout(Stdio::null())
        .spawn()
        .expect("the lion binary runs");
    let start = Instant::now();
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        if start.elapsed() > Duration::from_secs(20) || child.try_wait().ok().flatten().is_some() {
            let _ = child.kill();
            panic!("the server never listened");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Server(child)
}

/// Sends `request` as written and gives the answer.
fn ask(port: u16, request: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    answer
}

fn get(port: u16, path: &str) -> String {
    ask(port, &format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n"))
}

#[test]
fn files_stay_in_their_folder_and_a_silent_client_delays_no_one() {
    let folder = folder("server-files");
    std::fs::write(folder.join("public/ok.txt"), "ok\n").unwrap();
    std::fs::write(folder.join("secret.txt"), "secret\n").unwrap();
    let program = folder.join("site.lion");
    std::fs::write(
        &program,
        format!(
            "use net\nuse system\nlet port = try system.args()[1] as Int\n\
             let app = net.router().files(\"/static\", {:?})\n\
             try net.serve(port, app.handle)\n",
            folder.join("public").display().to_string()
        ),
    )
    .unwrap();
    let port = free_port();
    let _server = start(&program, port);
    // A client that opens a connection and sends nothing.
    let silent = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let begun = Instant::now();
    assert!(get(port, "/static/ok.txt").ends_with("\r\n\r\nok\n"));
    assert!(begun.elapsed() < Duration::from_secs(5), "the silent client delayed the others");
    for path in [
        "/static/../secret.txt",
        "/static/%2e%2e/secret.txt",
        "/static/a/../../secret.txt",
        "/static/..%5Csecret.txt",
    ] {
        let answer = get(port, path);
        assert!(answer.starts_with("HTTP/1.1 404 "), "{path}: {answer}");
        assert!(!answer.ends_with("\r\n\r\nsecret\n"), "{path}: {answer}");
    }
    drop(silent);
}

#[test]
fn a_body_that_is_not_text_is_read_as_bytes() {
    let folder = folder("server-bytes");
    let program = folder.join("bytes.lion");
    std::fs::write(
        &program,
        "use net\nuse system\nuse bytes\nlet port = try system.args()[1] as Int\n\
         fun echo(r in net.Request) in net.Response = net.bytes(r.bytes(), \"application/octet-stream\")\n\
         try net.serve(port, echo)\n",
    )
    .unwrap();
    let port = free_port();
    let _server = start(&program, port);
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.write_all(b"POST / HTTP/1.1\r\nContent-Length: 4\r\n\r\n\xff\x00\xfe\x01").unwrap();
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer).unwrap();
    assert!(answer.starts_with(b"HTTP/1.1 200 OK\r\n"), "{}", String::from_utf8_lossy(&answer));
    assert!(answer.ends_with(b"\r\n\r\n\xff\x00\xfe\x01"), "{}", String::from_utf8_lossy(&answer));
}

#[test]
fn serve_parallel_answers_several_requests_at_once() {
    let folder = folder("server-parallel");
    let program = folder.join("slow.lion");
    std::fs::write(
        &program,
        "use net\nuse system\nuse time\nlet port = try system.args()[1] as Int\n\
         fun slow(r in net.Request) in net.Response:\n    time.sleep(1.0)\n    return net.plain(\"done {r.path}\")\n;\n\
         try net.serve_parallel(port, slow, 4)\n",
    )
    .unwrap();
    let port = free_port();
    let _server = start(&program, port);
    let begun = Instant::now();
    let clients: Vec<_> = (1..=4).map(|i| std::thread::spawn(move || get(port, &format!("/{i}")))).collect();
    for (i, client) in clients.into_iter().enumerate() {
        let answer = client.join().unwrap();
        assert!(answer.ends_with(&format!("done /{}", i + 1)), "{answer}");
    }
    // One at a time, the four answers would take 4 seconds.
    assert!(begun.elapsed() < Duration::from_millis(3000), "{:?}", begun.elapsed());
}
