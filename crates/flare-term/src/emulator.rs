//! Emulator, TermEvent, ModeSnapshot.
//!
//! One `alacritty_terminal::Term` plus the `vte` processor plus the byte `Scanner` for what
//! alacritty drops (OSC and `CSI > q`). `feed` runs `vte` in slices that end exactly at each
//! scanner event and acts on the event between slices, so replies come out in stream order.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Osc52 as AlacrittyOsc52, Term, TermMode};
use alacritty_terminal::vte::ansi::Processor;
use flare_render::grid::Grid;
use flare_shells::cwd::parse_osc9;

use crate::click_caret::MouseTracking;
use crate::grid_map::snapshot_grid;
use crate::osc52::{OSC52_MAX, Osc52, parse_osc52};
use crate::replies::{Ground, colour_query_replies, xtversion_reply};
use crate::scan::{ScanEvent, Scanner};
use crate::version::{ALACRITTY_TERMINAL_VERSION, FLARE_TERM_VERSION};

/// The size handed to `Term::new` and `Term::resize`.
pub struct Dims {
    pub cols: usize,
    pub rows: usize,
}

impl Dimensions for Dims {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TermEvent {
    /// Bytes to write back to the pty (an answer to a query in its stream).
    Reply(Vec<u8>),
    Title(String),
    ResetTitle,
    Bell,
    /// OSC 9;9 folder report.
    Cwd(String),
    /// OSC 52 write, at most `OSC52_MAX` chars.
    ClipboardWrite(String),
    /// OSC 777 passed up raw (the agent code parses it).
    Osc {
        ident: u32,
        data: String,
    },
    Started,
    StartFailed,
    Exited(Option<u32>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModeSnapshot {
    pub app_cursor: bool,
    pub alt_screen: bool,
    pub bracketed_paste: bool,
    pub cursor_visible: bool,
    pub mouse: MouseTracking,
    pub sgr_mouse: bool,
    pub alternate_scroll: bool,
    pub focus_reporting: bool,
}

/// Forwards alacritty's events to the emulator's own queue.
pub struct Listener(Sender<Event>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let _ = self.0.send(event);
    }
}

pub struct Emulator {
    term: Term<Listener>,
    processor: Processor,
    scanner: Scanner,
    events: Receiver<Event>,
    fg: String,
    bg: String,
}

/// alacritty answers DA1 with `ESC[?6c`; xterm.js (and the e2e parity check) say `ESC[?1;2c`.
fn normalise_reply(s: String) -> Vec<u8> {
    if s == "\x1b[?6c" {
        b"\x1b[?1;2c".to_vec()
    } else {
        s.into_bytes()
    }
}

impl Emulator {
    pub const SCROLLBACK: usize = 10_000;

    pub fn new(cols: usize, rows: usize) -> Self {
        let (tx, rx) = channel();
        let config = Config {
            scrolling_history: Self::SCROLLBACK,
            osc52: AlacrittyOsc52::Disabled,
            kitty_keyboard: false,
            ..Config::default()
        };
        let term = Term::new(config, &Dims { cols, rows }, Listener(tx));
        Self {
            term,
            processor: Processor::new(),
            scanner: Scanner::new(),
            events: rx,
            fg: "#dedacf".to_string(),
            bg: "#171717".to_string(),
        }
    }

    /// What OSC 10 and 11 queries are answered with.
    pub fn set_colours(&mut self, fg: &str, bg: &str) {
        self.fg = fg.to_string();
        self.bg = bg.to_string();
    }

    /// Scans the chunk, then feeds `vte` in slices that end at each scanner event, acting on the
    /// event between slices. Returns the events in stream order.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<TermEvent> {
        let scanned = self.scanner.scan(bytes);
        let mut out = Vec::new();
        let mut at = 0;
        for ev in scanned {
            let end = match &ev {
                ScanEvent::Osc { end, .. } | ScanEvent::Xtversion { end } => {
                    (*end).min(bytes.len())
                }
            };
            if end > at {
                self.processor.advance(&mut self.term, &bytes[at..end]);
                at = end;
            }
            self.drain(&mut out);
            self.act(ev, &mut out);
        }
        if at < bytes.len() {
            self.processor.advance(&mut self.term, &bytes[at..]);
        }
        self.drain(&mut out);
        out
    }

    fn act(&mut self, ev: ScanEvent, out: &mut Vec<TermEvent>) {
        match ev {
            ScanEvent::Xtversion { .. } => out.push(TermEvent::Reply(
                xtversion_reply(FLARE_TERM_VERSION, ALACRITTY_TERMINAL_VERSION).into_bytes(),
            )),
            ScanEvent::Osc { payload, .. } => {
                let text = String::from_utf8_lossy(&payload).into_owned();
                let (ident_text, data) = text.split_once(';').unwrap_or((text.as_str(), ""));
                let Ok(ident) = ident_text.parse::<u32>() else {
                    return;
                };
                match ident {
                    9 => {
                        if let Some(p) = parse_osc9(data) {
                            out.push(TermEvent::Cwd(p));
                        }
                    }
                    52 => {
                        if let Some(Osc52::Write(t)) = parse_osc52(data, OSC52_MAX) {
                            out.push(TermEvent::ClipboardWrite(t));
                        }
                    }
                    10 | 11 => {
                        let ground = Ground {
                            fg: &self.fg,
                            bg: &self.bg,
                        };
                        if let Some(rs) = colour_query_replies(ident as u8, data, &ground) {
                            out.extend(rs.into_iter().map(|r| TermEvent::Reply(r.into_bytes())));
                        }
                    }
                    777 => out.push(TermEvent::Osc {
                        ident,
                        data: data.to_string(),
                    }),
                    _ => {}
                }
            }
        }
    }

    /// Maps what alacritty queued; everything unlisted is dropped (never answer a clipboard
    /// load, colour queries come from the scanner).
    fn drain(&mut self, out: &mut Vec<TermEvent>) {
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                Event::PtyWrite(s) => out.push(TermEvent::Reply(normalise_reply(s))),
                Event::Title(t) => out.push(TermEvent::Title(t)),
                Event::ResetTitle => out.push(TermEvent::ResetTitle),
                Event::Bell => out.push(TermEvent::Bell),
                _ => {}
            }
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.term.resize(Dims { cols, rows });
    }

    pub fn cols(&self) -> usize {
        self.term.columns()
    }

    pub fn rows(&self) -> usize {
        self.term.screen_lines()
    }

    pub fn history_size(&self) -> usize {
        self.term.grid().history_size()
    }

    pub fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    /// (line, column) in screen coordinates.
    pub fn cursor(&self) -> (i32, usize) {
        let p = self.term.grid().cursor.point;
        (p.line.0, p.column.0)
    }

    /// Positive moves toward older output.
    pub fn scroll_view(&mut self, delta_lines: i32) {
        self.term.scroll_display(Scroll::Delta(delta_lines));
    }

    pub fn scroll_to_bottom(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
    }

    pub fn mode(&self) -> ModeSnapshot {
        let m = self.term.mode();
        let mouse = if m.contains(TermMode::MOUSE_MOTION) {
            MouseTracking::Any
        } else if m.contains(TermMode::MOUSE_DRAG) {
            MouseTracking::Drag
        } else if m.contains(TermMode::MOUSE_REPORT_CLICK) {
            MouseTracking::Vt200
        } else {
            MouseTracking::None
        };
        ModeSnapshot {
            app_cursor: m.contains(TermMode::APP_CURSOR),
            alt_screen: m.contains(TermMode::ALT_SCREEN),
            bracketed_paste: m.contains(TermMode::BRACKETED_PASTE),
            cursor_visible: m.contains(TermMode::SHOW_CURSOR),
            mouse,
            sgr_mouse: m.contains(TermMode::SGR_MOUSE),
            alternate_scroll: m.contains(TermMode::ALTERNATE_SCROLL),
            focus_reporting: m.contains(TermMode::FOCUS_IN_OUT),
        }
    }

    pub fn snapshot(&self) -> Grid {
        snapshot_grid(&self.term)
    }

    /// The cells of one screen line as text, trailing blanks trimmed, wide spacers skipped.
    pub fn row_text(&self, line: i32) -> String {
        let grid = self.term.grid();
        let mut s = String::new();
        for col in 0..self.term.columns() {
            let cell = &grid[Point::new(Line(line), Column(col))];
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            s.push(if cell.c == '\0' { ' ' } else { cell.c });
            if let Some(z) = cell.zerowidth() {
                s.extend(z.iter());
            }
        }
        s.trim_end().to_string()
    }

    pub fn term(&self) -> &Term<Listener> {
        &self.term
    }

    pub fn term_mut(&mut self) -> &mut Term<Listener> {
        &mut self.term
    }

    /// While a synchronized update (DEC 2026) holds bytes back: when it must be let go.
    pub fn sync_deadline(&self) -> Option<Instant> {
        self.processor.sync_timeout().sync_timeout()
    }

    /// Lets a synchronized update go when its deadline has passed.
    pub fn release_sync_if_due(&mut self, now: Instant) -> Vec<TermEvent> {
        let mut out = Vec::new();
        if let Some(d) = self.sync_deadline()
            && now >= d
        {
            self.processor.stop_sync(&mut self.term);
            self.drain(&mut out);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_render::grid::CellFlags;
    use std::time::Duration;

    fn replies(evs: &[TermEvent]) -> Vec<Vec<u8>> {
        evs.iter()
            .filter_map(|e| match e {
                TermEvent::Reply(b) => Some(b.clone()),
                _ => None,
            })
            .collect()
    }

    fn xt() -> Vec<u8> {
        format!("\x1bP>|Flare {FLARE_TERM_VERSION} (alacritty_terminal 0.26.0)\x1b\\").into_bytes()
    }

    #[test]
    fn scrollback_is_ten_thousand_lines() {
        let mut e = Emulator::new(80, 24);
        for chunk in 0..11 {
            let mut s = String::new();
            for i in chunk * 1000..(chunk + 1) * 1000 {
                s.push_str(&format!("l{i}\r\n"));
            }
            e.feed(s.as_bytes());
        }
        assert_eq!(e.history_size(), 10_000);
    }

    #[test]
    fn da1_is_answered_the_xterm_way() {
        let mut e = Emulator::new(80, 24);
        assert_eq!(replies(&e.feed(b"\x1b[c")), vec![b"\x1b[?1;2c".to_vec()]);
    }

    #[test]
    fn cpr_is_answered_with_the_cursor() {
        let mut e = Emulator::new(80, 24);
        assert_eq!(
            replies(&e.feed(b"\x1b[3;5H\x1b[6n")),
            vec![b"\x1b[3;5R".to_vec()]
        );
    }

    #[test]
    fn device_status_ok_is_answered() {
        let mut e = Emulator::new(80, 24);
        assert_eq!(replies(&e.feed(b"\x1b[5n")), vec![b"\x1b[0n".to_vec()]);
    }

    #[test]
    fn xtversion_is_answered_in_stream_order() {
        let mut e = Emulator::new(80, 24);
        assert_eq!(
            replies(&e.feed(b"\x1b[>q\x1b[c")),
            vec![xt(), b"\x1b[?1;2c".to_vec()]
        );
        assert_eq!(
            replies(&e.feed(b"\x1b[c\x1b[>q")),
            vec![b"\x1b[?1;2c".to_vec(), xt()]
        );
    }

    #[test]
    fn xtversion_split_across_two_feeds_is_answered_once() {
        let mut e = Emulator::new(80, 24);
        let mut all = replies(&e.feed(b"\x1b["));
        all.extend(replies(&e.feed(b">q")));
        assert_eq!(all, vec![xt()]);
    }

    #[test]
    fn osc_11_and_10_queries_answer_the_ground() {
        let mut e = Emulator::new(80, 24);
        e.set_colours("#3d2a1f", "#e6d8c0");
        assert_eq!(
            replies(&e.feed(b"\x1b]11;?\x1b\\")),
            vec![b"\x1b]11;rgb:e6e6/d8d8/c0c0\x1b\\".to_vec()]
        );
        assert_eq!(
            replies(&e.feed(b"\x1b]10;?;?\x07")),
            vec![
                b"\x1b]10;rgb:3d3d/2a2a/1f1f\x1b\\".to_vec(),
                b"\x1b]11;rgb:e6e6/d8d8/c0c0\x1b\\".to_vec()
            ]
        );
        assert!(replies(&e.feed(b"\x1b]11;rgb:00/00/00\x07")).is_empty());
    }

    #[test]
    fn osc_52_write_becomes_an_event_and_a_read_is_never_answered() {
        let mut e = Emulator::new(80, 24);
        assert_eq!(
            e.feed(b"\x1b]52;c;aGVsbG8=\x07"),
            vec![TermEvent::ClipboardWrite("hello".into())]
        );
        assert!(e.feed(b"\x1b]52;c;?\x07").is_empty());
        assert!(e.feed(b"\x1b]52;p;aGVsbG8=\x07").is_empty());
    }

    #[test]
    fn osc_9_9_is_the_folder_report() {
        let mut e = Emulator::new(80, 24);
        assert_eq!(
            e.feed(b"\x1b]9;9;C:\\Users\\me\x1b\\"),
            vec![TermEvent::Cwd("C:\\Users\\me".into())]
        );
        assert!(e.feed(b"\x1b]9;4;100;50\x07").is_empty());
    }

    #[test]
    fn osc_777_is_passed_up_raw() {
        let mut e = Emulator::new(80, 24);
        assert_eq!(
            e.feed(b"\x1b]777;flare-agent;state=working\x07"),
            vec![TermEvent::Osc {
                ident: 777,
                data: "flare-agent;state=working".into()
            }]
        );
    }

    #[test]
    fn title_and_bell() {
        let mut e = Emulator::new(80, 24);
        assert_eq!(
            e.feed(b"\x1b]0;hello\x07"),
            vec![TermEvent::Title("hello".into())]
        );
        assert_eq!(e.feed(b"\x07"), vec![TermEvent::Bell]);
    }

    #[test]
    fn unasked_things_are_not_answered() {
        let mut e = Emulator::new(80, 24);
        let evs = e.feed(b"\x1b[?1004h\x1b[?9001h\x1b[?996n\x1b[?2031h");
        assert!(replies(&evs).is_empty());
    }

    #[test]
    fn modes_follow_the_stream() {
        let mut e = Emulator::new(80, 24);
        e.feed(b"\x1b[?1h\x1b[?2004h\x1b[?1000h\x1b[?1006h");
        let m = e.mode();
        assert!(m.app_cursor && m.bracketed_paste && m.sgr_mouse);
        assert_eq!(m.mouse, MouseTracking::Vt200);
        e.feed(b"\x1b[?1002h");
        assert_eq!(e.mode().mouse, MouseTracking::Drag);
        e.feed(b"\x1b[?1003h");
        assert_eq!(e.mode().mouse, MouseTracking::Any);
        e.feed(b"\x1b[?1049h");
        assert!(e.mode().alt_screen);
        e.feed(b"\x1b[?25l");
        assert!(!e.mode().cursor_visible);
        e.feed(b"\x1b[?1004h");
        assert!(e.mode().focus_reporting);
    }

    #[test]
    fn wide_and_combining_survive_into_the_snapshot() {
        let mut e = Emulator::new(80, 24);
        e.feed("\u{4e2d}e\u{301}".as_bytes());
        let g = e.snapshot();
        assert_eq!(g.cell(0, 0).cluster(), "\u{4e2d}");
        assert!(g.cell(0, 0).flags.contains(CellFlags::WIDE));
        assert!(g.cell(1, 0).flags.contains(CellFlags::WIDE_SPACER));
        assert_eq!(g.cell(2, 0).cluster(), "e\u{301}");
    }

    #[test]
    fn row_text_trims_and_skips_spacers() {
        let mut e = Emulator::new(80, 24);
        e.feed("a\u{4e2d}b".as_bytes());
        assert_eq!(e.row_text(0), "a\u{4e2d}b");
    }

    #[test]
    fn scroll_view_moves_and_a_key_snaps_back() {
        let mut e = Emulator::new(5, 3);
        for i in 0..10 {
            e.feed(format!("{i}\r\n").as_bytes());
        }
        e.scroll_view(2);
        assert_eq!(e.display_offset(), 2);
        e.scroll_to_bottom();
        assert_eq!(e.display_offset(), 0);
    }

    #[test]
    fn a_garbled_stream_does_not_panic() {
        let mut e = Emulator::new(80, 24);
        let mut x: u32 = 1;
        let bytes: Vec<u8> = (0..65_536)
            .map(|_| {
                x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (x >> 24) as u8
            })
            .collect();
        e.feed(&bytes);
        assert_eq!(e.rows(), 24);
    }

    #[test]
    fn a_closed_synchronized_update_shows_at_once() {
        let mut e = Emulator::new(80, 24);
        e.feed(b"\x1b[?2026hA\x1b[?2026l");
        assert_eq!(e.row_text(0), "A");
        assert_eq!(e.sync_deadline(), None);
    }

    #[test]
    fn an_open_synchronized_update_is_let_go_at_its_deadline() {
        let mut e = Emulator::new(80, 24);
        e.feed(b"\x1b[?2026hB");
        assert_eq!(e.row_text(0), "");
        let d = e.sync_deadline().expect("deadline");
        assert!(
            e.release_sync_if_due(d - Duration::from_millis(1))
                .is_empty()
        );
        assert_eq!(e.row_text(0), "");
        e.release_sync_if_due(d);
        assert_eq!(e.row_text(0), "B");
        assert_eq!(e.sync_deadline(), None);
    }

    #[test]
    fn a_query_held_in_a_synchronized_update_is_answered_when_let_go() {
        let mut e = Emulator::new(80, 24);
        assert!(replies(&e.feed(b"\x1b[?2026h\x1b[5n")).is_empty());
        let d = e.sync_deadline().expect("deadline");
        assert_eq!(
            e.release_sync_if_due(d),
            vec![TermEvent::Reply(b"\x1b[0n".to_vec())]
        );
    }
}
