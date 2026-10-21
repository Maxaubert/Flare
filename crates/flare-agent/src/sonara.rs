//! Telling Sonara the question on screen (Task 12). Ported from Prism Terminal's `core/main/sonaraLink.ts`.
//!
//! Claude Code tells its plugins nothing while the user moves between the questions of one
//! AskUserQuestion, so Sonara cannot read the question the user is on. The tab can: the question widget
//! read off the screen (`question_widget`) is handed to a Sonara running on this PC, naming the tab by
//! the `FLARE_TAB_ID` its shell carries.
//!
//! Only to 127.0.0.1, only when Sonara wrote its `runtime.json` (its home is `SONARA_HOME`, else
//! `%LOCALAPPDATA%\Sonara`). No Sonara: nothing is sent and nothing fails. Every failure is quiet; the
//! caller gets false. The request is hand-written HTTP/1.1 over `std::net`, with no HTTP dependency
//! (decision D9). The calls block, so they run on a worker thread.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use crate::question_widget::ShownQuestion;

/// The only host ever contacted.
const LOOPBACK: Ipv4Addr = Ipv4Addr::LOCALHOST;

/// The most of the status line that is read.
const STATUS_LIMIT: usize = 1024;

/// `SONARA_HOME`, else `%LOCALAPPDATA%\Sonara`, else None. `env` is how the test injects variables.
pub fn sonara_home(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    if let Some(home) = env("SONARA_HOME").filter(|h| !h.is_empty()) {
        return Some(PathBuf::from(home));
    }
    env("LOCALAPPDATA")
        .filter(|l| !l.is_empty())
        .map(|local| PathBuf::from(local).join("Sonara"))
}

/// The port and token Sonara wrote in its `runtime.json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Runtime {
    pub http_port: u16,
    pub token: String,
    /// The process that wrote the file. The post goes only to a port this process listens on.
    pub pid: Option<u32>,
}

/// `runtime.json` in the home: `http_port` an integer 1..=65535 (a JSON number with no fraction; `5.0`
/// counts), `token` a non-empty string without control characters; anything else None.
pub fn read_runtime(home: &Path) -> Option<Runtime> {
    // sync-ok: a small file read on the worker thread that posts (decision D9).
    let raw = std::fs::read_to_string(home.join("runtime.json")).ok()?;
    let value: Value = serde_json::from_str(&raw).ok()?;
    let http_port = port_of(value.get("http_port")?)?;
    let token = value.get("token")?.as_str()?;
    if token.is_empty() || token.chars().any(char::is_control) {
        return None;
    }
    let pid = value
        .get("pid")
        .and_then(Value::as_u64)
        .and_then(|p| u32::try_from(p).ok())
        .filter(|p| *p != 0);
    Some(Runtime {
        http_port,
        token: token.to_string(),
        pid,
    })
}

/// A JSON number that is a whole port: `5` and `5.0` are, `5.5` and `0` and `70000` are not.
fn port_of(value: &Value) -> Option<u16> {
    let n = value.as_f64()?;
    if n.fract() != 0.0 || !(1.0..=65535.0).contains(&n) {
        return None;
    }
    Some(n as u16)
}

/// Tell Sonara what tab `tab_token` (the FLARE_TAB_ID) shows. True only on an HTTP 200. BLOCKING: connect,
/// write and read each run under `timeout`, so call it from a worker thread.
pub fn post_question_shown(
    tab_token: &str,
    shown: &ShownQuestion,
    env: &dyn Fn(&str) -> Option<String>,
    timeout: Duration,
) -> bool {
    if tab_token.is_empty() {
        return false;
    }
    let Some(home) = sonara_home(env) else {
        return false;
    };
    let Some(runtime) = read_runtime(&home) else {
        return false;
    };
    // The file can outlive Sonara and its port can be taken by another program: the token and the
    // question go only to a listener owned by the process the file names (audit L21).
    let Some(pid) = runtime.pid else {
        return false;
    };
    if !flare_win::tcp_owner::only_owner(
        &flare_win::tcp_owner::listener_pids(runtime.http_port),
        pid,
    ) {
        return false;
    }
    let body = match shown {
        ShownQuestion::Text(text) => json!({"host_tab": tab_token, "text": text}),
        ShownQuestion::Review => json!({"host_tab": tab_token, "review": true}),
    }
    .to_string();
    let request = format!(
        "POST /v1/question_shown HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\n\
         Content-Type: application/json\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n{body}",
        port = runtime.http_port,
        token = runtime.token,
        len = body.len(),
    );
    let addr = SocketAddr::from((LOOPBACK, runtime.http_port));
    // sync-ok: the connect, write and read are bounded by `timeout` and run on the posting worker.
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, timeout) else {
        return false;
    };
    if stream.set_read_timeout(Some(timeout)).is_err()
        || stream.set_write_timeout(Some(timeout)).is_err()
        || stream.write_all(request.as_bytes()).is_err()
    {
        return false;
    }
    let mut head = Vec::new();
    let mut buf = [0u8; 256];
    while head.len() < STATUS_LIMIT && !head.contains(&b'\n') {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => head.extend_from_slice(&buf[..n]),
            Err(_) => return false,
        }
    }
    let head = String::from_utf8_lossy(&head);
    let status_line = head.lines().next().unwrap_or("");
    let mut parts = status_line.split_whitespace();
    matches!(parts.next(), Some("HTTP/1.1" | "HTTP/1.0")) && parts.next() == Some("200")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;
    use std::time::Instant;

    /// One request as the fake Sonara read it.
    struct Got {
        request_line: String,
        authorization: Option<String>,
        body: String,
    }

    fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
        hay.windows(needle.len()).position(|w| w == needle)
    }

    /// Reads one whole request (headers, then the Content-Length body) off `s`.
    fn read_request(s: &mut TcpStream) -> Got {
        let mut data = Vec::new();
        let mut buf = [0u8; 1024];
        let head_end = loop {
            if let Some(p) = find(&data, b"\r\n\r\n") {
                break p;
            }
            let n = s.read(&mut buf).expect("read head");
            assert!(n > 0, "client closed before the headers ended");
            data.extend_from_slice(&buf[..n]);
        };
        let head = String::from_utf8_lossy(&data[..head_end]).to_string();
        let len = head
            .lines()
            .filter_map(|l| l.split_once(':'))
            .find(|(k, _)| k.trim().eq_ignore_ascii_case("content-length"))
            .and_then(|(_, v)| v.trim().parse::<usize>().ok())
            .unwrap_or(0);
        let body_start = head_end + 4;
        while data.len() < body_start + len {
            let n = s.read(&mut buf).expect("read body");
            assert!(n > 0, "client closed before the body ended");
            data.extend_from_slice(&buf[..n]);
        }
        let body = String::from_utf8_lossy(&data[body_start..body_start + len]).to_string();
        let authorization = head
            .lines()
            .filter_map(|l| l.split_once(':'))
            .find(|(k, _)| k.trim().eq_ignore_ascii_case("authorization"))
            .map(|(_, v)| v.trim().to_string());
        Got {
            request_line: head.lines().next().unwrap_or("").to_string(),
            authorization,
            body,
        }
    }

    /// A stand-in Sonara on 127.0.0.1: answers `answers` connections with `status`, recording each.
    fn fake_sonara(answers: usize, status: &'static str) -> (u16, thread::JoinHandle<Vec<Got>>) {
        let listener = TcpListener::bind((LOOPBACK, 0)).expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let handle = thread::spawn(move || {
            let mut got = Vec::new();
            for _ in 0..answers {
                let (mut s, _) = listener.accept().expect("accept");
                got.push(read_request(&mut s));
                let body = "{\"ok\":true,\"moved\":true}";
                let reply = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                s.write_all(reply.as_bytes()).expect("write reply");
            }
            got
        });
        (port, handle)
    }

    /// An environment with `SONARA_HOME` set to `home`.
    fn home_env(home: &Path) -> impl Fn(&str) -> Option<String> + use<> {
        let home = home.to_string_lossy().into_owned();
        move |k: &str| (k == "SONARA_HOME").then(|| home.clone())
    }

    fn write_runtime(home: &Path, value: &Value) {
        std::fs::write(home.join("runtime.json"), value.to_string()).expect("write runtime.json");
    }

    fn text(s: &str) -> ShownQuestion {
        ShownQuestion::Text(s.to_string())
    }

    #[test]
    fn home_is_sonara_home_else_localappdata_sonara_else_none() {
        let both = |k: &str| match k {
            "SONARA_HOME" => Some(r"C:\s".to_string()),
            "LOCALAPPDATA" => Some(r"C:\l".to_string()),
            _ => None,
        };
        assert_eq!(sonara_home(&both), Some(PathBuf::from(r"C:\s")));
        let local_only = |k: &str| (k == "LOCALAPPDATA").then(|| r"C:\l".to_string());
        assert_eq!(
            sonara_home(&local_only),
            Some(PathBuf::from(r"C:\l").join("Sonara"))
        );
        let none = |_: &str| None;
        assert_eq!(sonara_home(&none), None);
    }

    #[test]
    fn posts_the_tab_and_the_question_to_the_sonara_in_runtime_json() {
        let (port, server) = fake_sonara(2, "200 OK");
        let dir = tempfile::tempdir().expect("tempdir");
        write_runtime(
            dir.path(),
            &json!({"http_port": port, "token": "secret", "port": 1, "pid": std::process::id()}),
        );
        let env = home_env(dir.path());
        let t = Duration::from_secs(5);
        assert!(post_question_shown("tab-1", &text("Pick a size?"), &env, t));
        assert!(post_question_shown(
            "tab-1",
            &ShownQuestion::Review,
            &env,
            t
        ));
        let got = server.join().expect("server");
        assert_eq!(got.len(), 2);
        assert!(
            got.iter()
                .all(|g| g.request_line == "POST /v1/question_shown HTTP/1.1")
        );
        assert_eq!(got[0].authorization.as_deref(), Some("Bearer secret"));
        let first: Value = serde_json::from_str(&got[0].body).expect("json");
        assert_eq!(first, json!({"host_tab": "tab-1", "text": "Pick a size?"}));
        let second: Value = serde_json::from_str(&got[1].body).expect("json");
        assert_eq!(second, json!({"host_tab": "tab-1", "review": true}));
    }

    #[test]
    fn does_nothing_without_sonara_and_fails_quietly_when_it_is_gone() {
        let t = Duration::from_millis(300);
        let dir = tempfile::tempdir().expect("tempdir");
        let env = home_env(dir.path());
        // No runtime.json at all.
        assert!(!post_question_shown("tab-1", &text("Q?"), &env, t));
        // A runtime.json that is not JSON.
        std::fs::write(dir.path().join("runtime.json"), "{not json").expect("write");
        assert!(!post_question_shown("tab-1", &text("Q?"), &env, t));
        // A port nobody listens on: bind, take the port, drop the listener.
        let probe = TcpListener::bind((LOOPBACK, 0)).expect("bind");
        let gone = probe.local_addr().expect("addr").port();
        drop(probe);
        write_runtime(
            dir.path(),
            &json!({"http_port": gone, "token": "t", "pid": std::process::id()}),
        );
        assert!(!post_question_shown("tab-1", &text("Q?"), &env, t));
        // An empty tab token does no I/O at all.
        assert!(!post_question_shown("", &text("Q?"), &env, t));
    }

    #[test]
    fn a_token_with_a_control_character_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_runtime(
            dir.path(),
            &json!({"http_port": 9, "token": "a\r\nX-Evil: 1"}),
        );
        assert_eq!(read_runtime(dir.path()), None);
        let env = home_env(dir.path());
        assert!(!post_question_shown(
            "tab-1",
            &text("Q?"),
            &env,
            Duration::from_millis(300)
        ));
    }

    #[test]
    fn a_non_200_is_false_and_a_slow_server_times_out() {
        // A 500 answer is not a success.
        let (port, server) = fake_sonara(1, "500 Internal Server Error");
        let dir = tempfile::tempdir().expect("tempdir");
        write_runtime(
            dir.path(),
            &json!({"http_port": port, "token": "t", "pid": std::process::id()}),
        );
        let env = home_env(dir.path());
        assert!(!post_question_shown(
            "tab-1",
            &text("Q?"),
            &env,
            Duration::from_secs(5)
        ));
        server.join().expect("server");

        // A server that accepts and never answers: false within the timeout (plus a second).
        let listener = TcpListener::bind((LOOPBACK, 0)).expect("bind");
        let slow_port = listener.local_addr().expect("addr").port();
        let holder = thread::spawn(move || {
            let (s, _) = listener.accept().expect("accept");
            thread::sleep(Duration::from_millis(2000));
            drop(s);
        });
        write_runtime(
            dir.path(),
            &json!({"http_port": slow_port, "token": "t", "pid": std::process::id()}),
        );
        let timeout = Duration::from_millis(300);
        let started = Instant::now();
        assert!(!post_question_shown("tab-1", &text("Q?"), &env, timeout));
        assert!(started.elapsed() < timeout + Duration::from_secs(1));
        drop(holder);
    }

    /// A listener that records whether anyone ever connected (nonblocking, so the test never hangs).
    fn watched_listener() -> (u16, TcpListener) {
        let listener = TcpListener::bind((LOOPBACK, 0)).expect("bind");
        listener.set_nonblocking(true).expect("nonblocking");
        (listener.local_addr().expect("addr").port(), listener)
    }

    #[test]
    fn a_port_held_by_another_process_gets_neither_the_token_nor_the_question() {
        // The listener is THIS process; runtime.json names another pid (System, 4), as a stale file
        // does after Sonara quit and something else took its port (audit L21).
        let (port, listener) = watched_listener();
        let dir = tempfile::tempdir().expect("tempdir");
        write_runtime(
            dir.path(),
            &json!({"http_port": port, "token": "secret", "pid": 4}),
        );
        let env = home_env(dir.path());
        assert!(!post_question_shown(
            "tab-1",
            &text("Q?"),
            &env,
            Duration::from_millis(500)
        ));
        assert!(listener.accept().is_err(), "the stranger was connected to");
    }

    #[test]
    fn a_runtime_json_without_a_pid_is_not_trusted() {
        let (port, listener) = watched_listener();
        let dir = tempfile::tempdir().expect("tempdir");
        write_runtime(dir.path(), &json!({"http_port": port, "token": "secret"}));
        let env = home_env(dir.path());
        assert!(!post_question_shown(
            "tab-1",
            &text("Q?"),
            &env,
            Duration::from_millis(500)
        ));
        assert!(listener.accept().is_err(), "the stranger was connected to");
    }

    #[test]
    fn only_loopback_is_ever_contacted() {
        let src = include_str!("sonara.rs");
        let code = src
            .split("#[cfg(test)]")
            .next()
            .expect("the module code comes first");
        let ip = regex::Regex::new(r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b").expect("ip");
        for m in ip.find_iter(code) {
            assert_eq!(m.as_str(), "127.0.0.1", "a non-loopback address literal");
        }
        assert!(!code.contains("to_socket_addrs"));
        assert!(!code.contains("ToSocketAddrs"));
        assert!(!code.contains("lookup_host"));
        assert!(code.contains("SocketAddr::from((LOOPBACK"));
    }
}
