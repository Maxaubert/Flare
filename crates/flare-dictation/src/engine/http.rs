//! Loopback HTTP to the engine over `std::net::TcpStream` (Task 13, D4): a GET for "is it up" and a
//! multipart POST for a pass, hand-written, no crate. Every request is `Connection: close`, so a socket
//! to a server about to be killed is never kept alive. Only 127.0.0.1 is ever dialled.

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

/// How often a waiting read looks up to ask whether it should give up (the server died, a deadline).
const SLICE: Duration = Duration::from_millis(25);

/// An answer: the status code and the body bytes.
#[derive(Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Why a POST gave no answer.
#[derive(Debug)]
pub enum PostError {
    /// A socket error (connect, write or read).
    Io(io::Error),
    /// The deadline passed before the answer was complete.
    Timeout,
    /// The caller's `abort` said stop (the server went away).
    Aborted,
}

fn loopback(port: u16) -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, port))
}

/// `GET /` and the status code, or an error when nobody answers within `timeout`.
pub fn get_status(port: u16, timeout: Duration) -> io::Result<u16> {
    // sync-ok: runs on the engine's worker thread, never the UI thread
    let mut stream = TcpStream::connect_timeout(&loopback(port), timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    stream.write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        if let Some(r) = parse_response(&buf, false) {
            return Ok(r.status);
        }
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) => return Err(e),
        }
    }
    parse_response(&buf, true)
        .map(|r| r.status)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no HTTP status line"))
}

/// POSTs `body` and reads the whole answer. `abort` is asked every 25 ms while waiting; `deadline` ends
/// the wait. The socket is closed on return either way.
pub fn post(
    port: u16,
    path: &str,
    content_type: &str,
    body: &[u8],
    abort: &dyn Fn() -> bool,
    deadline: Instant,
) -> Result<Response, PostError> {
    let left = |d: Instant| d.saturating_duration_since(Instant::now());
    // sync-ok: runs on the engine's worker thread, never the UI thread
    let mut stream = TcpStream::connect_timeout(&loopback(port), left(deadline).max(SLICE))
        .map_err(PostError::Io)?;
    stream
        .set_write_timeout(Some(left(deadline).max(SLICE)))
        .map_err(PostError::Io)?;
    stream
        .set_read_timeout(Some(SLICE))
        .map_err(PostError::Io)?;
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).map_err(PostError::Io)?;
    stream.write_all(body).map_err(PostError::Io)?;
    let mut buf = Vec::new();
    let mut chunk = vec![0u8; 16 * 1024];
    loop {
        if let Some(r) = parse_response(&buf, false) {
            return Ok(r);
        }
        match stream.read(&mut chunk) {
            Ok(0) => {
                return parse_response(&buf, true).ok_or_else(|| {
                    PostError::Io(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "the server closed the connection without an answer",
                    ))
                });
            }
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                if abort() {
                    return Err(PostError::Aborted);
                }
                if Instant::now() >= deadline {
                    return Err(PostError::Timeout);
                }
            }
            Err(e) => return Err(PostError::Io(e)),
        }
    }
}

/// Parses what has arrived. `Some` once the answer is complete: a `Content-Length` body that is all
/// there, a chunked body whose last chunk is there, or (when `eof`) a body that runs to the end.
pub fn parse_response(buf: &[u8], eof: bool) -> Option<Response> {
    let split = find(buf, b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&buf[..split]).into_owned();
    let mut lines = head.split("\r\n");
    let status_line = lines.next()?;
    let status: u16 = status_line.split_whitespace().nth(1)?.parse().ok()?;
    let mut length: Option<usize> = None;
    let mut chunked = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            length = value.parse().ok();
        } else if name.eq_ignore_ascii_case("transfer-encoding")
            && value.to_ascii_lowercase().contains("chunked")
        {
            chunked = true;
        }
    }
    let rest = &buf[split + 4..];
    if chunked {
        return dechunk(rest).map(|body| Response { status, body });
    }
    match length {
        Some(n) if rest.len() >= n => Some(Response {
            status,
            body: rest[..n].to_vec(),
        }),
        Some(_) => None,
        None if eof => Some(Response {
            status,
            body: rest.to_vec(),
        }),
        None => None,
    }
}

/// The body of a complete chunked message, or `None` while its last chunk has not arrived.
fn dechunk(mut rest: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let eol = find(rest, b"\r\n")?;
        let size_text = String::from_utf8_lossy(&rest[..eol]).into_owned();
        let size_text = size_text.split(';').next().unwrap_or("").trim().to_string();
        let size = usize::from_str_radix(&size_text, 16).ok()?;
        rest = &rest[eol + 2..];
        if size == 0 {
            return Some(out);
        }
        if rest.len() < size + 2 {
            return None;
        }
        out.extend_from_slice(&rest[..size]);
        rest = &rest[size + 2..];
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_content_length_body_is_complete_when_all_of_it_is_there() {
        let part = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhel";
        assert_eq!(parse_response(part, false), None);
        let whole = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
        assert_eq!(
            parse_response(whole, false),
            Some(Response {
                status: 200,
                body: b"hello".to_vec()
            })
        );
    }

    #[test]
    fn a_body_with_no_length_runs_to_the_end_of_the_stream() {
        let part = b"HTTP/1.1 404 Not Found\r\n\r\nnope";
        assert_eq!(parse_response(part, false), None);
        assert_eq!(parse_response(part, true).map(|r| r.status), Some(404));
    }

    #[test]
    fn a_chunked_body_is_joined_when_its_last_chunk_arrives() {
        let part = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n";
        assert_eq!(parse_response(part, false), None);
        let whole =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n";
        assert_eq!(
            parse_response(whole, false),
            Some(Response {
                status: 200,
                body: b"abcde".to_vec()
            })
        );
    }

    #[test]
    fn a_missing_status_line_is_not_an_answer() {
        assert_eq!(parse_response(b"garbage\r\n\r\n", true), None);
        assert_eq!(parse_response(b"HTTP/1.1 200 OK\r\n", true), None);
    }
}
