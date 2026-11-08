//! A tiny loopback HTTP server for the spike's test pages, and a UDP listener that tells whether WebRTC
//! got a packet out. Every request is logged (host and path), so a test can prove that a sandboxed page
//! sent nothing.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::pages;

#[derive(Clone, Default)]
pub struct Log(Arc<Mutex<Vec<String>>>);

impl Log {
    pub fn push(&self, s: String) {
        if let Ok(mut v) = self.0.lock() {
            v.push(s);
        }
    }

    pub fn all(&self) -> Vec<String> {
        self.0.lock().map(|v| v.clone()).unwrap_or_default()
    }
}

pub struct Server {
    pub port: u16,
    pub udp_port: u16,
    pub log: Log,
    pub udp_log: Log,
}

pub fn start() -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let log = Log::default();
    let l2 = log.clone();
    std::thread::spawn(move || {
        for s in listener.incoming().flatten() {
            let l3 = l2.clone();
            std::thread::spawn(move || handle(s, port, &l3));
        }
    });
    let udp = UdpSocket::bind("127.0.0.1:0").expect("udp");
    let udp_port = udp.local_addr().expect("addr").port();
    let udp_log = Log::default();
    let u2 = udp_log.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 2048];
        while let Ok((n, from)) = udp.recv_from(&mut buf) {
            u2.push(format!("{n} bytes from {from}"));
        }
    });
    Server {
        port,
        udp_port,
        log,
        udp_log,
    }
}

fn handle(mut s: TcpStream, port: u16, log: &Log) {
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let mut r = BufReader::new(match s.try_clone() {
        Ok(c) => c,
        Err(_) => return,
    });
    let mut line = String::new();
    if r.read_line(&mut line).is_err() {
        return;
    }
    let mut host = String::new();
    let mut upgrade = false;
    let mut len = 0usize;
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).is_err() || h.trim().is_empty() {
            break;
        }
        let lower = h.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("host:") {
            host = v.trim().to_string();
        }
        if lower.starts_with("upgrade:") {
            upgrade = true;
        }
        if let Some(v) = lower.strip_prefix("content-length:") {
            len = v.trim().parse().unwrap_or(0);
        }
    }
    if len > 0 {
        let mut body = vec![0u8; len.min(1 << 20)];
        let _ = r.read_exact(&mut body);
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("/").to_string();
    log.push(format!(
        "{method} {host}{path}{}",
        if upgrade { " (upgrade)" } else { "" }
    ));
    let route = path.split('?').next().unwrap_or("/");
    match route {
        "/dl.bin" => {
            // 4 MiB in 64 KiB pieces with small pauses, so progress can be watched.
            let total = 4 * 1024 * 1024;
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=\"spike-download.bin\"\r\nContent-Length: {total}\r\nConnection: close\r\n\r\n"
            );
            if s.write_all(head.as_bytes()).is_err() {
                return;
            }
            let chunk = vec![0x5Au8; 64 * 1024];
            for _ in 0..(total / chunk.len()) {
                if s.write_all(&chunk).is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(15));
            }
        }
        "/slow" => {
            std::thread::sleep(Duration::from_millis(1500));
            reply(&mut s, 200, "text/html", pages::SLOW);
        }
        "/status404" => reply(&mut s, 404, "text/html", "<title>nope</title>not here"),
        "/main" => reply(&mut s, 200, "text/html", &pages::main(port)),
        "/same" => reply(&mut s, 200, "text/html", pages::SAME),
        "/cross" => reply(&mut s, 200, "text/html", pages::CROSS),
        "/page2" => reply(&mut s, 200, "text/html", pages::PAGE2),
        "/setcookie" => {
            let body = "<title>cookie</title>set";
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nSet-Cookie: spike=panel; Path=/; Max-Age=3600\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = s.write_all(head.as_bytes());
            let _ = s.write_all(body.as_bytes());
        }
        r if crate::adv::page(r).is_some() => {
            let (h, body) = crate::adv::page(r).unwrap_or_default();
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n{h}Content-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = s.write_all(head.as_bytes());
            let _ = s.write_all(body.as_bytes());
        }
        _ => reply(&mut s, 200, "text/plain", "ok"),
    }
}

fn reply(s: &mut TcpStream, code: u16, mime: &str, body: &str) {
    let reason = if code == 200 { "OK" } else { "Not Found" };
    let head = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: {mime}; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = s.write_all(head.as_bytes());
    let _ = s.write_all(body.as_bytes());
}
