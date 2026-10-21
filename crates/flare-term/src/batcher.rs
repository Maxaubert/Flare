//! OutputBatcher: coalesces pty output into one write per window (8 ms), or at once when a
//! full megabyte is waiting, so a flood never queues a million tiny updates.

use std::time::{Duration, Instant};

/// Output coalesces for this long after the first byte of a batch.
pub const BATCH_WINDOW: Duration = Duration::from_millis(8);
/// A batch this full is due at once, whatever the time.
pub const BATCH_MAX_BYTES: usize = 1024 * 1024;

/// Holds pty output until it is due.
pub struct OutputBatcher {
    window: Duration,
    max_bytes: usize,
    buf: Vec<u8>,
    first_at: Option<Instant>,
}

impl OutputBatcher {
    pub fn new(window: Duration, max_bytes: usize) -> Self {
        Self {
            window,
            max_bytes,
            buf: Vec::new(),
            first_at: None,
        }
    }

    /// Appends; the first push after a take sets first_at = now.
    pub fn push(&mut self, data: &[u8], now: Instant) {
        if self.buf.is_empty() {
            self.first_at = Some(now);
        }
        self.buf.extend_from_slice(data);
    }

    /// None when empty; Some(ZERO) when due (now - first_at >= window, or buf.len() >= max_bytes); else the time left.
    pub fn due_in(&self, now: Instant) -> Option<Duration> {
        let first = self.first_at?;
        if self.buf.is_empty() {
            return None;
        }
        if self.buf.len() >= self.max_bytes {
            return Some(Duration::ZERO);
        }
        let elapsed = now.saturating_duration_since(first);
        Some(self.window.saturating_sub(elapsed))
    }

    /// The whole buffer when due_in == Some(ZERO), else None. Resets.
    pub fn take_due(&mut self, now: Instant) -> Option<Vec<u8>> {
        if self.due_in(now) == Some(Duration::ZERO) {
            self.take()
        } else {
            None
        }
    }

    /// Everything now (used on exit so the shell's last words are not left queued); None when empty. Resets.
    pub fn flush(&mut self) -> Option<Vec<u8>> {
        self.take()
    }

    fn take(&mut self) -> Option<Vec<u8>> {
        self.first_at = None;
        if self.buf.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut self.buf))
        }
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalesces_chunks_and_flushes_once_per_window() {
        let t0 = Instant::now();
        let mut b = OutputBatcher::new(Duration::from_millis(8), BATCH_MAX_BYTES);
        b.push(b"a", t0);
        b.push(b"b", t0);
        b.push(b"c", t0);
        assert_eq!(b.take_due(t0), None);
        assert_eq!(b.take_due(t0 + Duration::from_millis(7)), None);
        assert_eq!(
            b.take_due(t0 + Duration::from_millis(8)),
            Some(b"abc".to_vec())
        );
        b.push(b"d", t0 + Duration::from_millis(8));
        assert_eq!(b.take_due(t0 + Duration::from_millis(15)), None);
        assert_eq!(
            b.take_due(t0 + Duration::from_millis(16)),
            Some(b"d".to_vec())
        );
    }

    #[test]
    fn flush_empties_at_once_and_is_idempotent() {
        let t0 = Instant::now();
        let mut b = OutputBatcher::new(BATCH_WINDOW, BATCH_MAX_BYTES);
        b.push(b"bye", t0);
        assert_eq!(b.flush(), Some(b"bye".to_vec()));
        assert_eq!(b.flush(), None);
    }

    #[test]
    fn a_full_buffer_is_due_at_once() {
        let t0 = Instant::now();
        let mut b = OutputBatcher::new(Duration::from_millis(8), 4);
        b.push(b"abcd", t0);
        assert_eq!(b.due_in(t0), Some(Duration::ZERO));
        assert_eq!(b.take_due(t0), Some(b"abcd".to_vec()));
    }

    #[test]
    fn due_in_counts_down() {
        let t0 = Instant::now();
        let mut b = OutputBatcher::new(Duration::from_millis(8), BATCH_MAX_BYTES);
        assert_eq!(b.due_in(t0), None);
        b.push(b"x", t0);
        assert_eq!(
            b.due_in(t0 + Duration::from_millis(3)),
            Some(Duration::from_millis(5))
        );
    }
}
