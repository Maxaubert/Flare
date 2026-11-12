//! OutputBatcher: coalesces pty output into one write per window (8 ms), or at once when a
//! full megabyte is waiting, so a flood never queues a million tiny updates. A lone chunk (a
//! typed key's echo) leaves at once, unless it leaves a line half drawn (rule 1189).

use std::time::{Duration, Instant};

/// Output coalesces for this long after the first byte of a batch.
pub const BATCH_WINDOW: Duration = Duration::from_millis(8);
/// A lone chunk that leaves a line half drawn waits until the pty has been quiet this long (or the
/// window ends), so the rest of the redraw joins it (rule 1189). Node's inquirer writes its erase
/// and its text 0.1 to 0.2 ms apart (measured 2026-11-12, `bw login` on the dev Mac).
pub const REDRAW_SETTLE: Duration = Duration::from_millis(2);
/// A batch this full is due at once, whatever the time.
pub const BATCH_MAX_BYTES: usize = 1024 * 1024;

/// The pty reader's buffer at rest: one read takes at most this much.
pub const READ_MIN: usize = 64 * 1024;
/// The pty reader's buffer while output is sustained.
pub const READ_MAX: usize = 1024 * 1024;
/// Full reads in a row before the buffer doubles.
const FULL_READS_TO_GROW: u32 = 2;

/// Sizes the pty reader's buffer (M10 Task 67): 64 KiB at rest, doubling toward 1 MiB while reads keep
/// filling it (a flood, so each read call moves more bytes), and back to 64 KiB at the first short
/// read (so a quiet shell holds no big buffer). It changes how much is READ per call, never when the
/// batcher flushes: the 8 ms window and the 1 MiB cap above stay as they were (rule 31).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadSizer {
    cap: usize,
    full_in_a_row: u32,
}

impl Default for ReadSizer {
    fn default() -> Self {
        Self::new()
    }
}

impl ReadSizer {
    pub fn new() -> Self {
        Self {
            cap: READ_MIN,
            full_in_a_row: 0,
        }
    }

    /// The size the next read's buffer should have.
    pub fn cap(&self) -> usize {
        self.cap
    }

    /// Notes that a read of `n` bytes returned.
    pub fn after_read(&mut self, n: usize) {
        if n >= self.cap {
            self.full_in_a_row += 1;
            if self.full_in_a_row >= FULL_READS_TO_GROW {
                self.cap = (self.cap * 2).min(READ_MAX);
                self.full_in_a_row = 0;
            }
        } else {
            self.cap = READ_MIN;
            self.full_in_a_row = 0;
        }
    }
}

/// Holds pty output until it is due.
pub struct OutputBatcher {
    window: Duration,
    max_bytes: usize,
    buf: Vec<u8>,
    first_at: Option<Instant>,
    /// When the last batch left; an arrival more than one window after it is an isolated chunk.
    last_take: Option<Instant>,
    /// The batch in hand began as an isolated chunk: it is due at once (a keystroke's echo).
    isolated: bool,
    /// When the last chunk arrived.
    last_push: Option<Instant>,
    /// Whether the output so far leaves a line half drawn (rule 1189).
    scan: RedrawScan,
    /// How long a held half-drawn chunk waits for the pty to go quiet (`REDRAW_SETTLE`).
    settle: Duration,
}

impl OutputBatcher {
    pub fn new(window: Duration, max_bytes: usize) -> Self {
        Self {
            window,
            max_bytes,
            buf: Vec::new(),
            first_at: None,
            last_take: None,
            isolated: false,
            last_push: None,
            scan: RedrawScan::default(),
            settle: REDRAW_SETTLE,
        }
    }

    /// The same batcher with another settle time (a test seam: the session's tests stretch it so a
    /// loaded runner's scheduling cannot pass for a quiet pty).
    pub fn with_settle(mut self, settle: Duration) -> Self {
        self.settle = settle;
        self
    }

    /// Appends; the first push after a take sets first_at = now. A push that starts a batch more
    /// than one window after the previous batch left is an isolated chunk (M10 Task 68): it is due
    /// at once, so a typed key's echo does not wait for the window. A burst still coalesces: the
    /// chunks after the first arrive within a window of the flush and wait for the next one.
    pub fn push(&mut self, data: &[u8], now: Instant) {
        if self.buf.is_empty() {
            self.first_at = Some(now);
            self.isolated = self
                .last_take
                .is_some_and(|t| now.saturating_duration_since(t) > self.window);
        }
        self.buf.extend_from_slice(data);
        self.scan.feed(data);
        self.last_push = Some(now);
    }

    /// Whether the pump should look again after this push: a held lone chunk may be due now that
    /// the rest of its redraw came (rule 1189).
    pub fn holds_a_half_drawn_line(&self) -> bool {
        self.isolated && !self.buf.is_empty()
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
        let window_left = self
            .window
            .saturating_sub(now.saturating_duration_since(first));
        if self.isolated {
            // An echo leaves at once. A redraw that has only erased its line so far, or hidden the
            // caret, waits for the pty to go quiet, so no frame shows the blank line (rule 1189).
            if !self.scan.half_drawn() {
                return Some(Duration::ZERO);
            }
            let quiet = now.saturating_duration_since(self.last_push.unwrap_or(first));
            return Some(self.settle.saturating_sub(quiet).min(window_left));
        }
        Some(window_left)
    }

    /// The whole buffer when due_in == Some(ZERO), else None. Resets.
    pub fn take_due(&mut self, now: Instant) -> Option<Vec<u8>> {
        if self.due_in(now) == Some(Duration::ZERO) {
            self.last_take = Some(now);
            self.take()
        } else {
            None
        }
    }

    /// Everything now (used on exit so the shell's last words are not left queued); None when empty. Resets.
    pub fn flush(&mut self) -> Option<Vec<u8>> {
        self.last_take = Some(Instant::now());
        self.take()
    }

    fn take(&mut self) -> Option<Vec<u8>> {
        self.first_at = None;
        self.isolated = false;
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

/// Follows the output's escape sequences just far enough to tell whether it leaves a line half
/// drawn (rule 1189): the caret hidden (`CSI ? 25 l`), something erased (`CSI K`, `CSI J`,
/// `CSI X`) with no text written since, or a sequence cut in the middle. Kept across batches,
/// because a hidden caret stays hidden until shown.
#[derive(Clone, Debug, Default)]
struct RedrawScan {
    state: ScanState,
    params: Vec<u8>,
    caret_hidden: bool,
    erased: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ScanState {
    #[default]
    Ground,
    Esc,
    EscInter,
    Csi,
    Str,
    StrEsc,
}

/// The longest CSI parameter string kept; longer ones are not caret or erase sequences.
const SCAN_PARAMS_MAX: usize = 32;

impl RedrawScan {
    fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.step(b);
        }
    }

    fn step(&mut self, b: u8) {
        use ScanState::*;
        self.state = match (self.state, b) {
            (Str, 0x07) => Ground,
            (Str, 0x1b) => StrEsc,
            (Str, _) => Str,
            (StrEsc, b'\\') => Ground,
            (StrEsc, _) => Str,
            (_, 0x1b) => Esc,
            (Ground, b) => {
                if b >= 0x20 && b != 0x7f {
                    self.erased = false;
                }
                Ground
            }
            (Esc, b'[') => {
                self.params.clear();
                Csi
            }
            (Esc, b']' | b'P' | b'_' | b'^' | b'X') => Str,
            (Esc | EscInter, 0x20..=0x2f) => EscInter,
            (Esc | EscInter, _) => Ground,
            (Csi, 0x30..=0x3f) => {
                if self.params.len() < SCAN_PARAMS_MAX {
                    self.params.push(b);
                }
                Csi
            }
            (Csi, 0x20..=0x2f) => Csi,
            (Csi, 0x40..=0x7e) => {
                self.csi(b);
                Ground
            }
            (Csi, _) => Csi,
        };
    }

    fn csi(&mut self, fin: u8) {
        match fin {
            b'K' | b'J' | b'X' => self.erased = true,
            b'h' | b'l' => {
                if let Some(modes) = self.params.strip_prefix(b"?")
                    && modes.split(|c| *c == b';').any(|m| m == b"25")
                {
                    self.caret_hidden = fin == b'l';
                }
            }
            _ => {}
        }
    }

    fn half_drawn(&self) -> bool {
        self.caret_hidden || self.erased || self.state != ScanState::Ground
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
    fn the_read_buffer_grows_while_output_is_sustained_and_falls_back_at_once() {
        let mut s = ReadSizer::new();
        assert_eq!(s.cap(), READ_MIN);
        // One full read is not yet a flood; two in a row are.
        s.after_read(READ_MIN);
        assert_eq!(s.cap(), READ_MIN);
        s.after_read(READ_MIN);
        assert_eq!(s.cap(), 2 * READ_MIN);
        let mut seen = vec![s.cap()];
        for _ in 0..20 {
            let c = s.cap();
            s.after_read(c);
            if s.cap() != *seen.last().unwrap() {
                seen.push(s.cap());
            }
        }
        assert_eq!(seen, vec![128 * 1024, 256 * 1024, 512 * 1024, READ_MAX]);
        // It never passes the cap, and one short read ends it.
        s.after_read(READ_MAX);
        s.after_read(READ_MAX);
        assert_eq!(s.cap(), READ_MAX);
        s.after_read(100);
        assert_eq!(s.cap(), READ_MIN);
    }

    #[test]
    fn the_flush_rule_still_holds() {
        // Bigger reads change the size of a push, not the rule: a batch is due 8 ms after its first
        // byte, or at once when a full mebibyte is waiting.
        let t0 = Instant::now();
        let mut b = OutputBatcher::new(BATCH_WINDOW, BATCH_MAX_BYTES);
        b.push(&vec![b'x'; READ_MAX - 1], t0);
        assert_eq!(
            b.take_due(t0),
            None,
            "just under the cap waits for the window"
        );
        assert_eq!(
            b.due_in(t0 + Duration::from_millis(3)),
            Some(Duration::from_millis(5))
        );
        b.push(b"y", t0 + Duration::from_millis(3));
        assert_eq!(
            b.take_due(t0 + Duration::from_millis(3)).map(|v| v.len()),
            Some(READ_MAX),
            "a full mebibyte is due at once"
        );
        // A read of the largest size is itself a full batch.
        b.push(&vec![b'z'; READ_MAX], t0 + Duration::from_millis(4));
        assert_eq!(
            b.due_in(t0 + Duration::from_millis(4)),
            Some(Duration::ZERO)
        );
        assert_eq!(BATCH_WINDOW, Duration::from_millis(8));
    }

    #[test]
    fn an_isolated_chunk_flushes_immediately() {
        let t0 = Instant::now();
        let mut b = OutputBatcher::new(BATCH_WINDOW, BATCH_MAX_BYTES);
        // The first batch of a session coalesces as before, then leaves.
        b.push(b"prompt", t0);
        assert_eq!(b.take_due(t0), None);
        assert!(b.take_due(t0 + BATCH_WINDOW).is_some());
        // Much later one echo arrives alone: due at once.
        let t1 = t0 + Duration::from_millis(500);
        b.push(b"x", t1);
        assert_eq!(b.due_in(t1), Some(Duration::ZERO));
        assert_eq!(b.take_due(t1), Some(b"x".to_vec()));
    }

    #[test]
    fn a_burst_still_coalesces_to_8_ms() {
        let t0 = Instant::now();
        let mut b = OutputBatcher::new(BATCH_WINDOW, BATCH_MAX_BYTES);
        b.push(b"a", t0);
        assert!(b.take_due(t0 + BATCH_WINDOW).is_some());
        // An isolated first chunk of a burst leaves at once, the rest wait one window.
        let t1 = t0 + Duration::from_millis(500);
        b.push(b"b", t1);
        assert_eq!(b.take_due(t1), Some(b"b".to_vec()));
        let t2 = t1 + Duration::from_millis(1);
        b.push(b"c", t2);
        b.push(b"d", t2 + Duration::from_millis(1));
        assert_eq!(b.take_due(t2 + Duration::from_millis(2)), None);
        assert_eq!(
            b.take_due(t2 + BATCH_WINDOW),
            Some(b"cd".to_vec()),
            "chunks inside the window of a flush coalesce"
        );
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

    // Rule 1189: `bw login` (Node inquirer 8.2.6) redraws its prompt line on every key in four
    // writes, captured under a pty on the dev Mac (2026-11-12): erase, text, move left, move right.
    const PROMPT: &[u8] = b"\x1b[32m?\x1b[39m \x1b[1mEmail address:\x1b[22m\x1b[0m \x1b[0m";

    fn inquirer_key(typed: &str) -> Vec<Vec<u8>> {
        let n = 17 + typed.len();
        let mut text = PROMPT.to_vec();
        text.extend_from_slice(typed.as_bytes());
        vec![
            b"\x1b[2K\x1b[G".to_vec(),
            text,
            format!("\x1b[{n}D").into_bytes(),
            format!("\x1b[{n}C").into_bytes(),
        ]
    }

    /// Pushes timed chunks and polls the batcher as the pump would (every 50 us). Feeds each
    /// delivered batch to a terminal and returns what each presented frame showed: the prompt
    /// row's text, the caret's column and whether the caret was visible.
    fn present(chunks: &[(u64, Vec<u8>)], until_us: u64) -> Vec<(String, usize, bool)> {
        let t0 = Instant::now();
        let at = |us: u64| t0 + Duration::from_micros(us);
        let mut b = OutputBatcher::new(BATCH_WINDOW, BATCH_MAX_BYTES);
        let mut e = crate::emulator::Emulator::new(80, 5);
        // The prompt was drawn long ago.
        b.push(&[PROMPT, b"\x1b[17D\x1b[17C"].concat(), t0);
        e.feed(&b.take_due(at(10_000)).unwrap());
        let mut frames = Vec::new();
        let mut next = 0;
        let mut us = 500_000;
        while us <= 500_000 + until_us {
            while next < chunks.len() && 500_000 + chunks[next].0 <= us {
                b.push(&chunks[next].1, at(us));
                next += 1;
            }
            if let Some(batch) = b.take_due(at(us)) {
                e.feed(&batch);
                let m = e.mode();
                frames.push((e.row_text(0), e.cursor().1, m.cursor_visible));
            }
            us += 50;
        }
        frames
    }

    #[test]
    fn an_inquirer_redraw_presents_no_blank_line() {
        // The writes land 100 to 200 us apart, as measured; no frame shows the erased line.
        let key = inquirer_key("a");
        let chunks: Vec<(u64, Vec<u8>)> = [0, 150, 170, 180].into_iter().zip(key).collect();
        let frames = present(&chunks, 30_000);
        assert!(!frames.is_empty());
        for (row, col, shown) in &frames {
            assert_eq!(row, "? Email address: a", "a frame showed {row:?}");
            assert_eq!(*col, 18);
            assert!(*shown);
        }
    }

    #[test]
    fn a_slow_redraw_still_presents_no_blank_line() {
        // A busy machine: 1.5 ms between the erase and the text, still inside the settle time.
        let key = inquirer_key("ab");
        let chunks: Vec<(u64, Vec<u8>)> = [0, 1_500, 1_600, 1_700].into_iter().zip(key).collect();
        for (row, ..) in present(&chunks, 30_000) {
            assert_eq!(row, "? Email address: ab");
        }
    }

    #[test]
    fn a_hidden_caret_holds_a_lone_redraw_until_shown() {
        let chunks = vec![
            (0, b"\x1b[?25l\x1b[2K\x1b[G".to_vec()),
            (300, PROMPT.to_vec()),
            (600, b"x\x1b[?25h".to_vec()),
        ];
        let frames = present(&chunks, 30_000);
        assert_eq!(frames.len(), 1, "{frames:?}");
        assert_eq!(frames[0], ("? Email address: x".into(), 18, true));
    }

    #[test]
    fn a_half_drawn_line_waits_at_most_the_window() {
        // A program that erases and then goes quiet is still shown: after the settle time, and
        // never later than the window.
        let t0 = Instant::now();
        let mut b = OutputBatcher::new(BATCH_WINDOW, BATCH_MAX_BYTES);
        b.push(b"x", t0);
        b.take_due(t0 + BATCH_WINDOW).unwrap();
        let t1 = t0 + Duration::from_millis(500);
        b.push(b"\x1b[2K", t1);
        assert!(b.holds_a_half_drawn_line());
        assert_eq!(b.due_in(t1), Some(REDRAW_SETTLE));
        assert_eq!(b.take_due(t1 + REDRAW_SETTLE), Some(b"\x1b[2K".to_vec()));
        // A caret hidden for good: every lone chunk waits for quiet, but each push restarts it
        // only up to the window.
        let t2 = t1 + Duration::from_millis(500);
        b.push(b"\x1b[?25l", t2);
        for k in 1..=7 {
            b.push(b".", t2 + Duration::from_millis(k));
        }
        assert_eq!(
            b.due_in(t2 + Duration::from_millis(7)),
            Some(Duration::from_millis(1)),
            "capped by the window"
        );
        assert!(b.take_due(t2 + BATCH_WINDOW).is_some());
    }

    #[test]
    fn a_plain_echo_still_leaves_at_once() {
        // zsh's echoes, captured on the dev Mac (2026-11-12): a letter, a retyped word, a backspace.
        for echo in [&b"l"[..], b"\x08ls", b"\x08\x08l \x08", b"\x1b[D", b"\r\n"] {
            let t0 = Instant::now();
            let mut b = OutputBatcher::new(BATCH_WINDOW, BATCH_MAX_BYTES);
            b.push(b"prompt", t0);
            b.take_due(t0 + BATCH_WINDOW).unwrap();
            let t1 = t0 + Duration::from_millis(500);
            b.push(echo, t1);
            assert_eq!(b.due_in(t1), Some(Duration::ZERO), "{echo:?}");
        }
    }

    #[test]
    fn the_scan_follows_caret_erase_and_cut_sequences() {
        let half = |bytes: &[u8]| {
            let mut s = RedrawScan::default();
            s.feed(bytes);
            s.half_drawn()
        };
        assert!(!half(b"plain text"));
        assert!(half(b"\x1b[K"));
        assert!(half(b"\x1b[2J"));
        assert!(half(b"\x1b[5X"));
        assert!(!half(b"\x1b[2Kback"));
        assert!(half(b"\x1b[?25l"));
        assert!(half(b"\x1b[?1049;25l"));
        assert!(!half(b"\x1b[?25l\x1b[?25h"));
        assert!(!half(b"\x1b[?251l"));
        assert!(!half(b"\x1b[25l"), "not the private mode");
        assert!(half(b"\x1b["), "cut in the middle");
        assert!(half(b"\x1b]0;title"), "an unfinished title");
        assert!(!half(b"\x1b]0;title\x07"));
        assert!(!half(b"\x1b]0;title\x1b\\"));
        assert!(!half(b"\x1b7\x1b8"));
        assert!(!half(b"\x1b(B"));
        assert!(!half("\u{e9}".as_bytes()), "UTF-8 text");
        // The caret stays hidden across batches until it is shown.
        let mut s = RedrawScan::default();
        s.feed(b"\x1b[?25l");
        s.feed(b"more text");
        assert!(s.half_drawn());
        s.feed(b"\x1b[?25h");
        assert!(!s.half_drawn());
    }
}
