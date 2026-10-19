//! The one place dictation talks to the network (D4, Task 12): HTTPS downloads through `ureq`, streamed in
//! 64 KiB chunks into the store's sink. Nothing else in this crate calls `ureq`; a scan test holds that.
//! Redirects are `ureq`'s own business (the model host answers every download with one). No retry here or
//! in the store: a failure is reported and the user presses Retry.

use std::io::Read;
use std::time::Duration;

use crate::store::{Failure, Fetch};

/// Downloads with `ureq`.
pub struct UreqFetch {
    agent: ureq::Agent,
}

impl UreqFetch {
    pub fn new() -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(20)))
            // A stalled line gives up instead of hanging a Cancel press for ever.
            .timeout_recv_body(Some(Duration::from_secs(60)))
            .build()
            .into();
        Self { agent }
    }
}

impl Default for UreqFetch {
    fn default() -> Self {
        Self::new()
    }
}

impl Fetch for UreqFetch {
    fn get(&self, url: &str, sink: &mut dyn FnMut(&[u8]) -> bool) -> Result<(), Failure> {
        let response = self.agent.get(url).call().map_err(|e| {
            log::warn!("dictation fetch: {e}");
            Failure::Network
        })?;
        let mut reader = response.into_body().into_reader();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = reader.read(&mut buf).map_err(|e| {
                log::warn!("dictation fetch: read: {e}");
                Failure::Network
            })?;
            if n == 0 {
                return Ok(());
            }
            if !sink(&buf[..n]) {
                return Err(Failure::Cancelled);
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
}
