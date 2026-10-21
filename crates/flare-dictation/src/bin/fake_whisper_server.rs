//! A stand-in for whisper-server.exe, for the engine tests (Task 13). Tests only, never packed. It
//! speaks the two routes the engine uses (GET / and POST /inference, measured against the real b5130
//! server) and answers with a description of what it RECEIVED, so a test can read the argv, the
//! multipart parts and the order of the passes back out of `text` instead of trusting the engine's word.
//!
//! Started the way the real one is: `-m <model> --host 127.0.0.1 --port <n> -l <lang>`. Switches, as
//! argv or as the environment variable beside each (ported 1:1 from Prism Terminal's fixture):
//!   --fake-exit              FAKE_WHISPER_EXIT=1         say why on stderr and exit 3 at once
//!   --fake-never-ready       FAKE_WHISPER_NEVER_READY=1  stay alive and never listen
//!   --fake-delay <ms>        FAKE_WHISPER_DELAY=<ms>     hold every /inference answer this long
//!   --fake-die-on-inference  FAKE_WHISPER_DIE=1          read the upload, say why on stderr, exit 4

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

struct Args(Vec<String>);

impl Args {
    fn flag(&self, name: &str, var: &str) -> bool {
        self.0.iter().any(|a| a == name) || std::env::var(var).is_ok_and(|v| v == "1")
    }

    fn value(&self, name: &str) -> Option<String> {
        let i = self.0.iter().position(|a| a == name)?;
        self.0.get(i + 1).cloned()
    }
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    hay.get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

struct Part {
    name: Option<String>,
    filename: Option<String>,
    kind: Option<String>,
    data: Vec<u8>,
}

fn header_value(head: &str, key: &str) -> Option<String> {
    let lower = head.to_ascii_lowercase();
    let at = lower.find(&format!("{key}=\""))? + key.len() + 2;
    let rest = &head[at..];
    Some(rest[..rest.find('"')?].to_string())
}

/// The parts of a multipart/form-data body.
fn parse_multipart(body: &[u8], content_type: &str) -> Vec<Part> {
    let Some(at) = content_type.to_ascii_lowercase().find("boundary=") else {
        return Vec::new();
    };
    let boundary = content_type[at + 9..].trim().trim_matches('"').to_string();
    let mark = format!("--{boundary}").into_bytes();
    let mut parts = Vec::new();
    let mut at = find(body, &mark, 0);
    while let Some(start) = at {
        let Some(next) = find(body, &mark, start + mark.len()) else {
            break;
        };
        // Between two marks: CRLF, the headers, a blank line, the bytes, CRLF.
        let chunk = &body[start + mark.len() + 2..next - 2];
        if let Some(split) = find(chunk, b"\r\n\r\n", 0) {
            let head = String::from_utf8_lossy(&chunk[..split]).into_owned();
            let kind = head
                .lines()
                .find(|l| l.to_ascii_lowercase().starts_with("content-type:"))
                .map(|l| l[13..].trim().to_string());
            parts.push(Part {
                name: header_value(&head, "name"),
                filename: header_value(&head, "filename"),
                kind,
                data: chunk[split + 4..].to_vec(),
            });
        }
        at = Some(next);
    }
    parts
}

struct Shared {
    model: Option<String>,
    language: Option<String>,
    port: u16,
    delay: u64,
    die: bool,
    count: AtomicU64,
    active: AtomicU64,
    max_active: AtomicU64,
}

fn reply(stream: &mut TcpStream, status: &str, kind: &str, body: &str) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}

fn handle(mut stream: TcpStream, shared: &Shared) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    let split = loop {
        if let Some(at) = find(&buf, b"\r\n\r\n", 0) {
            break at;
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    };
    let head = String::from_utf8_lossy(&buf[..split]).into_owned();
    let mut first = head.lines().next().unwrap_or("").split_whitespace();
    let (method, path) = (first.next().unwrap_or(""), first.next().unwrap_or(""));
    if method == "GET" {
        reply(
            &mut stream,
            "200 OK",
            "text/html",
            "<html>fake whisper server</html>",
        );
        return;
    }
    if method != "POST" || path != "/inference" {
        reply(&mut stream, "404 Not Found", "text/plain", "");
        return;
    }
    let header = |key: &str| {
        head.lines()
            .find(|l| l.to_ascii_lowercase().starts_with(&format!("{key}:")))
            .map(|l| l[key.len() + 1..].trim().to_string())
    };
    let length: usize = header("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let content_type = header("content-type").unwrap_or_default();
    let mut body = buf[split + 4..].to_vec();
    while body.len() < length {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => body.extend_from_slice(&chunk[..n]),
        }
    }
    if shared.die {
        // The whole message goes out before the exit, so the engine's stderr tail has it.
        let mut err = std::io::stderr();
        let _ = err.write_all(b"fake: ggml_cuda: out of memory\n");
        let _ = err.flush();
        std::process::exit(4);
    }
    let active = shared.active.fetch_add(1, Ordering::SeqCst) + 1;
    shared.max_active.fetch_max(active, Ordering::SeqCst);
    let n = shared.count.fetch_add(1, Ordering::SeqCst) + 1;
    let parts = parse_multipart(&body, &content_type);
    let file = parts.iter().find(|p| p.name.as_deref() == Some("file"));
    let format = parts
        .iter()
        .find(|p| p.name.as_deref() == Some("response_format"));
    std::thread::sleep(Duration::from_millis(shared.delay));
    let seen = serde_json::json!({
        "n": n,
        "maxActive": shared.max_active.load(Ordering::SeqCst),
        "bytes": file.map_or(-1, |p| i64::try_from(p.data.len()).unwrap_or(-1)),
        "filename": file.and_then(|p| p.filename.clone()),
        "type": file.and_then(|p| p.kind.clone()),
        "format": format.map(|p| String::from_utf8_lossy(&p.data).into_owned()),
        "model": shared.model,
        "language": shared.language,
        "host": "127.0.0.1",
        "port": shared.port,
        "pid": std::process::id(),
        "cwd": std::env::current_dir().ok().map(|p| p.to_string_lossy().into_owned()),
        "cudaCache": std::env::var("CUDA_CACHE_PATH").ok(),
        "cudaMax": std::env::var("CUDA_CACHE_MAXSIZE").ok(),
    });
    shared.active.fetch_sub(1, Ordering::SeqCst);
    let answer = serde_json::json!({ "text": seen.to_string() });
    reply(
        &mut stream,
        "200 OK",
        "application/json",
        &answer.to_string(),
    );
}

fn main() {
    let args = Args(std::env::args().skip(1).collect());
    if args.flag("--fake-exit", "FAKE_WHISPER_EXIT") {
        eprintln!("fake: CUDA error: no kernel image is available for execution on the device");
        std::process::exit(3);
    }
    if args.flag("--fake-never-ready", "FAKE_WHISPER_NEVER_READY") {
        // Alive, and deaf: what a server stuck loading its model looks like from outside.
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    let port: u16 = args
        .value("--port")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let host = args
        .value("--host")
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let delay = args
        .value("--fake-delay")
        .or_else(|| std::env::var("FAKE_WHISPER_DELAY").ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let shared = Arc::new(Shared {
        model: args.value("-m"),
        language: args.value("-l"),
        port,
        delay,
        die: args.flag("--fake-die-on-inference", "FAKE_WHISPER_DIE"),
        count: AtomicU64::new(0),
        active: AtomicU64::new(0),
        max_active: AtomicU64::new(0),
    });
    let Ok(listener) = TcpListener::bind((host.as_str(), port)) else {
        eprintln!("fake: could not listen on {host}:{port}");
        std::process::exit(5);
    };
    for stream in listener.incoming().flatten() {
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || handle(stream, &shared));
    }
}
