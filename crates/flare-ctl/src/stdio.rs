//! A [`Wire`] over two byte streams: the serve process's own stdin and stdout (`flare --serve-remote`, started by
//! sshd), or a child's (`ssh <host> <flare> --serve-remote`, started by the viewer). Remote sessions PR 1
//! (2026-11-12, `docs/superpowers/plans/2026-11-12-remote-sessions.md`).
//!
//! A stdio stream has no "how much is ready" probe that works for every kind of handle (a pipe, a socket, a file), so
//! a reader thread does the blocking `read` into a queue, and [`Wire::poll_read`] only drains that queue: it never
//! waits. The queue holds at most [`QUEUE_CAP`] bytes; past that the reader thread waits for the line reader to catch
//! up, so a peer that writes faster than it is read never grows this side's memory. Writes go straight to the stream
//! under a lock and are flushed, one whole line at a time.
//!
//! **Stdout carries only protocol lines (rule 277).** In serve mode this wire is the only writer to stdout: a log
//! line, a panic message or a stray `println!` there would be read by the viewer as a broken message. Logs go to
//! stderr, which ssh carries apart.
//!
//! **Binary on both systems.** [`StdioWire::from_process`] reads and writes a duplicate of the process's own stdin
//! and stdout handles as plain files: on Windows that is `ReadFile` and `WriteFile` with no console decoding and no
//! newline translation (Rust never puts a handle in the C runtime's text mode; this is what `_setmode(_O_BINARY)` gives
//! a C program), and on macOS a plain file descriptor. A child's pipes (`ChildStdin`, `ChildStdout`) are binary already.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::process::{ChildStdin, ChildStdout};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use crate::framing::Wire;

/// The most bytes the reader thread keeps that the line reader has not taken yet.
pub const QUEUE_CAP: usize = 1024 * 1024;
/// How much the reader thread asks the stream for at once.
const CHUNK: usize = 64 * 1024;

#[derive(Default)]
struct Queue {
    data: VecDeque<u8>,
    /// The stream ended (or a read failed): once `data` is drained, `poll_read` says closed.
    ended: bool,
    /// The wire was dropped: the reader thread stops at its next turn instead of waiting for room.
    dropped: bool,
}

#[derive(Default)]
struct Inbox {
    queue: Mutex<Queue>,
    /// Signalled when the line reader took bytes, or the wire was dropped.
    room: Condvar,
}

impl Inbox {
    fn lock(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A [`Wire`] over a reader and a writer, read by a thread of its own (see the module text).
pub struct StdioWire {
    inbox: Arc<Inbox>,
    writer: Mutex<Box<dyn Write + Send>>,
}

impl StdioWire {
    /// A wire that reads `reader` on a thread of its own and writes `writer`. Fails only when the thread cannot start.
    pub fn new<R, W>(reader: R, writer: W) -> io::Result<StdioWire>
    where
        R: Read + Send + 'static,
        W: Write + Send + 'static,
    {
        let inbox = Arc::new(Inbox::default());
        let feed = inbox.clone();
        std::thread::Builder::new()
            .name("flare-stdio-read".into())
            .spawn(move || pump(reader, &feed))?;
        Ok(StdioWire {
            inbox,
            writer: Mutex::new(Box::new(writer)),
        })
    }

    /// The viewer's end of a link: the child's stdout is what the host answers, its stdin what the viewer sends.
    pub fn from_child(stdin: ChildStdin, stdout: ChildStdout) -> io::Result<StdioWire> {
        StdioWire::new(stdout, stdin)
    }

    /// The serve process's end: its own stdin and stdout, as raw binary files (see the module text). Fails when the
    /// process has no such handles (a GUI process started with none).
    pub fn from_process() -> io::Result<StdioWire> {
        let input = raw_file(&io::stdin())?;
        let output = raw_file(&io::stdout())?;
        StdioWire::new(input, output)
    }
}

/// A file over a duplicate of a standard handle, so reads and writes skip Rust's console handling and buffers.
#[cfg(windows)]
fn raw_file(h: &impl std::os::windows::io::AsHandle) -> io::Result<std::fs::File> {
    Ok(std::fs::File::from(h.as_handle().try_clone_to_owned()?))
}

/// A file over a duplicate of a standard descriptor, so reads and writes skip Rust's buffers.
#[cfg(unix)]
fn raw_file(h: &impl std::os::fd::AsFd) -> io::Result<std::fs::File> {
    Ok(std::fs::File::from(h.as_fd().try_clone_to_owned()?))
}

/// The reader thread: blocking reads into the queue until the stream ends, a read fails or the wire is dropped.
fn pump(mut reader: impl Read, inbox: &Inbox) {
    let mut buf = vec![0u8; CHUNK];
    loop {
        let got = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                // A broken stream ends the link like a close does; there is no one to tell but the log (stderr).
                log::debug!("remote link: the stream failed: {e}");
                break;
            }
        };
        let mut q = inbox.lock();
        while q.data.len() >= QUEUE_CAP && !q.dropped {
            q = inbox.room.wait(q).unwrap_or_else(|e| e.into_inner());
        }
        if q.dropped {
            return;
        }
        q.data.extend(&buf[..got]);
    }
    inbox.lock().ended = true;
}

impl Wire for StdioWire {
    fn poll_read(&self, buf: &mut [u8]) -> io::Result<Option<usize>> {
        let mut q = self.inbox.lock();
        if q.data.is_empty() {
            return Ok(if q.ended { None } else { Some(0) });
        }
        let n = buf.len().min(q.data.len());
        for (slot, b) in buf.iter_mut().zip(q.data.drain(..n)) {
            *slot = b;
        }
        drop(q);
        self.inbox.room.notify_all();
        Ok(Some(n))
    }

    fn write_all(&self, bytes: &[u8]) -> io::Result<()> {
        let mut w = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        w.write_all(bytes)?;
        w.flush()
    }
}

impl Drop for StdioWire {
    fn drop(&mut self) {
        // A reader thread waiting for room stops now. One parked in a read stops when its stream ends (the child
        // exits, or the serve process does); it holds nothing but the read end.
        self.inbox.lock().dropped = true;
        self.inbox.room.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framing::{LineReader, ReadError};
    use crate::proto::MAX_LINE;
    use std::sync::atomic::AtomicBool;
    use std::time::{Duration, Instant};

    /// A wire over a fresh OS pipe pair: what the test writes to the returned writer comes out of the wire, and what
    /// the wire writes comes out of the returned reader.
    fn os_wire() -> (StdioWire, io::PipeWriter, io::PipeReader) {
        let (in_r, in_w) = io::pipe().unwrap();
        let (out_r, out_w) = io::pipe().unwrap();
        (StdioWire::new(in_r, out_w).unwrap(), in_w, out_r)
    }

    fn soon() -> Option<Instant> {
        Some(Instant::now() + Duration::from_secs(5))
    }

    fn no_stop() -> AtomicBool {
        AtomicBool::new(false)
    }

    /// Reads from the wire until `want` bytes came, or 5 s passed.
    fn drain(wire: &StdioWire, want: usize) -> Vec<u8> {
        let mut got = Vec::new();
        let mut buf = [0u8; 4096];
        let until = Instant::now() + Duration::from_secs(5);
        while got.len() < want && Instant::now() < until {
            match wire.poll_read(&mut buf).unwrap() {
                Some(0) => std::thread::sleep(Duration::from_millis(2)),
                Some(n) => got.extend_from_slice(&buf[..n]),
                None => break,
            }
        }
        got
    }

    #[test]
    fn a_poll_with_nothing_ready_answers_at_once() {
        let (wire, _keep_open, _out) = os_wire();
        let mut buf = [0u8; 64];
        let t = Instant::now();
        for _ in 0..100 {
            assert_eq!(wire.poll_read(&mut buf).unwrap(), Some(0));
        }
        assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
    }

    #[test]
    fn the_end_of_the_stream_is_none_once_everything_was_read() {
        let (wire, mut w, _out) = os_wire();
        w.write_all(b"last words").unwrap();
        drop(w);
        assert_eq!(drain(&wire, 10), b"last words");
        let mut buf = [0u8; 16];
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            match wire.poll_read(&mut buf).unwrap() {
                None => break,
                Some(0) if Instant::now() < until => std::thread::sleep(Duration::from_millis(2)),
                other => panic!("expected the end, got {other:?}"),
            }
        }
    }

    #[test]
    fn partial_lines_join_and_a_carriage_return_is_stripped() {
        let (wire, mut w, _out) = os_wire();
        let mut r = LineReader::new(MAX_LINE);
        w.write_all(b"hel").unwrap();
        assert!(matches!(
            r.read_line(
                &wire,
                Some(Instant::now() + Duration::from_millis(50)),
                &no_stop()
            ),
            Err(ReadError::Timeout)
        ));
        w.write_all(b"lo\r\nnext\n").unwrap();
        assert_eq!(r.read_line(&wire, soon(), &no_stop()).unwrap(), b"hello");
        assert_eq!(r.read_line(&wire, soon(), &no_stop()).unwrap(), b"next");
    }

    /// The 16 MB cap of the framing holds over a stdio stream too, and the reader thread never holds more than its
    /// queue: the writer is held back until the line reader takes bytes.
    #[test]
    fn a_line_over_16_mb_is_too_long_and_the_queue_stays_capped() {
        let (wire, mut w, _out) = os_wire();
        let writer = std::thread::spawn(move || {
            let chunk = vec![b'x'; 1024 * 1024];
            for _ in 0..=(MAX_LINE / chunk.len()) {
                if w.write_all(&chunk).is_err() {
                    return;
                }
            }
            let _ = w.write_all(b"\n");
        });
        // Nobody reads yet: the queue fills to its cap and stops there.
        let until = Instant::now() + Duration::from_secs(5);
        while wire.inbox.lock().data.len() < QUEUE_CAP && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(50));
        assert!(wire.inbox.lock().data.len() <= QUEUE_CAP + CHUNK);
        let mut r = LineReader::new(MAX_LINE);
        assert!(matches!(
            r.read_line(
                &wire,
                Some(Instant::now() + Duration::from_secs(30)),
                &no_stop()
            ),
            Err(ReadError::TooLong)
        ));
        // Dropping the wire lets the reader thread go, which closes the pipe, which ends the writer.
        drop(wire);
        drop(r);
        writer.join().unwrap();
    }

    #[test]
    fn a_write_reaches_the_stream_whole_and_flushed() {
        let (wire, _in, mut out) = os_wire();
        wire.write_all(b"{\"id\":1}\n").unwrap();
        let mut got = [0u8; 9];
        out.read_exact(&mut got).unwrap();
        assert_eq!(&got, b"{\"id\":1}\n");
    }

    /// The whole link over OS pipes, as `ssh` would carry it: the host prints a banner first, then serves; the viewer's
    /// `remote.hello` skips the banner and gets the remote grant's answer; the link ends when the viewer hangs up.
    #[test]
    fn a_remote_hello_round_trips_over_os_pipes_after_a_banner() {
        use crate::client::{Client, RemoteHelloParams};
        use crate::grant::Grant;
        use crate::method::Method;
        use crate::proto::CtlError;
        use crate::session::{
            CallContext, ConnOrigin, Dispatcher, ServerConfig, Shared, serve_stdio,
        };
        use serde_json::Value;

        struct Host;
        impl Dispatcher for Host {
            fn resolve_token(&self, _: &str) -> Option<Grant> {
                None
            }
            fn resolve_tab_token(&self, _: &str) -> Option<String> {
                None
            }
            fn remote_grant(&self, _: ConnOrigin) -> Option<Grant> {
                Some(Grant::Remote)
            }
            fn call(&self, _: &CallContext, _: Method, _: Value) -> Result<Value, CtlError> {
                Ok(Value::Null)
            }
        }

        let (to_host_r, to_host_w) = io::pipe().unwrap();
        let (to_viewer_r, mut to_viewer_w) = io::pipe().unwrap();
        to_viewer_w.write_all(b"Welcome to the host\r\n").unwrap();
        let host = std::thread::spawn(move || {
            let wire = StdioWire::new(to_host_r, to_viewer_w).unwrap();
            let shared = Shared::new_stdio(
                Arc::new(Host),
                ServerConfig {
                    machine: "host".into(),
                    ..ServerConfig::stdio()
                },
            );
            serve_stdio(&wire, &shared);
        });
        let mut viewer = Client::new(StdioWire::new(to_viewer_r, to_host_w).unwrap());
        let info = viewer
            .remote_hello(
                &RemoteHelloParams {
                    machine: "viewer".into(),
                    flare_version: "1".into(),
                },
                Duration::from_secs(10),
            )
            .unwrap();
        assert_eq!(info.machine, "host");
        assert_eq!(info.capabilities, ["remote.hello"]);
        drop(viewer);
        host.join().unwrap();
    }

    #[test]
    fn a_write_to_a_closed_stream_fails() {
        let (wire, _in, out) = os_wire();
        drop(out);
        assert!(wire.write_all(b"x\n").is_err());
    }
}
