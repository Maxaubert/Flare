//! A loopback HTTP server for the probe's pages (the spike's `server.rs`, cut down): one thread per connection,
//! `GET` only, `Connection: close`. Every request is logged (path), so a scenario can tell what the page fetched.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::pages;

pub struct Server {
    pub port: u16,
    pub log: Arc<Mutex<Vec<String>>>,
}

impl Server {
    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    pub fn requests(&self) -> Vec<String> {
        self.log.lock().map(|v| v.clone()).unwrap_or_default()
    }
}

pub fn start() -> Result<Server, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("bind: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let log = Arc::new(Mutex::new(Vec::new()));
    let l2 = Arc::clone(&log);
    std::thread::spawn(move || {
        for s in listener.incoming().flatten() {
            let l3 = Arc::clone(&l2);
            std::thread::spawn(move || handle(s, &l3));
        }
    });
    Ok(Server { port, log })
}

fn body_of(path: &str) -> Option<String> {
    let path = path.split('?').next().unwrap_or(path);
    Some(match path {
        "/" | "/main" => pages::main(),
        "/same" => pages::SAME.to_string(),
        "/page2" => pages::PAGE2.to_string(),
        "/paint" => pages::PAINT.to_string(),
        "/runaway" => pages::RUNAWAY.to_string(),
        "/trap" => pages::TRAP.to_string(),
        _ => return None,
    })
}

fn handle(mut s: TcpStream, log: &Arc<Mutex<Vec<String>>>) {
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let Ok(clone) = s.try_clone() else { return };
    let mut r = BufReader::new(clone);
    let mut line = String::new();
    if r.read_line(&mut line).is_err() {
        return;
    }
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).is_err() || h.trim().is_empty() {
            break;
        }
    }
    let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
    if let Ok(mut v) = log.lock() {
        v.push(path.clone());
    }
    let (status, body) = match body_of(&path) {
        Some(b) => ("200 OK", b),
        None => (
            "404 Not Found",
            "<!doctype html><title>Missing</title>not here".to_string(),
        ),
    };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = s.write_all(head.as_bytes());
    let _ = s.write_all(body.as_bytes());
}
