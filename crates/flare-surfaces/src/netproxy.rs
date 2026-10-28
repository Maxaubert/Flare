//! Flare's filtering proxy for the networked browsers (issue #97, DNS rebinding): the networked plugins' browser and
//! the mini-apps' networked one send every connection here (`--proxy-server`, with loopback taken out of the built-in
//! bypass), so the wall holds for the address a name resolved to, not only for the name as written. A page's
//! `https:` and `wss:` both arrive as a `CONNECT host:port`; anything else is refused with 403.
//!
//! For each `CONNECT` the proxy checks the host's text, looks the name up itself, keeps only the public addresses
//! (`crate::net::allowed_addresses`) and connects to one of them, so the tunnel is pinned to an address it checked:
//! a resolver that answers a public address first and this PC next has nothing to answer the second time. Then it
//! replies 200 and copies bytes both ways until either side closes or the tunnel has been idle for a while.
//!
//! Std only: a listener on `127.0.0.1` with a port the system picks, started once for the process ([`port`]) and kept
//! for its life, so a browser made again later gets the same arguments. A thread a connection (two while a tunnel is
//! open), a cap on how many at once, a cap on the request head and a deadline for it, and an idle timeout on a tunnel,
//! so a client that stalls or never closes does not keep a thread forever. Any program on this PC can use the port,
//! which reaches only what it could reach anyway.

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// Where a `CONNECT` to a host and port may go: the addresses to try in order, or why it is refused. The proxy never
/// connects to anything this did not give it.
pub type Rule = Arc<dyn Fn(&str, u16) -> Result<Vec<SocketAddr>, &'static str> + Send + Sync>;

/// How much a connection may take.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// The request head's size, its blank line included; a longer one is dropped.
    pub head_bytes: usize,
    /// How long the whole request head may take to arrive.
    pub head_time: Duration,
    /// How long one address may take to answer the connection.
    pub connect_time: Duration,
    /// How long a tunnel may go with no byte either way before both ends are closed.
    pub idle: Duration,
    /// Connections served at once; one more is closed at once.
    pub connections: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            head_bytes: 8 * 1024,
            head_time: Duration::from_secs(10),
            connect_time: Duration::from_secs(10),
            idle: Duration::from_secs(5 * 60),
            connections: 256,
        }
    }
}

/// The rule of the real proxy: [`crate::net::allowed_addresses`] with the system's own lookup.
pub fn public_rule(host: &str, port: u16) -> Result<Vec<SocketAddr>, &'static str> {
    crate::net::allowed_addresses(host, port, |name, port| {
        (name, port).to_socket_addrs().map(Iterator::collect)
    })
}

/// The port of this process's proxy, started on first use with [`public_rule`] and the default limits. The same port
/// every time; an error every time if it could not start, and a browser that would use it must then not be made.
pub fn port() -> Result<u16, String> {
    static PORT: OnceLock<Result<u16, String>> = OnceLock::new();
    PORT.get_or_init(|| {
        let r = start(Arc::new(public_rule), Limits::default());
        match &r {
            Ok(p) => log::info!("network wall proxy on 127.0.0.1:{p}"),
            Err(e) => log::warn!("the network wall proxy did not start: {e}"),
        }
        r.map_err(|e| {
            format!("Flare's network wall could not start ({e}), so no networked page opens")
        })
    })
    .clone()
}

/// Starts a proxy on `127.0.0.1` with a port the system picks and answers that port. It runs until the process ends.
pub fn start(rule: Rule, limits: Limits) -> io::Result<u16> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let port = listener.local_addr()?.port();
    let live = Arc::new(AtomicUsize::new(0));
    std::thread::Builder::new()
        .name("flare-netwall".into())
        .spawn(move || {
            for conn in listener.incoming() {
                let Ok(stream) = conn else {
                    // A failed accept (out of handles, a reset in the queue): wait a moment instead of spinning.
                    std::thread::sleep(Duration::from_millis(20)); // sync-ok: the proxy's own accept thread
                    continue;
                };
                if live.fetch_add(1, Ordering::SeqCst) >= limits.connections {
                    live.fetch_sub(1, Ordering::SeqCst);
                    continue;
                }
                let slot = Slot(Arc::clone(&live));
                let rule = Arc::clone(&rule);
                // A thread that could not start drops its slot and its stream with the closure.
                let _ = std::thread::Builder::new()
                    .name("flare-netwall-conn".into())
                    .stack_size(128 * 1024)
                    .spawn(move || {
                        let _slot = slot;
                        serve(stream, &rule, limits);
                    });
            }
        })?;
    Ok(port)
}

/// One connection counted against [`Limits::connections`] while it lives.
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

const FORBIDDEN: &[u8] =
    b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const BAD_REQUEST: &[u8] =
    b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const BAD_GATEWAY: &[u8] =
    b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const ESTABLISHED: &[u8] = b"HTTP/1.1 200 Connection Established\r\n\r\n";

/// Serves one client: the head, the rule, the pinned connection and the tunnel.
fn serve(mut client: TcpStream, rule: &Rule, limits: Limits) {
    let _ = client.set_nodelay(true);
    let Some((head, early)) = read_head(&mut client, limits) else {
        return;
    };
    let (host, port) = match connect_target(&head) {
        Ok(t) => t,
        Err(answer) => return refuse(client, answer),
    };
    let addrs = match rule(&host, port) {
        Ok(a) => a,
        Err(why) => {
            log::info!("network wall: {host}:{port} refused ({why})");
            return refuse(client, FORBIDDEN);
        }
    };
    let Some(mut upstream) = addrs
        .iter()
        .find_map(|a| TcpStream::connect_timeout(a, limits.connect_time).ok())
    else {
        return refuse(client, BAD_GATEWAY);
    };
    let _ = upstream.set_nodelay(true);
    if client.write_all(ESTABLISHED).is_err()
        || (!early.is_empty() && upstream.write_all(&early).is_err())
    {
        return;
    }
    relay(client, upstream, limits.idle);
}

/// Answers `answer` and closes.
fn refuse(mut client: TcpStream, answer: &[u8]) {
    let _ = client.write_all(answer);
    let _ = client.flush();
    let _ = client.shutdown(Shutdown::Write);
}

/// Reads the request head up to its blank line, within the size and time limits: the head (without the blank line)
/// and any bytes the client sent after it. None when it is too long, too slow, or the client left.
fn read_head(client: &mut TcpStream, limits: Limits) -> Option<(Vec<u8>, Vec<u8>)> {
    let deadline = Instant::now() + limits.head_time;
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 2048];
    loop {
        let left = deadline.checked_duration_since(Instant::now())?;
        if left.is_zero() || client.set_read_timeout(Some(left)).is_err() {
            return None;
        }
        let n = match client.read(&mut chunk) {
            Ok(0) | Err(_) => return None,
            Ok(n) => n,
        };
        let from = buf.len().saturating_sub(3);
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf[from..].windows(4).position(|w| w == b"\r\n\r\n") {
            let end = from + i;
            if end + 4 > limits.head_bytes {
                return None;
            }
            let early = buf.split_off(end + 4);
            buf.truncate(end);
            return Some((buf, early));
        }
        if buf.len() >= limits.head_bytes {
            return None;
        }
    }
}

/// The host and port of a `CONNECT` head, or the answer for anything else: 403 for another method (plain `http:`, a
/// request meant for the proxy itself), 400 for a head that is not HTTP/1 or a target that is not a host and a port.
fn connect_target(head: &[u8]) -> Result<(String, u16), &'static [u8]> {
    let text = std::str::from_utf8(head).map_err(|_| BAD_REQUEST)?;
    let line = text.split("\r\n").next().unwrap_or_default();
    let mut parts = line.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(BAD_REQUEST);
    };
    if !version.starts_with("HTTP/1.") {
        return Err(BAD_REQUEST);
    }
    if method != "CONNECT" {
        return Err(FORBIDDEN);
    }
    crate::net::parse_authority(target).ok_or(BAD_REQUEST)
}

/// When a tunnel last moved a byte, shared by its two directions.
struct Clock {
    start: Instant,
    last_ms: AtomicU64,
}

impl Clock {
    fn touch(&self) {
        let ms = self.start.elapsed().as_millis() as u64;
        self.last_ms.store(ms, Ordering::Relaxed);
    }

    fn idle(&self) -> Duration {
        let ms = self.start.elapsed().as_millis() as u64;
        Duration::from_millis(ms.saturating_sub(self.last_ms.load(Ordering::Relaxed)))
    }
}

/// Copies both ways until both sides have closed, one fails, or nothing moved for `idle`.
fn relay(client: TcpStream, upstream: TcpStream, idle: Duration) {
    let (Ok(client2), Ok(upstream2)) = (client.try_clone(), upstream.try_clone()) else {
        return;
    };
    // Reads wake up this often to look at the shared clock; a write that cannot finish in `idle` fails.
    let tick = idle.min(Duration::from_secs(5));
    for s in [&client, &upstream] {
        let _ = s.set_read_timeout(Some(tick));
        let _ = s.set_write_timeout(Some(idle));
    }
    let clock = Arc::new(Clock {
        start: Instant::now(),
        last_ms: AtomicU64::new(0),
    });
    let back = Arc::clone(&clock);
    let down = std::thread::Builder::new()
        .name("flare-netwall-relay".into())
        .stack_size(128 * 1024)
        .spawn(move || pipe(upstream2, client2, &back, idle));
    if down.is_err() {
        let _ = client.shutdown(Shutdown::Both);
        let _ = upstream.shutdown(Shutdown::Both);
        return;
    }
    pipe(client, upstream, &clock, idle);
    if let Ok(t) = down {
        let _ = t.join(); // sync-ok: a tunnel thread waits for its other half
    }
}

/// One direction of a tunnel. Its end of file closes the other side's writing half; an error or the idle timeout
/// closes both sockets, which ends the other direction too.
fn pipe(mut from: TcpStream, mut to: TcpStream, clock: &Clock, idle: Duration) {
    let mut buf = [0u8; 16 * 1024];
    loop {
        match from.read(&mut buf) {
            Ok(0) => {
                let _ = to.shutdown(Shutdown::Write);
                return;
            }
            Ok(n) => {
                if to.write_all(&buf[..n]).is_err() {
                    break;
                }
                clock.touch();
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                if clock.idle() >= idle {
                    break;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    let _ = from.shutdown(Shutdown::Both);
    let _ = to.shutdown(Shutdown::Both);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_connect_to_a_host_and_port_is_a_target() {
        assert_eq!(
            connect_target(b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443"),
            Ok(("example.com".into(), 443))
        );
        assert_eq!(
            connect_target(b"CONNECT [2606:4700::1111]:443 HTTP/1.1"),
            Ok(("2606:4700::1111".into(), 443))
        );
        assert_eq!(
            connect_target(b"GET http://example.com/ HTTP/1.1"),
            Err(FORBIDDEN)
        );
        assert_eq!(connect_target(b"POST / HTTP/1.1"), Err(FORBIDDEN));
        for bad in [
            &b"CONNECT example.com HTTP/1.1"[..],
            b"CONNECT example.com:443 HTTP/2",
            b"CONNECT  example.com:443 HTTP/1.1",
            b"CONNECT example.com:443",
            b"\xff\xfe",
            b"",
        ] {
            assert_eq!(connect_target(bad), Err(BAD_REQUEST), "{bad:?}");
        }
    }

    #[test]
    fn the_port_is_the_same_every_time() {
        let a = port();
        assert!(a.is_ok(), "{a:?}");
        assert_eq!(a, port());
    }
}
