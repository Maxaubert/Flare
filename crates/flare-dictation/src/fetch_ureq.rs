//! The one place dictation talks to the network (D4, Task 12): HTTPS downloads through `ureq`, streamed in
//! 64 KiB chunks into the store's sink. Nothing else in this crate calls `ureq`; a scan test holds that.
//! Redirects are `ureq`'s own business (the model host answers every download with one). No retry here or
//! in the store: a failure is reported and the user presses Retry.
//!
//! Two rules about waiting. There is NO total deadline on the body (ureq's `timeout_recv_body` is one: a
//! 3 GB model on an ordinary line would never finish); a stall is an IDLE limit instead, measured from the
//! last byte. And a blocked read cannot be interrupted from outside, so the blocking part (the request and
//! every read) runs on a helper thread and this thread only waits on a channel in short slices, pinging
//! the sink with an EMPTY slice each time so the store sees a Cancel press at once. A cancelled or stalled
//! helper is abandoned: it ends when its socket does (at the latest at the 12 hour ceiling below).

use std::io::Read;
use std::sync::mpsc::{RecvTimeoutError, sync_channel};
use std::time::{Duration, Instant};

use crate::store::{Failure, Fetch};

/// No byte for this long is a dead line.
const IDLE_LIMIT: Duration = Duration::from_secs(60);
/// How often the waiting side looks up to ask the sink about a cancel.
const POLL: Duration = Duration::from_millis(100);
/// Only so an abandoned helper cannot outlive a day-long stall; no honest download takes this long.
const CEILING: Duration = Duration::from_secs(12 * 60 * 60);

/// Downloads with `ureq`.
pub struct UreqFetch {
    agent: ureq::Agent,
    idle: Duration,
    poll: Duration,
}

impl UreqFetch {
    pub fn new() -> Self {
        Self::with_timing(IDLE_LIMIT, POLL)
    }

    /// `idle`: how long without a byte before giving up; `poll`: how often the sink is asked about a cancel.
    pub fn with_timing(idle: Duration, poll: Duration) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(20)))
            .timeout_global(Some(CEILING))
            .build()
            .into();
        Self { agent, idle, poll }
    }
}

impl Default for UreqFetch {
    fn default() -> Self {
        Self::new()
    }
}

impl Fetch for UreqFetch {
    fn get(&self, url: &str, sink: &mut dyn FnMut(&[u8]) -> bool) -> Result<(), Failure> {
        // Two chunks in flight at most: the helper waits for this side, so memory stays bounded.
        let (tx, rx) = sync_channel::<Result<Vec<u8>, Failure>>(2);
        let agent = self.agent.clone();
        let url = url.to_string();
        std::thread::spawn(move || {
            let response = match agent.get(&url).call() {
                Ok(r) => r,
                Err(e) => {
                    log::warn!("dictation fetch: {e}");
                    let _ = tx.send(Err(Failure::Network));
                    return;
                }
            };
            let mut reader = response.into_body().into_reader();
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => return, // the channel closing is the end of the body
                    Ok(n) => {
                        if tx.send(Ok(buf[..n].to_vec())).is_err() {
                            return; // abandoned
                        }
                    }
                    Err(e) => {
                        log::warn!("dictation fetch: read: {e}");
                        let _ = tx.send(Err(Failure::Network));
                        return;
                    }
                }
            }
        });
        let mut last_byte = Instant::now();
        loop {
            match rx.recv_timeout(self.poll) {
                Ok(Ok(chunk)) => {
                    last_byte = Instant::now();
                    if !sink(&chunk) {
                        return Err(Failure::Cancelled);
                    }
                }
                Ok(Err(f)) => return Err(f),
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
                Err(RecvTimeoutError::Timeout) => {
                    if !sink(&[]) {
                        return Err(Failure::Cancelled);
                    }
                    if last_byte.elapsed() >= self.idle {
                        log::warn!("dictation fetch: no data for {:?}", self.idle);
                        return Err(Failure::Network);
                    }
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

    /// A loopback server that answers one request with `reply`, then closes.
    fn serve_once(reply: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                let _ = s.write_all(&reply);
            }
        });
        format!("http://127.0.0.1:{port}/x")
    }

    #[test]
    fn streams_a_loopback_body_in_chunks_and_stops_when_the_sink_says_so() {
        let body = vec![7u8; 200_000];
        let mut reply = format!(
            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        reply.extend_from_slice(&body);
        let url = serve_once(reply.clone());
        let mut got = 0usize;
        UreqFetch::new()
            .get(&url, &mut |c| {
                got += c.len();
                true
            })
            .expect("get");
        assert_eq!(got, body.len());

        let url = serve_once(reply);
        let mut calls = 0;
        let r = UreqFetch::new().get(&url, &mut |_| {
            calls += 1;
            false
        });
        assert_eq!(r, Err(Failure::Cancelled));
        assert_eq!(calls, 1);
    }

    #[test]
    fn answers_network_for_a_404() {
        let url = serve_once(
            b"HTTP/1.1 404 Not Found\r\ncontent-length: 2\r\nconnection: close\r\n\r\nno".to_vec(),
        );
        let r = UreqFetch::new().get(&url, &mut |_| true);
        assert_eq!(r, Err(Failure::Network));
    }

    /// A loopback server for one request: `script` runs on the accepted socket after the request is read.
    fn serve_script(script: impl FnOnce(&mut std::net::TcpStream) + Send + 'static) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                script(&mut s);
            }
        });
        format!("http://127.0.0.1:{port}/x")
    }

    fn quick() -> UreqFetch {
        UreqFetch::with_timing(Duration::from_millis(600), Duration::from_millis(20))
    }

    #[test]
    fn a_slow_but_steady_body_longer_than_the_idle_limit_still_succeeds() {
        // 12 chunks, 150 ms apart: 1.8 s in all, three times the 600 ms idle limit. A total body
        // deadline (ureq's timeout_recv_body) would have cut this off; only a stall may.
        let url = serve_script(|s| {
            let _ = s.write_all(
                b"HTTP/1.1 200 OK\r\ncontent-length: 12000\r\nconnection: close\r\n\r\n",
            );
            for _ in 0..12 {
                let _ = s.write_all(&[1u8; 1000]);
                let _ = s.flush();
                std::thread::sleep(Duration::from_millis(150));
            }
        });
        let mut got = 0usize;
        let t = std::time::Instant::now();
        quick()
            .get(&url, &mut |c| {
                got += c.len();
                true
            })
            .expect("get");
        assert_eq!(got, 12_000);
        assert!(
            t.elapsed() > Duration::from_millis(1500),
            "it really was slow"
        );
    }

    #[test]
    fn a_body_that_stalls_gives_up_after_the_idle_limit() {
        let url = serve_script(|s| {
            let _ = s
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 9000\r\nconnection: close\r\n\r\n");
            let _ = s.write_all(&[1u8; 1000]);
            let _ = s.flush();
            std::thread::sleep(Duration::from_secs(20));
        });
        let t = std::time::Instant::now();
        let r = quick().get(&url, &mut |_| true);
        assert_eq!(r, Err(Failure::Network));
        assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    }

    #[test]
    fn a_server_that_never_answers_gives_up_after_the_idle_limit() {
        let url = serve_script(|_| std::thread::sleep(Duration::from_secs(20)));
        let t = std::time::Instant::now();
        let r = quick().get(&url, &mut |_| true);
        assert_eq!(r, Err(Failure::Network));
        assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    }

    /// The real fetch pings the sink with an empty slice while it waits, so a cancel is seen at once
    /// even though no byte is coming (the idle limit here is a whole minute).
    fn cancel_during(url: String) {
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        let fetch = UreqFetch::with_timing(Duration::from_secs(60), Duration::from_millis(20));
        let mut pinged = false;
        let t = std::time::Instant::now();
        let r = fetch.get(&url, &mut |c| {
            if c.is_empty() {
                pinged = true;
            }
            !cancel.load(std::sync::atomic::Ordering::SeqCst)
        });
        assert_eq!(r, Err(Failure::Cancelled));
        assert!(pinged, "the sink was asked with an empty slice");
        assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    }

    #[test]
    fn a_cancel_is_seen_at_once_while_the_body_stalls() {
        cancel_during(serve_script(|s| {
            let _ = s
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 9000\r\nconnection: close\r\n\r\n");
            let _ = s.write_all(&[1u8; 1000]);
            let _ = s.flush();
            std::thread::sleep(Duration::from_secs(20));
        }));
    }

    #[test]
    fn a_cancel_is_seen_at_once_while_the_server_never_answers() {
        cancel_during(serve_script(|_| {
            std::thread::sleep(Duration::from_secs(20))
        }));
    }
}

/// The e2e's stand-in for the network (`FLARE_E2E_FETCH=stall`, M8 Task 30): zeros from memory, one chunk every
/// few milliseconds, and never a byte from the network. It runs until the sink says stop (a Cancel), which is
/// the only way a scenario's download ends; it has no end of its own.
pub struct StallFetch {
    chunk: usize,
    every: Duration,
}

impl StallFetch {
    /// The app's stand-in: 64 KiB every 250 ms.
    pub fn new() -> Self {
        Self::with_timing(64 * 1024, Duration::from_millis(250))
    }

    pub fn with_timing(chunk: usize, every: Duration) -> Self {
        Self { chunk, every }
    }
}

impl Default for StallFetch {
    fn default() -> Self {
        Self::new()
    }
}

impl Fetch for StallFetch {
    fn get(&self, _url: &str, sink: &mut dyn FnMut(&[u8]) -> bool) -> Result<(), Failure> {
        let zeros = vec![0u8; self.chunk];
        loop {
            std::thread::sleep(self.every);
            if !sink(&zeros) {
                return Err(Failure::Cancelled);
            }
        }
    }
}

#[cfg(test)]
mod stall_tests {
    use super::*;

    #[test]
    fn the_stall_serves_zeros_slowly_and_ends_only_when_the_sink_says_stop() {
        let f = StallFetch::with_timing(4096, Duration::from_millis(1));
        let mut got = 0usize;
        let mut calls = 0;
        let r = f.get("https://never.invalid/model.bin", &mut |c| {
            assert!(c.iter().all(|b| *b == 0), "the stand-in serves zeros");
            got += c.len();
            calls += 1;
            calls < 3
        });
        assert_eq!(r, Err(Failure::Cancelled));
        assert_eq!(got, 3 * 4096);
    }

    #[test]
    fn the_app_stand_in_is_64_kib_per_quarter_second() {
        let f = StallFetch::new();
        assert_eq!(f.chunk, 64 * 1024);
        assert_eq!(f.every, Duration::from_millis(250));
    }
}
