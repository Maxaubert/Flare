//! The network observers of the walls scenario, in the probe's own process: an mDNS sniffer, a raw TCP accept
//! logger, a logging CONNECT proxy that refuses every tunnel, and a dual-stack HTTP server. Each writes one line per
//! event to `<dir>/<name>.log` (`<unix secs>.<ms> <what>`), and `start.log` once all of them are bound.
//!
//! They were a Python script (`/usr/bin/python3`) until the CI runner showed it never came up there in 5 s: the probe
//! must not lean on a tool the machine may lack or start slowly, so they are threads of the probe now.

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::os::fd::FromRawFd;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone)]
struct Log {
    dir: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl Log {
    fn line(&self, name: &str, what: &str) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join(format!("{name}.log")))
        {
            let _ = writeln!(f, "{}.{:03} {what}", now.as_secs(), now.subsec_millis());
        }
    }
}

/// The bytes as Python's `repr` of a `bytes` would show them, near enough for a log line (`b'...'`).
fn repr(d: &[u8]) -> String {
    let mut s = String::from("b'");
    for &b in d {
        match b {
            b'\\' => s.push_str("\\\\"),
            b'\'' => s.push_str("\\'"),
            b'\n' => s.push_str("\\n"),
            b'\r' => s.push_str("\\r"),
            b'\t' => s.push_str("\\t"),
            0x20..=0x7e => s.push(char::from(b)),
            _ => s.push_str(&format!("\\x{b:02x}")),
        }
    }
    s.push('\'');
    s
}

/// The mDNS port shared with mDNSResponder (SO_REUSEADDR and SO_REUSEPORT before the bind, which std cannot set),
/// joined to the mDNS group.
fn mdns_socket() -> std::io::Result<UdpSocket> {
    // SAFETY: plain socket calls on a descriptor this function owns until it is handed to UdpSocket.
    unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_DGRAM, libc::IPPROTO_UDP);
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let sock = UdpSocket::from_raw_fd(fd);
        let one: libc::c_int = 1;
        for opt in [libc::SO_REUSEADDR, libc::SO_REUSEPORT] {
            if libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                opt,
                (&raw const one).cast(),
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            ) != 0
            {
                return Err(std::io::Error::last_os_error());
            }
        }
        let addr = libc::sockaddr_in {
            sin_len: std::mem::size_of::<libc::sockaddr_in>() as u8,
            sin_family: libc::AF_INET as libc::sa_family_t,
            sin_port: 5353u16.to_be(),
            sin_addr: libc::in_addr { s_addr: 0 },
            sin_zero: [0; 8],
        };
        if libc::bind(
            fd,
            (&raw const addr).cast(),
            std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
        ) != 0
        {
            return Err(std::io::Error::last_os_error());
        }
        sock.join_multicast_v4(&Ipv4Addr::new(224, 0, 0, 251), &Ipv4Addr::UNSPECIFIED)?;
        Ok(sock)
    }
}

fn mdns(log: Log, sock: UdpSocket) {
    let mut buf = [0u8; 9000];
    while let Ok((n, a)) = sock.recv_from(&mut buf) {
        let d = &buf[..n];
        let low = d.to_ascii_lowercase();
        if let Some(i) = low.windows(6).position(|w| w == b"advdns") {
            let end = (i + 30).min(d.len());
            log.line("mdns", &format!("{a} {}", repr(&d[i..end])));
        }
    }
}

fn tcp_accept(log: Log, l: TcpListener) {
    for c in l.incoming() {
        let Ok(mut c) = c else { continue };
        let a = c.peer_addr().map(|a| a.to_string()).unwrap_or_default();
        let _ = c.set_read_timeout(Some(Duration::from_millis(1500)));
        let mut buf = [0u8; 200];
        let first = match c.read(&mut buf) {
            Ok(n) => repr(&buf[..n.min(60)]),
            Err(_) => repr(b"<no bytes>"),
        };
        log.line("tcp", &format!("accept from {a} first={first}"));
    }
}

fn proxy(log: Log, l: TcpListener) {
    for c in l.incoming() {
        let Ok(mut c) = c else { continue };
        let log = log.clone();
        std::thread::spawn(move || {
            let _ = c.set_read_timeout(Some(Duration::from_secs(3)));
            let mut buf = vec![0u8; 2000];
            match c.read(&mut buf) {
                Ok(n) => {
                    let d = &buf[..n];
                    let first = d.split(|&b| b == b'\r').next().unwrap_or_default();
                    log.line("proxy", &String::from_utf8_lossy(first));
                    let _ = c.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n");
                }
                Err(e) => log.line("proxy", &format!("err {e}")),
            }
        });
    }
}

fn http_one(log: &Log, mut c: TcpStream, peer: &str) {
    let _ = c.set_read_timeout(Some(Duration::from_secs(3)));
    let mut buf = vec![0u8; 4000];
    match c.read(&mut buf) {
        Ok(n) => {
            let d = &buf[..n];
            let text = String::from_utf8_lossy(d);
            let mut lines = text.split("\r\n");
            let first = lines.next().unwrap_or_default().to_string();
            let host: Vec<String> = text
                .split("\r\n")
                .filter(|l| l.to_ascii_lowercase().starts_with("host:"))
                .take(1)
                .map(|l| repr(l.as_bytes()))
                .collect();
            log.line("http", &format!("{peer} {first} [{}]", host.join(", ")));
            let _ = c.write_all(
                b"HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
            );
        }
        Err(e) => log.line("http", &format!("err {e}")),
    }
}

fn http(log: Log, l: TcpListener) {
    for c in l.incoming() {
        let Ok(c) = c else { continue };
        let peer = c
            .peer_addr()
            .map(|a| a.ip().to_string())
            .unwrap_or_default();
        let log = log.clone();
        std::thread::spawn(move || http_one(&log, c, &peer));
    }
}

/// An IPv6-only listener on `::` (IPV6_V6ONLY, so the IPv4 one on `0.0.0.0` can share the port).
fn v6_only_listener(port: u16) -> std::io::Result<TcpListener> {
    // SAFETY: plain socket calls on a descriptor this function owns until it is handed to TcpListener.
    unsafe {
        let fd = libc::socket(libc::AF_INET6, libc::SOCK_STREAM, 0);
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let l = TcpListener::from_raw_fd(fd);
        let one: libc::c_int = 1;
        for (level, opt) in [
            (libc::SOL_SOCKET, libc::SO_REUSEADDR),
            (libc::IPPROTO_IPV6, libc::IPV6_V6ONLY),
        ] {
            if libc::setsockopt(
                fd,
                level,
                opt,
                (&raw const one).cast(),
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            ) != 0
            {
                return Err(std::io::Error::last_os_error());
            }
        }
        let mut addr: libc::sockaddr_in6 = std::mem::zeroed();
        addr.sin6_len = std::mem::size_of::<libc::sockaddr_in6>() as u8;
        addr.sin6_family = libc::AF_INET6 as libc::sa_family_t;
        addr.sin6_port = port.to_be();
        if libc::bind(
            fd,
            (&raw const addr).cast(),
            std::mem::size_of::<libc::sockaddr_in6>() as libc::socklen_t,
        ) != 0
            || libc::listen(fd, 50) != 0
        {
            return Err(std::io::Error::last_os_error());
        }
        Ok(l)
    }
}

/// Binds every observer and starts its thread; what could not be bound is said in `start.log` (an observer that is
/// not listening would make an empty log prove nothing, so the walls checks' own self-tests catch it too). The
/// threads live as long as the probe.
pub fn start(dir: &Path) -> std::io::Result<()> {
    let log = Log {
        dir: dir.to_path_buf(),
        lock: Arc::default(),
    };
    let tcp = TcpListener::bind((Ipv4Addr::LOCALHOST, super::pages::OBS_TCP))?;
    let prx = TcpListener::bind((Ipv4Addr::LOCALHOST, super::pages::OBS_PROXY))?;
    let h4 = TcpListener::bind(SocketAddr::from((
        Ipv4Addr::UNSPECIFIED,
        super::pages::OBS_HTTP,
    )))?;
    let h6 = v6_only_listener(super::pages::OBS_HTTP);
    let md = mdns_socket();
    let mut notes = Vec::new();
    {
        let log = log.clone();
        std::thread::spawn(move || tcp_accept(log, tcp));
    }
    {
        let log = log.clone();
        std::thread::spawn(move || proxy(log, prx));
    }
    {
        let log = log.clone();
        std::thread::spawn(move || http(log, h4));
    }
    match h6 {
        Ok(l) => {
            let log = log.clone();
            std::thread::spawn(move || http(log, l));
        }
        Err(e) => notes.push(format!(
            "http on [{}] not bound: {e}",
            Ipv6Addr::UNSPECIFIED
        )),
    }
    match md {
        Ok(s) => {
            let log = log.clone();
            std::thread::spawn(move || mdns(log, s));
        }
        Err(e) => notes.push(format!("mdns not bound: {e}")),
    }
    log.line(
        "start",
        &if notes.is_empty() {
            "up".to_string()
        } else {
            format!("up ({})", notes.join("; "))
        },
    );
    Ok(())
}
