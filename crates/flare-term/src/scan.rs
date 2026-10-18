//! Scanner (OSC and CSI > q).
//!
//! `alacritty_terminal` drops OSC 9, 52, the colour queries, 777 and `CSI > q` inside `vte`, and
//! Flare needs them. The scanner runs over every chunk before `vte`, never alters the bytes, and
//! reports where each sequence ended so the caller can feed `vte` up to that point, then act.

/// An OSC 52 of 1 MB of text is about 1.4 MB of base64.
pub const OSC_MAX_BYTES: usize = 1_500_000;

/// More collected CSI parameter bytes than this and the sequence is abandoned.
const CSI_MAX_BYTES: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScanEvent {
    /// A complete OSC (introduced by ESC ], ended by BEL or ESC \). `payload` is everything
    /// between, e.g. `b"9;9;C:\\x"`. `end` is the index in THIS chunk just after the terminator.
    Osc { end: usize, payload: Vec<u8> },
    /// ESC [ > q or ESC [ > 0 q (XTVERSION). `end` as above.
    Xtversion { end: usize },
    /// A CSI erase that alacritty answers by dropping the selection (EL and ED, with their `?`
    /// forms). `start` is where the sequence begins in THIS chunk (0 when it began in an earlier
    /// chunk), `end` just after its final byte. The caller feeds up to `start`, saves the
    /// selection, feeds to `end`, and puts the selection back (xterm keeps it, decision O23).
    SelectionGuard {
        start: usize,
        end: usize,
        erase: Erase,
    },
}

/// What a guarded erase does to the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Erase {
    /// EL, ED 0 and ED 1: nothing scrolls, the selection keeps its place.
    InPlace,
    /// ED 2: alacritty scrolls the screen into the history.
    Screen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Ground,
    Esc,
    OscBody,
    OscEsc,
    Csi,
}

pub struct Scanner {
    state: State,
    osc: Vec<u8>,
    csi: Vec<u8>,
    csi_start: usize,
    overflow: bool,
    limit: usize,
}

impl Default for Scanner {
    fn default() -> Self {
        Self::new()
    }
}

impl Scanner {
    pub fn new() -> Self {
        Self::with_limit(OSC_MAX_BYTES)
    }

    pub fn with_limit(limit: usize) -> Self {
        Self {
            state: State::Ground,
            osc: Vec::new(),
            csi: Vec::new(),
            csi_start: 0,
            overflow: false,
            limit,
        }
    }

    fn start_osc(&mut self) {
        self.osc.clear();
        self.overflow = false;
        self.state = State::OscBody;
    }

    fn start_csi(&mut self) {
        self.csi.clear();
        self.state = State::Csi;
    }

    fn finish_osc(&mut self, end: usize, out: &mut Vec<ScanEvent>) {
        if !self.overflow {
            out.push(ScanEvent::Osc {
                end,
                payload: std::mem::take(&mut self.osc),
            });
        }
        self.osc.clear();
        self.overflow = false;
        self.state = State::Ground;
    }

    /// The byte after an ESC, shared by `Esc` and by an ESC that interrupts an OSC.
    fn after_esc(&mut self, b: u8, esc_at: usize) {
        match b {
            b']' => self.start_osc(),
            b'[' => {
                self.start_csi();
                self.csi_start = esc_at;
            }
            0x1b => self.state = State::Esc,
            _ => self.state = State::Ground,
        }
    }

    pub fn scan(&mut self, chunk: &[u8]) -> Vec<ScanEvent> {
        let mut out = Vec::new();
        let mut i = 0;
        if self.state == State::Csi {
            self.csi_start = 0;
        }
        while i < chunk.len() {
            match self.state {
                State::Ground => match memchr::memchr(0x1b, &chunk[i..]) {
                    Some(p) => {
                        i += p + 1;
                        self.state = State::Esc;
                    }
                    None => break,
                },
                State::Esc => {
                    self.after_esc(chunk[i], i.saturating_sub(1));
                    i += 1;
                }
                State::OscBody => {
                    let rest = &chunk[i..];
                    let term = memchr::memchr2(0x07, 0x1b, rest);
                    let seg = &rest[..term.unwrap_or(rest.len())];
                    if let Some(c) = memchr::memchr2(0x18, 0x1a, seg) {
                        i += c + 1;
                        self.osc.clear();
                        self.overflow = false;
                        self.state = State::Ground;
                        continue;
                    }
                    if !self.overflow {
                        let room = self.limit.saturating_sub(self.osc.len());
                        if seg.len() > room {
                            self.osc.extend_from_slice(&seg[..room]);
                            self.overflow = true;
                        } else {
                            self.osc.extend_from_slice(seg);
                        }
                    }
                    i += seg.len();
                    match term {
                        Some(_) => {
                            let b = chunk[i];
                            i += 1;
                            if b == 0x07 {
                                self.finish_osc(i, &mut out);
                            } else {
                                self.state = State::OscEsc;
                            }
                        }
                        None => break,
                    }
                }
                State::OscEsc => {
                    let b = chunk[i];
                    i += 1;
                    if b == b'\\' {
                        self.finish_osc(i, &mut out);
                    } else {
                        self.after_esc(b, i.saturating_sub(2));
                    }
                }
                State::Csi => {
                    let b = chunk[i];
                    i += 1;
                    match b {
                        0x1b => self.state = State::Esc,
                        0x18 | 0x1a => self.state = State::Ground,
                        0x20..=0x3f => {
                            if self.csi.len() >= CSI_MAX_BYTES {
                                self.state = State::Ground;
                            } else {
                                self.csi.push(b);
                            }
                        }
                        0x40..=0x7e => {
                            if b == b'q' && (self.csi == b">" || self.csi == b">0") {
                                out.push(ScanEvent::Xtversion { end: i });
                            } else if let Some(erase) = guarded_erase(b, &self.csi) {
                                out.push(ScanEvent::SelectionGuard {
                                    start: self.csi_start,
                                    end: i,
                                    erase,
                                });
                            }
                            self.state = State::Ground;
                        }
                        0x00..=0x1f => {}
                        _ => self.state = State::Ground,
                    }
                }
            }
        }
        out
    }
}

/// The erase a CSI final byte and its parameters make, when alacritty drops the selection for it.
fn guarded_erase(final_byte: u8, params: &[u8]) -> Option<Erase> {
    let params = params.strip_prefix(b"?").unwrap_or(params);
    if !params.iter().all(|b| b.is_ascii_digit()) {
        return None;
    }
    match (final_byte, params) {
        (b'K', _) => Some(Erase::InPlace),
        (b'J', b"" | b"0" | b"1") => Some(Erase::InPlace),
        (b'J', b"2") => Some(Erase::Screen),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::ScanEvent::{Osc, Xtversion};
    use super::*;

    fn osc(end: usize, payload: &[u8]) -> ScanEvent {
        Osc {
            end,
            payload: payload.to_vec(),
        }
    }

    #[test]
    fn finds_an_osc_ended_by_bel() {
        let mut s = Scanner::new();
        assert_eq!(s.scan(b"ab\x1b]0;hi\x07cd"), vec![osc(9, b"0;hi")]);
    }

    #[test]
    fn finds_an_osc_ended_by_st() {
        let mut s = Scanner::new();
        assert_eq!(
            s.scan(b"\x1b]52;c;QQ==\x1b\\x"),
            vec![osc(13, b"52;c;QQ==")]
        );
    }

    #[test]
    fn an_osc_split_across_chunks() {
        let mut s = Scanner::new();
        assert_eq!(s.scan(b"x\x1b]9;9;C:\\wo"), vec![]);
        assert_eq!(s.scan(b"rk\x07y"), vec![osc(3, b"9;9;C:\\work")]);
    }

    #[test]
    fn st_split_between_esc_and_backslash() {
        let mut s = Scanner::new();
        assert_eq!(s.scan(b"\x1b]0;t\x1b"), vec![]);
        assert_eq!(s.scan(b"\\z"), vec![osc(1, b"0;t")]);
    }

    #[test]
    fn an_esc_that_is_not_st_aborts_the_osc() {
        let mut s = Scanner::new();
        assert_eq!(s.scan(b"\x1b]0;t\x1b[31mtext"), vec![]);
    }

    #[test]
    fn two_oscs_in_one_chunk() {
        let mut s = Scanner::new();
        assert_eq!(
            s.scan(b"\x1b]0;a\x07\x1b]0;b\x07"),
            vec![osc(6, b"0;a"), osc(12, b"0;b")]
        );
    }

    #[test]
    fn xtversion_csi_gt_q() {
        let mut s = Scanner::new();
        assert_eq!(s.scan(b"\x1b[>q"), vec![Xtversion { end: 4 }]);
        assert_eq!(s.scan(b"\x1b[>0q"), vec![Xtversion { end: 5 }]);
    }

    #[test]
    fn other_csi_q_forms_are_not_xtversion() {
        let mut s = Scanner::new();
        for seq in [
            &b"\x1b[>1q"[..],
            b"\x1b[>0;0q",
            b"\x1b[q",
            b"\x1b[ q",
            b"\x1b[?q",
        ] {
            assert_eq!(s.scan(seq), vec![], "{seq:?}");
        }
    }

    #[test]
    fn xtversion_split_across_chunks() {
        let mut s = Scanner::new();
        assert_eq!(s.scan(b"\x1b["), vec![]);
        assert_eq!(s.scan(b">"), vec![]);
        assert_eq!(s.scan(b"qZ"), vec![Xtversion { end: 1 }]);
    }

    #[test]
    fn an_ordinary_csi_does_not_hang() {
        let mut s = Scanner::new();
        assert_eq!(s.scan(b"\x1b[1;31mred\x1b[0m"), vec![]);
        assert_eq!(s.scan(b"\x1b]0;x\x07"), vec![osc(6, b"0;x")]);
    }

    #[test]
    fn an_oversize_osc_is_dropped_and_resyncs() {
        let mut s = Scanner::with_limit(16);
        let mut input = b"\x1b]0;".to_vec();
        input.extend(std::iter::repeat_n(b'x', 40));
        input.push(0x07);
        input.extend_from_slice(b"\x1b]0;ok\x07");
        let got = s.scan(&input);
        assert_eq!(got.len(), 1);
        match &got[0] {
            Osc { payload, .. } => assert_eq!(payload, b"0;ok"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn can_and_sub_cancel_an_osc() {
        let mut s = Scanner::new();
        assert_eq!(s.scan(b"\x1b]0;ti\x18tle\x07"), vec![]);
        assert_eq!(s.scan(b"\x1b]0;ti\x1atle\x07"), vec![]);
    }

    #[test]
    fn plain_text_scans_empty_and_fast() {
        let mut s = Scanner::new();
        let big = vec![b'a'; 1 << 20];
        let t = std::time::Instant::now();
        assert_eq!(s.scan(&big), vec![]);
        if cfg!(not(debug_assertions)) {
            assert!(t.elapsed() < std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn esc_osc_inside_an_osc_restarts_it() {
        let mut s = Scanner::new();
        assert_eq!(s.scan(b"\x1b]0;a\x1b]0;b\x07"), vec![osc(11, b"0;b")]);
    }

    #[test]
    fn erases_are_guarded_with_their_span() {
        use super::{Erase, ScanEvent::SelectionGuard};
        let mut s = Scanner::new();
        assert_eq!(
            s.scan(b"ab\x1b[2Kc"),
            vec![SelectionGuard {
                start: 2,
                end: 6,
                erase: Erase::InPlace
            }]
        );
        assert_eq!(
            s.scan(b"\x1b[?J\x1b[2J\x1b[3J\x1b[1;3J"),
            vec![
                SelectionGuard {
                    start: 0,
                    end: 4,
                    erase: Erase::InPlace
                },
                SelectionGuard {
                    start: 4,
                    end: 8,
                    erase: Erase::Screen
                },
            ]
        );
    }

    #[test]
    fn a_guarded_erase_split_across_chunks_starts_at_zero() {
        use super::{Erase, ScanEvent::SelectionGuard};
        let mut s = Scanner::new();
        assert_eq!(s.scan(b"xxxx\x1b[2"), vec![]);
        assert_eq!(
            s.scan(b"Jyy"),
            vec![SelectionGuard {
                start: 0,
                end: 1,
                erase: Erase::Screen
            }]
        );
    }
}
