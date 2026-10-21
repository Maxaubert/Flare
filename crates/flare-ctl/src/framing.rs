//! JSON-lines framing over any byte pipe, with a size cap, a deadline and a stop flag.
//!
//! Reads never block in the OS: the reader asks the wire how many bytes are ready and sleeps a few
//! milliseconds when there are none. That is what lets a worker thread honour a deadline, notice a
//! stop request, and lets another thread write to the same pipe handle meanwhile (a synchronous
//! Windows handle serialises its calls, so a parked `ReadFile` would hold every writer out).

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// A duplex byte pipe with a non-blocking "how much is ready" probe.
pub trait Wire {
    /// Reads what is ready right now, without waiting: `Ok(Some(0))` when nothing is, `Ok(None)` once
    /// the peer has closed and everything it wrote has been read. Data written before a close must
    /// still come out (a refusal line is written and the pipe closed straight after).
    fn poll_read(&self, buf: &mut [u8]) -> io::Result<Option<usize>>;
    /// Writes every byte or fails.
    fn write_all(&self, bytes: &[u8]) -> io::Result<()>;
}

#[derive(Debug)]
pub enum ReadError {
    /// The peer closed the pipe (a partial last line is dropped).
    Closed,
    /// A line passed the cap.
    TooLong,
    /// The deadline passed with no complete line.
    Timeout,
    /// The stop flag was raised.
    Stopped,
    Io(io::Error),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::Closed => f.write_str("the connection closed"),
            ReadError::TooLong => f.write_str("a message passed the size limit"),
            ReadError::Timeout => f.write_str("timed out"),
            ReadError::Stopped => f.write_str("stopped"),
            ReadError::Io(e) => write!(f, "{e}"),
        }
    }
}

/// How long an idle reader sleeps between probes.
const POLL_MAX: Duration = Duration::from_millis(10);
const CHUNK: usize = 64 * 1024;

/// Splits a byte stream into lines, keeping what it has read past the last newline for next time.
pub struct LineReader {
    buf: Vec<u8>,
    cap: usize,
}

impl LineReader {
    /// `cap` is the longest line accepted, without its newline.
    pub fn new(cap: usize) -> Self {
        Self {
            buf: Vec::new(),
            cap,
        }
    }

    /// The next non-empty line (newline and a trailing `\r` stripped).
    pub fn read_line(
        &mut self,
        wire: &impl Wire,
        deadline: Option<Instant>,
        stop: &AtomicBool,
    ) -> Result<Vec<u8>, ReadError> {
        self.read_line_with(wire, deadline, stop, &mut || Ok(()))
    }

    /// `read_line`, calling `idle` before every nap. The server uses it to flush queued events to
    /// the client from the one thread that touches the wire.
    pub fn read_line_with(
        &mut self,
        wire: &impl Wire,
        deadline: Option<Instant>,
        stop: &AtomicBool,
        idle: &mut dyn FnMut() -> io::Result<()>,
    ) -> Result<Vec<u8>, ReadError> {
        let mut nap = Duration::from_millis(1);
        loop {
            while let Some(pos) = self.buf.iter().position(|b| *b == b'\n') {
                let mut line: Vec<u8> = self.buf.drain(..=pos).collect();
                line.pop();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                if line.len() > self.cap {
                    return Err(ReadError::TooLong);
                }
                if !line.iter().all(u8::is_ascii_whitespace) {
                    return Ok(line);
                }
            }
            if self.buf.len() > self.cap {
                return Err(ReadError::TooLong);
            }
            if stop.load(Ordering::SeqCst) {
                return Err(ReadError::Stopped);
            }
            if deadline.is_some_and(|d| Instant::now() >= d) {
                return Err(ReadError::Timeout);
            }
            let start = self.buf.len();
            self.buf.resize(start + CHUNK, 0);
            let polled = wire.poll_read(&mut self.buf[start..]);
            match polled {
                Err(e) => {
                    self.buf.truncate(start);
                    return Err(ReadError::Io(e));
                }
                Ok(None) => {
                    self.buf.truncate(start);
                    return Err(ReadError::Closed);
                }
                Ok(Some(0)) => {
                    self.buf.truncate(start);
                    idle().map_err(ReadError::Io)?;
                    std::thread::sleep(nap); // sync-ok: a channel worker thread, never the UI thread
                    nap = (nap * 2).min(POLL_MAX);
                }
                Ok(Some(got)) => {
                    self.buf.truncate(start + got);
                    nap = Duration::from_millis(1);
                }
            }
        }
    }
}

/// Writes one message as a line. Refuses a message over the cap instead of sending one the peer
/// would have to drop.
pub fn write_line(wire: &impl Wire, line: &[u8], cap: usize) -> io::Result<()> {
    if line.len() > cap + 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "message passes the size limit",
        ));
    }
    wire.write_all(line)
}

#[cfg(test)]
pub mod testwire {
    //! An in-memory wire for tests: two queues, one per direction.
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    pub struct Half {
        pub data: VecDeque<u8>,
        pub closed: bool,
    }

    /// One end: reads from `rx`, writes to `tx`.
    #[derive(Clone)]
    pub struct MemWire {
        pub rx: Arc<Mutex<Half>>,
        pub tx: Arc<Mutex<Half>>,
    }

    pub fn pair() -> (MemWire, MemWire) {
        let a = Arc::new(Mutex::new(Half::default()));
        let b = Arc::new(Mutex::new(Half::default()));
        (
            MemWire {
                rx: a.clone(),
                tx: b.clone(),
            },
            MemWire { rx: b, tx: a },
        )
    }

    impl MemWire {
        pub fn close(&self) {
            self.tx.lock().unwrap().closed = true;
        }
    }

    impl Wire for MemWire {
        fn poll_read(&self, buf: &mut [u8]) -> io::Result<Option<usize>> {
            let mut h = self.rx.lock().unwrap();
            if h.data.is_empty() && h.closed {
                return Ok(None);
            }
            let n = buf.len().min(h.data.len());
            for slot in buf.iter_mut().take(n) {
                *slot = h.data.pop_front().unwrap();
            }
            Ok(Some(n))
        }

        fn write_all(&self, bytes: &[u8]) -> io::Result<()> {
            self.tx.lock().unwrap().data.extend(bytes.iter().copied());
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testwire::pair;
    use super::*;

    fn stop() -> AtomicBool {
        AtomicBool::new(false)
    }

    fn soon() -> Option<Instant> {
        Some(Instant::now() + Duration::from_millis(200))
    }

    #[test]
    fn splits_lines_and_keeps_the_remainder() {
        let (a, b) = pair();
        b.write_all(b"one\ntwo\npart").unwrap();
        let mut r = LineReader::new(1024);
        assert_eq!(r.read_line(&a, soon(), &stop()).unwrap(), b"one");
        assert_eq!(r.read_line(&a, soon(), &stop()).unwrap(), b"two");
        assert!(matches!(
            r.read_line(&a, soon(), &stop()),
            Err(ReadError::Timeout)
        ));
        b.write_all(b"ial\n").unwrap();
        assert_eq!(r.read_line(&a, soon(), &stop()).unwrap(), b"partial");
    }

    #[test]
    fn strips_carriage_returns_and_skips_blank_lines() {
        let (a, b) = pair();
        b.write_all(b"\n\r\n   \nhi\r\n").unwrap();
        let mut r = LineReader::new(1024);
        assert_eq!(r.read_line(&a, soon(), &stop()).unwrap(), b"hi");
    }

    #[test]
    fn a_line_over_the_cap_is_too_long_with_or_without_newline() {
        let (a, b) = pair();
        b.write_all(&[b'x'; 100]).unwrap();
        let mut r = LineReader::new(50);
        assert!(matches!(
            r.read_line(&a, soon(), &stop()),
            Err(ReadError::TooLong)
        ));
        let (a, b) = pair();
        let mut long = vec![b'x'; 60];
        long.push(b'\n');
        b.write_all(&long).unwrap();
        let mut r = LineReader::new(50);
        assert!(matches!(
            r.read_line(&a, soon(), &stop()),
            Err(ReadError::TooLong)
        ));
    }

    #[test]
    fn a_line_exactly_at_the_cap_passes() {
        let (a, b) = pair();
        let mut l = vec![b'x'; 50];
        l.push(b'\n');
        b.write_all(&l).unwrap();
        let mut r = LineReader::new(50);
        assert_eq!(r.read_line(&a, soon(), &stop()).unwrap().len(), 50);
    }

    #[test]
    fn closing_reports_closed_after_the_data_is_read() {
        let (a, b) = pair();
        b.write_all(b"last\n").unwrap();
        b.close();
        let mut r = LineReader::new(1024);
        assert_eq!(r.read_line(&a, soon(), &stop()).unwrap(), b"last");
        assert!(matches!(
            r.read_line(&a, soon(), &stop()),
            Err(ReadError::Closed)
        ));
    }

    #[test]
    fn a_partial_line_then_close_is_dropped() {
        let (a, b) = pair();
        b.write_all(b"never ends").unwrap();
        b.close();
        let mut r = LineReader::new(1024);
        assert!(matches!(
            r.read_line(&a, soon(), &stop()),
            Err(ReadError::Closed)
        ));
    }

    #[test]
    fn the_stop_flag_ends_a_wait() {
        let (a, _b) = pair();
        let flag = AtomicBool::new(true);
        let mut r = LineReader::new(1024);
        assert!(matches!(
            r.read_line(&a, None, &flag),
            Err(ReadError::Stopped)
        ));
    }

    #[test]
    fn the_deadline_ends_a_wait() {
        let (a, _b) = pair();
        let mut r = LineReader::new(1024);
        let t = Instant::now();
        assert!(matches!(
            r.read_line(&a, Some(t + Duration::from_millis(30)), &stop()),
            Err(ReadError::Timeout)
        ));
        assert!(t.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn write_line_refuses_an_oversize_message() {
        let (a, _b) = pair();
        assert!(write_line(&a, &[b'x'; 200], 100).is_err());
        assert!(write_line(&a, b"ok\n", 100).is_ok());
    }
}
