//! The output tail of a session (remote sessions PR 2): the last bytes a shell printed, numbered, so a remote viewer
//! can poll `remote.output {since}` and get each byte once, in order, across reconnects.
//!
//! Every byte the session delivers gets the next number of a `u64` that only goes up. The ring keeps the last
//! [`TAIL_CAP`] of them; a viewer whose `since` is older than the ring hears `gap` and asks for a snapshot instead
//! (rule 281). The ring is fed after the tab's own sink, under the session's delivery lock, so the order of the
//! batches is the order the tab saw, and a ring nobody reads costs a copy and never blocks the tab: the pushing thread
//! only takes the ring's own short lock, and a reader only copies out under it.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

/// The most bytes a session's tail keeps: 4 MiB.
pub const TAIL_CAP: usize = 4 << 20;

/// What a read of the tail answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TailRead {
    /// The number of the first byte of `data`.
    pub from: u64,
    /// The number after the last byte of `data`: the `since` of the next poll.
    pub to: u64,
    pub data: Vec<u8>,
    /// `since` was older than the oldest byte kept: bytes were lost, and `data` starts at the oldest kept.
    pub gap: bool,
}

/// The ring itself: plain data, no lock.
#[derive(Debug)]
pub struct TailRing {
    buf: VecDeque<u8>,
    /// The number of `buf[0]`.
    start: u64,
    cap: usize,
}

impl TailRing {
    pub fn new(cap: usize) -> Self {
        Self {
            buf: VecDeque::new(),
            start: 0,
            cap: cap.max(1),
        }
    }

    /// The number the next byte will get: what a viewer that attaches now polls from.
    pub fn end(&self) -> u64 {
        self.start + self.buf.len() as u64
    }

    /// The number of the oldest byte kept.
    pub fn start(&self) -> u64 {
        self.start
    }

    /// Appends `bytes`; the oldest go when the ring is full.
    pub fn push(&mut self, bytes: &[u8]) {
        if bytes.len() >= self.cap {
            let skip = bytes.len() - self.cap;
            self.start = self.end() + skip as u64;
            self.buf.clear();
            self.buf.extend(&bytes[skip..]);
            return;
        }
        let over = (self.buf.len() + bytes.len()).saturating_sub(self.cap);
        if over > 0 {
            self.buf.drain(..over);
            self.start += over as u64;
        }
        self.buf.extend(bytes);
    }

    /// At most `max` bytes from `since` on. A `since` past the end answers nothing from the end (a viewer of an older
    /// run of this host): never bytes it has not seen numbered.
    pub fn read(&self, since: u64, max: usize) -> TailRead {
        let end = self.end();
        if since > end {
            return TailRead {
                from: end,
                to: end,
                data: Vec::new(),
                gap: true,
            };
        }
        let gap = since < self.start;
        let from = since.max(self.start);
        let skip = (from - self.start) as usize;
        let take = (self.buf.len() - skip).min(max);
        let data: Vec<u8> = self.buf.range(skip..skip + take).copied().collect();
        TailRead {
            from,
            to: from + take as u64,
            data,
            gap,
        }
    }
}

/// A session's tail, shared by the session (which pushes) and whoever follows it (which reads).
#[derive(Clone, Debug)]
pub struct TailHandle(Arc<Mutex<TailRing>>);

impl TailHandle {
    pub fn new(cap: usize) -> Self {
        Self(Arc::new(Mutex::new(TailRing::new(cap))))
    }

    fn lk(&self) -> MutexGuard<'_, TailRing> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn push(&self, bytes: &[u8]) {
        self.lk().push(bytes);
    }

    pub fn end(&self) -> u64 {
        self.lk().end()
    }

    pub fn read(&self, since: u64, max: usize) -> TailRead {
        self.lk().read(since, max)
    }

    /// Whether two handles are the one tail.
    pub fn same(&self, other: &TailHandle) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tail_numbers_every_byte_once_in_order() {
        let mut r = TailRing::new(16);
        assert_eq!(r.end(), 0);
        r.push(b"hello ");
        r.push(b"world");
        let a = r.read(0, 100);
        assert_eq!((a.from, a.to, a.gap), (0, 11, false));
        assert_eq!(a.data, b"hello world");
        // The next poll starts where the last ended and gets nothing twice.
        r.push(b"!");
        let b = r.read(a.to, 100);
        assert_eq!((b.from, b.to, b.data.as_slice()), (11, 12, &b"!"[..]));
        assert_eq!(r.read(b.to, 100).data, b"");
    }

    #[test]
    fn a_read_is_capped_and_the_rest_comes_next() {
        let mut r = TailRing::new(64);
        r.push(b"abcdefghij");
        let a = r.read(2, 3);
        assert_eq!((a.from, a.to, a.data.as_slice()), (2, 5, &b"cde"[..]));
        let b = r.read(a.to, 100);
        assert_eq!(b.data, b"fghij");
    }

    /// Rule 281: the ring has a cap; a `since` that fell out of it is a gap, never a silent skip.
    #[test]
    fn the_ring_wraps_at_its_cap_and_an_old_since_is_a_gap() {
        let mut r = TailRing::new(8);
        r.push(b"0123456789");
        assert_eq!((r.start(), r.end()), (2, 10));
        let a = r.read(0, 100);
        assert!(a.gap);
        assert_eq!((a.from, a.data.as_slice()), (2, &b"23456789"[..]));
        r.push(b"ab");
        assert_eq!((r.start(), r.end()), (4, 12));
        let b = r.read(4, 100);
        assert!(!b.gap);
        assert_eq!(b.data, b"456789ab");
        // A chunk bigger than the ring keeps its own last bytes, numbered where they fell.
        r.push(&[b'z'; 20]);
        assert_eq!((r.start(), r.end()), (24, 32));
        assert_eq!(r.read(24, 100).data, vec![b'z'; 8]);
    }

    #[test]
    fn the_sequence_only_goes_up() {
        let mut r = TailRing::new(5);
        let mut last = r.end();
        for chunk in [&b"a"[..], b"bcdefg", b"", b"hi", b"jklmnopqrstu"] {
            r.push(chunk);
            assert!(r.end() >= last);
            assert_eq!(r.end(), last + chunk.len() as u64);
            last = r.end();
        }
    }

    #[test]
    fn a_since_past_the_end_answers_nothing_and_says_gap() {
        let mut r = TailRing::new(8);
        r.push(b"abc");
        let a = r.read(99, 10);
        assert!(a.gap && a.data.is_empty());
        assert_eq!((a.from, a.to), (3, 3));
    }

    #[test]
    fn a_handle_is_shared_by_its_clones() {
        let h = TailHandle::new(TAIL_CAP);
        let h2 = h.clone();
        h.push(b"x");
        assert_eq!(h2.end(), 1);
        assert!(h.same(&h2));
        assert!(!h.same(&TailHandle::new(4)));
    }
}
