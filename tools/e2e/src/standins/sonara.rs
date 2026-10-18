//! SonaraStandIn: a 127.0.0.1 HTTP stand-in with runtime.json and a bearer check (Task 15).
//!
//! Sonara is a program on the owner's PC; a scenario runs this stand-in instead, under a SONARA_HOME of
//! its own. Its `runtime.json` is the one `flare-agent::sonara::read_runtime` reads (`http_port` and
//! `token`; the plan's `port`/`key` names were never the committed reader's). Every request is recorded
//! before it is answered: a request with the wrong bearer is refused with 401 and recorded with
//! `auth_ok: false`, a good one answers 200 for `/v1/question_shown`.

use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{json, Value};

/// The one path Sonara answers.
const QUESTION_SHOWN: &str = "/v1/question_shown";

/// One request the stand-in received.
#[derive(Clone, Debug, PartialEq)]
pub struct Post {
    pub method: String,
    pub path: String,
    /// True when the bearer matched the stand-in's key.
    pub auth_ok: bool,
    /// The status the stand-in answered with.
    pub status: u16,
    /// The JSON body, or `Null` when it was not JSON.
    pub body: Value,
    /// The tab, from the body's `host_tab` or the `X-Flare-Tab-Id` header.
    pub tab_id: Option<String>,
}

/// A Sonara on 127.0.0.1 with its own home. Dropping it stops the server thread.
pub struct SonaraStandIn {
    pub port: u16,
    pub key: String,
    pub home: PathBuf,
    posts: Arc<Mutex<Vec<Post>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl SonaraStandIn {
    /// Binds a free loopback port, writes `runtime.json` into `home` and starts answering.
    pub fn start(home: &Path, key: &str) -> Result<Self, String> {
        std::fs::create_dir_all(home).map_err(|e| format!("the sonara home: {e}"))?;
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("bind: {e}"))?;
        let port = listener
            .local_addr()
            .map_err(|e| format!("addr: {e}"))?
            .port();
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("nonblocking: {e}"))?;
        std::fs::write(
            home.join("runtime.json"),
            json!({ "http_port": port, "token": key }).to_string(),
        )
        .map_err(|e| format!("runtime.json: {e}"))?;

        let posts = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (posts, stop, key) = (Arc::clone(&posts), Arc::clone(&stop), key.to_string());
            std::thread::spawn(move || serve(&listener, &key, &posts, &stop))
        };
        Ok(Self {
            port,
            key: key.to_string(),
            home: home.to_path_buf(),
            posts,
            stop,
            thread: Some(thread),
        })
    }

    /// Every request so far, in arrival order.
    pub fn posts(&self) -> Vec<Post> {
        self.posts.lock().map(|p| p.clone()).unwrap_or_default()
    }

    /// The variable that points the app at this stand-in's home.
    pub fn env(&self) -> Vec<(String, String)> {
        vec![("SONARA_HOME".into(), self.home.display().to_string())]
    }
}

impl Drop for SonaraStandIn {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve(listener: &TcpListener, key: &str, posts: &Mutex<Vec<Post>>, stop: &AtomicBool) {
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                let _ = handle(stream, key, posts);
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(_) => break,
        }
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn handle(mut stream: TcpStream, key: &str, posts: &Mutex<Vec<Post>>) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut raw = Vec::new();
    let mut buf = [0u8; 1024];
    let head_end = loop {
        if let Some(i) = find(&raw, b"\r\n\r\n") {
            break i;
        }
        let n = stream.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        raw.extend_from_slice(&buf[..n]);
    };
    let head = String::from_utf8_lossy(&raw[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut request = lines.next().unwrap_or_default().split_whitespace();
    let method = request.next().unwrap_or_default().to_string();
    let path = request.next().unwrap_or_default().to_string();
    let mut auth = None;
    let mut length = 0usize;
    let mut tab_header = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            let value = value.trim();
            match name.trim().to_ascii_lowercase().as_str() {
                "authorization" => auth = Some(value.to_string()),
                "content-length" => length = value.parse().unwrap_or(0),
                "x-flare-tab-id" => tab_header = Some(value.to_string()),
                _ => {}
            }
        }
    }
    let body_start = head_end + 4;
    while raw.len() < body_start + length {
        let n = stream.read(&mut buf)?;
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..n]);
    }
    let end = (body_start + length).min(raw.len());
    let body: Value = serde_json::from_slice(&raw[body_start.min(end)..end]).unwrap_or(Value::Null);

    let auth_ok = auth.as_deref() == Some(format!("Bearer {key}").as_str());
    let status = if !auth_ok {
        401
    } else if path == QUESTION_SHOWN {
        200
    } else {
        404
    };
    let tab_id = body
        .get("host_tab")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or(tab_header);
    if let Ok(mut log) = posts.lock() {
        log.push(Post {
            method,
            path,
            auth_ok,
            status,
            body,
            tab_id,
        });
    }

    let reason = match status {
        200 => "OK",
        401 => "Unauthorized",
        _ => "Not Found",
    };
    let reply = if status == 200 {
        "{\"ok\":true,\"moved\":true}"
    } else {
        "{\"ok\":false}"
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
        reply.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}
