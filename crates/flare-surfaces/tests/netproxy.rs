//! Flare's filtering proxy (issue #97) against a loopback echo server. The real rule refuses loopback, so the tests
//! give the proxy a rule that lets one test name (`echo.test`) reach the echo server and sends every other name through
//! the real check (`net::allowed_addresses`) with a resolver that answers the echo server's loopback address: a
//! public-looking name pointed at this PC.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use flare_surfaces::net;
use flare_surfaces::netproxy::{self, Limits, Rule};

/// An echo server on loopback; counts the connections it accepted.
fn echo_server() -> (SocketAddr, Arc<AtomicUsize>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let accepted = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&accepted);
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { continue };
            count.fetch_add(1, Ordering::SeqCst);
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while let Ok(n) = s.read(&mut buf) {
                    if n == 0 || s.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
            });
        }
    });
    (addr, accepted)
}

/// A proxy whose rule lets `echo.test` reach `echo` and checks every other name for real, with a resolver that
/// points it at `echo`; counts the lookups.
fn proxy(echo: SocketAddr, limits: Limits) -> (u16, Arc<AtomicUsize>) {
    let lookups = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&lookups);
    let rule: Rule = Arc::new(move |host: &str, port: u16| {
        if host == "echo.test" && port == echo.port() {
            return Ok(vec![echo]);
        }
        net::allowed_addresses(host, port, |_, _| {
            seen.fetch_add(1, Ordering::SeqCst);
            Ok(vec![echo])
        })
    });
    (netproxy::start(rule, limits).unwrap(), lookups)
}

fn limits() -> Limits {
    Limits {
        head_time: Duration::from_secs(2),
        idle: Duration::from_secs(2),
        ..Limits::default()
    }
}

fn client(port: u16) -> TcpStream {
    let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s
}

/// Everything the proxy sends until it closes.
fn read_all(s: &mut TcpStream) -> String {
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    String::from_utf8_lossy(&out).into_owned()
}

/// Reads the proxy's answer head (up to its blank line).
fn read_answer(s: &mut TcpStream) -> String {
    let mut out = Vec::new();
    let mut b = [0u8; 1];
    while !out.ends_with(b"\r\n\r\n") {
        match s.read(&mut b) {
            Ok(1) => out.push(b[0]),
            _ => break,
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[test]
fn a_connect_to_an_allowed_host_relays_both_ways() {
    let (echo, accepted) = echo_server();
    let (port, _) = proxy(echo, limits());
    let mut c = client(port);
    let target = format!("echo.test:{}", echo.port());
    write!(
        c,
        "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Connection: keep-alive\r\n\r\n"
    )
    .unwrap();
    assert!(read_answer(&mut c).starts_with("HTTP/1.1 200 "));
    for msg in [&b"hello through the wall"[..], b"and back again"] {
        c.write_all(msg).unwrap();
        let mut got = vec![0u8; msg.len()];
        c.read_exact(&mut got).unwrap();
        assert_eq!(got, msg);
    }
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
}

#[test]
fn bytes_sent_right_after_the_head_are_not_lost() {
    let (echo, _) = echo_server();
    let (port, _) = proxy(echo, limits());
    let mut c = client(port);
    let target = format!("echo.test:{}", echo.port());
    write!(c, "CONNECT {target} HTTP/1.1\r\n\r\nearly").unwrap();
    assert!(read_answer(&mut c).starts_with("HTTP/1.1 200 "));
    let mut got = [0u8; 5];
    c.read_exact(&mut got).unwrap();
    assert_eq!(&got, b"early");
}

#[test]
fn any_other_method_is_refused_and_nothing_is_dialled() {
    let (echo, accepted) = echo_server();
    let (port, lookups) = proxy(echo, limits());
    let mut c = client(port);
    write!(
        c,
        "GET http://echo.test:{}/ HTTP/1.1\r\nHost: echo.test\r\n\r\n",
        echo.port()
    )
    .unwrap();
    assert!(read_all(&mut c).starts_with("HTTP/1.1 403 "));
    assert_eq!(lookups.load(Ordering::SeqCst), 0);
    assert_eq!(accepted.load(Ordering::SeqCst), 0);
}

#[test]
fn a_name_pointed_at_this_pc_or_a_local_address_is_refused_and_nothing_is_dialled() {
    let (echo, accepted) = echo_server();
    let (port, lookups) = proxy(echo, limits());
    let p = echo.port();
    for target in [
        format!("rebind.example:{p}"),
        format!("127.0.0.1:{p}"),
        format!("[::ffff:7f00:1]:{p}"),
        format!("[::1]:{p}"),
        format!("localhost:{p}"),
        format!("flare-mod-notes.local:{p}"),
        format!("10.0.0.1:{p}"),
    ] {
        let mut c = client(port);
        write!(c, "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").unwrap();
        assert!(read_all(&mut c).starts_with("HTTP/1.1 403 "), "{target}");
    }
    // Only `rebind.example` was looked up (and its loopback answer thrown away); the rest were refused as written.
    assert_eq!(lookups.load(Ordering::SeqCst), 1);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        accepted.load(Ordering::SeqCst),
        0,
        "nothing reached the echo server"
    );
}

#[test]
fn a_bad_target_is_a_bad_request() {
    let (echo, accepted) = echo_server();
    let (port, _) = proxy(echo, limits());
    let mut c = client(port);
    write!(c, "CONNECT echo.test HTTP/1.1\r\n\r\n").unwrap();
    assert!(read_all(&mut c).starts_with("HTTP/1.1 400 "));
    assert_eq!(accepted.load(Ordering::SeqCst), 0);
}

#[test]
fn an_oversized_head_is_closed_without_an_answer() {
    let (echo, accepted) = echo_server();
    let (port, _) = proxy(echo, limits());
    let mut c = client(port);
    let target = format!("echo.test:{}", echo.port());
    let mut head = format!("CONNECT {target} HTTP/1.1\r\n");
    while head.len() < 9 * 1024 {
        head.push_str("X-Pad: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\r\n");
    }
    // The proxy may close while this is still being written.
    let _ = c.write_all(head.as_bytes());
    let _ = c.write_all(b"\r\n");
    let got = read_all(&mut c);
    assert!(!got.contains("200"), "{got}");
    assert_eq!(accepted.load(Ordering::SeqCst), 0);
}

#[test]
fn a_slow_client_is_closed_when_its_head_is_late() {
    let (echo, accepted) = echo_server();
    let (port, _) = proxy(
        echo,
        Limits {
            head_time: Duration::from_millis(300),
            ..limits()
        },
    );
    let mut c = client(port);
    c.write_all(b"CONNECT echo.test:").unwrap();
    let t = Instant::now();
    let got = read_all(&mut c);
    assert!(got.is_empty(), "{got}");
    assert!(
        t.elapsed() < Duration::from_secs(5),
        "closed by the deadline"
    );
    assert_eq!(accepted.load(Ordering::SeqCst), 0);
}

/// A pause shorter than the idle time, during which the relay's reads time out and are tried again, loses nothing.
#[test]
fn a_tunnel_quiet_for_less_than_the_idle_time_keeps_working() {
    let (echo, _) = echo_server();
    let (port, _) = proxy(
        echo,
        Limits {
            idle: Duration::from_millis(1200),
            ..limits()
        },
    );
    let mut c = client(port);
    let target = format!("echo.test:{}", echo.port());
    write!(c, "CONNECT {target} HTTP/1.1\r\n\r\n").unwrap();
    assert!(read_answer(&mut c).starts_with("HTTP/1.1 200 "));
    for msg in [&b"before the pause"[..], b"after the pause"] {
        c.write_all(msg).unwrap();
        let mut got = vec![0u8; msg.len()];
        c.read_exact(&mut got).unwrap();
        assert_eq!(got, msg);
        // Several of the relay's read ticks (a quarter of the idle time) pass with nothing to read.
        std::thread::sleep(Duration::from_millis(700));
    }
}

#[test]
fn an_idle_tunnel_is_closed() {
    let (echo, _) = echo_server();
    let (port, _) = proxy(
        echo,
        Limits {
            idle: Duration::from_millis(300),
            ..limits()
        },
    );
    let mut c = client(port);
    let target = format!("echo.test:{}", echo.port());
    write!(c, "CONNECT {target} HTTP/1.1\r\n\r\n").unwrap();
    assert!(read_answer(&mut c).starts_with("HTTP/1.1 200 "));
    let t = Instant::now();
    let rest = read_all(&mut c);
    assert!(rest.is_empty(), "{rest}");
    assert!(
        t.elapsed() < Duration::from_secs(5),
        "closed after the idle time"
    );
}
