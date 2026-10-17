//! Emulator, TermEvent, ModeSnapshot.
//!
//! One `alacritty_terminal::Term` plus the `vte` processor plus the byte `Scanner` for what
//! alacritty drops (OSC and `CSI > q`). `feed` runs `vte` in slices that end exactly at each
//! scanner event and acts on the event between slices, so replies come out in stream order.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Osc52 as AlacrittyOsc52, Term, TermMode};
use alacritty_terminal::vte::ansi::Processor;
use flare_render::decor::RowSpan;
use flare_render::grid::Grid;
use flare_shells::cwd::{has_control, parse_osc9};

use crate::cells::{
    BufferLike, CellInfo, ScratchCell, buffer_row_cells, char_at_cell, logical_line,
};
use crate::click_caret::MouseTracking;
use crate::grid_map::snapshot_grid;
use crate::links::find_links;
use crate::osc8::is_web_link;
use crate::osc52::{OSC52_MAX, Osc52, parse_osc52};
use crate::replies::{Ground, colour_query_replies, xtversion_reply};
use crate::scan::{ScanEvent, Scanner};
use crate::sel_machine::{Anchor, SelEnv, SelOut, SelSpec, Span};
use crate::sel_text::selection_text;
use crate::term_buf::{TermBuf, abs_to_line, line_to_abs, total_rows, viewport_top};
use crate::version::{ALACRITTY_TERMINAL_VERSION, FLARE_TERM_VERSION};
use crate::word_at::word_range;

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
                        if let Some(p) = parse_osc9(data).filter(|p| !has_control(p)) {
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
        crate::resize::resize_conpty(&mut self.term, cols, rows);
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

/// The ordered boundaries `(row, b)` of a selection and whether it is a block.
pub type SelBounds = ((usize, usize), (usize, usize), bool);

/// The cells of one logical line, with its first row.
struct LineCells {
    first: usize,
    cells: Vec<CellInfo>,
}

/// A `SelEnv` over the current buffer. Built per call; clicks only.
pub struct TermSelEnv<'a> {
    emu: &'a Emulator,
    buf: TermBuf<'a>,
}

impl TermSelEnv<'_> {
    fn line_cells(&self, row: usize) -> LineCells {
        let cols = self.emu.cols();
        let ll = logical_line(&self.buf, cols, row);
        let mut scratch = ScratchCell::default();
        let mut cells = Vec::new();
        for y in ll.first..=ll.last {
            cells
                .extend(buffer_row_cells(self.buf.line(y), cols, &mut scratch).unwrap_or_default());
        }
        LineCells {
            first: ll.first,
            cells,
        }
    }

    /// The hyperlink on the cell at line index `i` (a spacer shares its character's).
    fn hyperlink_at(&self, lc: &LineCells, i: usize) -> Option<(String, String)> {
        let cols = self.emu.cols();
        let mut i = i;
        if lc.cells[i].width == 0 && i > 0 {
            i -= 1;
        }
        let line = abs_to_line(self.emu.history_size(), lc.first + i / cols);
        let cell = &self.emu.term.grid()[Point::new(line, Column(i % cols))];
        cell.hyperlink()
            .map(|h| (h.id().to_string(), h.uri().to_string()))
    }

    /// Cells `s..e` of the line as a span of boundaries.
    fn span_of(&self, first: usize, s: usize, e: usize) -> Span {
        let cols = self.emu.cols();
        let last = e.max(s + 1) - 1;
        Span {
            start: Anchor {
                row: first + s / cols,
                b: s % cols,
            },
            end: Anchor {
                row: first + last / cols,
                b: last + 1 - (last / cols) * cols,
            },
        }
    }
}

impl SelEnv for TermSelEnv<'_> {
    fn cols(&self) -> usize {
        self.emu.cols()
    }

    fn total_rows(&self) -> usize {
        total_rows(&self.emu.term)
    }

    fn cell_width(&self, row: usize, col: usize) -> u8 {
        let mut s = ScratchCell::default();
        match self.buf.line(row) {
            Some(l) if l.load_cell(col, &mut s) => s.width,
            _ => 1,
        }
    }

    fn word_at(&self, row: usize, col: usize, allow_whitespace_only: bool) -> Option<Span> {
        let lc = self.line_cells(row);
        let at = (row - lc.first) * self.cols() + col;
        let (s, e) = word_range(&lc.cells, at, allow_whitespace_only)?;
        Some(self.span_of(lc.first, s, e))
    }

    fn line_at(&self, row: usize) -> (usize, usize) {
        let ll = logical_line(&self.buf, self.cols(), row);
        (ll.first, ll.last)
    }

    fn link_at(&self, row: usize, col: usize) -> Option<Span> {
        let cols = self.cols();
        let ll = logical_line(&self.buf, cols, row);
        let lc = self.line_cells(row);
        let at = (row - lc.first) * cols + col;
        if at >= lc.cells.len() {
            return None;
        }
        if let Some(link) = self.hyperlink_at(&lc, at)
            && is_web_link(&link.1)
        {
            let same = |i: usize| self.hyperlink_at(&lc, i).as_ref() == Some(&link);
            let mut s = at;
            while s > 0 && same(s - 1) {
                s -= 1;
            }
            let mut e = at + 1;
            while e < lc.cells.len() && same(e) {
                e += 1;
            }
            return Some(self.span_of(lc.first, s, e));
        }
        let c = char_at_cell(&ll, cols, row as i32, col as i32);
        let m = find_links(&ll.text)
            .into_iter()
            .find(|m| m.start <= c && c < m.end)?;
        let n = ll.widths.len();
        let s = (0..n).find(|&i| ll.index[i] >= m.start && ll.widths[i] > 0)?;
        let e = (0..n).find(|&i| ll.index[i] >= m.end).unwrap_or(n);
        Some(self.span_of(ll.first, s, e.max(s + 1)))
    }
}

impl Emulator {
    pub fn total_rows(&self) -> usize {
        total_rows(&self.term)
    }

    pub fn viewport_top(&self) -> usize {
        viewport_top(&self.term)
    }

    pub fn view_rows(&self) -> usize {
        self.term.screen_lines()
    }

    pub fn is_scrolled_back(&self) -> bool {
        self.display_offset() > 0
    }

    /// A `SelEnv` over the current buffer.
    pub fn sel_env(&self) -> TermSelEnv<'_> {
        TermSelEnv {
            emu: self,
            buf: TermBuf::all(&self.term),
        }
    }

    fn sel_point(&self, a: Anchor) -> (Point, Side) {
        let cols = self.cols();
        let line = abs_to_line(self.history_size(), a.row);
        let side = if a.b < cols { Side::Left } else { Side::Right };
        (
            Point::new(line, Column(a.b.min(cols.saturating_sub(1)))),
            side,
        )
    }

    /// Applies a machine output. Marks nothing dirty by itself (the handle does).
    pub fn sel_apply(&mut self, out: SelOut) {
        match out {
            SelOut::Nothing => {}
            SelOut::Clear => self.term.selection = None,
            SelOut::Set(spec) => {
                if spec.is_empty() {
                    self.term.selection = None;
                    return;
                }
                let ty = if spec.block {
                    SelectionType::Block
                } else {
                    SelectionType::Simple
                };
                let (p, side) = self.sel_point(spec.start);
                let mut sel = Selection::new(ty, p, side);
                let (p, side) = self.sel_point(spec.end);
                sel.update(p, side);
                self.term.selection = Some(sel);
            }
            SelOut::Extend(a) => {
                let Some((s, e, block)) = self.sel_bounds() else {
                    return;
                };
                let cols = self.cols();
                let lin = |p: (usize, usize)| (p.0 * (cols + 1) + p.1) as i64;
                let at = lin((a.row, a.b));
                let fixed = if at < lin(s) {
                    e
                } else if at > lin(e) || at - lin(s) >= lin(e) - at {
                    s
                } else {
                    e
                };
                self.sel_apply(SelOut::Set(SelSpec {
                    block,
                    start: Anchor {
                        row: fixed.0,
                        b: fixed.1,
                    },
                    end: a,
                }));
            }
        }
    }

    /// A non-empty Simple or Block selection stands.
    pub fn has_selection(&self) -> bool {
        self.sel_bounds().is_some()
    }

    /// The ordered boundaries of the selection and whether it is a block.
    pub fn sel_bounds(&self) -> Option<SelBounds> {
        let sel = self.term.selection.as_ref()?;
        if !matches!(sel.ty, SelectionType::Simple | SelectionType::Block) {
            return None;
        }
        let r = sel.to_range(&self.term)?;
        let hist = self.history_size();
        Some((
            (line_to_abs(hist, r.start.line), r.start.column.0),
            (line_to_abs(hist, r.end.line), r.end.column.0 + 1),
            r.is_block,
        ))
    }

    pub fn sel_text(&self) -> String {
        let Some((s, e, block)) = self.sel_bounds() else {
            return String::new();
        };
        selection_text(&TermBuf::all(&self.term), self.cols(), s, e, block)
    }

    /// The selection's pieces on viewport rows.
    pub fn sel_spans(&self) -> Vec<RowSpan> {
        let Some((s, e, block)) = self.sel_bounds() else {
            return Vec::new();
        };
        let cols = self.cols();
        let top = self.viewport_top();
        let mut out = Vec::new();
        for r in 0..self.view_rows() {
            let abs = top + r;
            if abs < s.0 || abs > e.0 {
                continue;
            }
            let (from, to) = if block {
                (s.1.min(e.1), s.1.max(e.1))
            } else {
                (
                    if abs == s.0 { s.1 } else { 0 },
                    if abs == e.0 { e.1 } else { cols },
                )
            };
            if to > from {
                out.push(RowSpan {
                    row: r,
                    col: from,
                    width: to - from,
                });
            }
        }
        out
    }

    pub fn sel_clear(&mut self) {
        self.term.selection = None;
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
    fn osc_9_9_with_a_control_character_never_emits_a_folder() {
        let mut e = Emulator::new(80, 24);
        let evs = e.feed("\x1b]9;9;C:\\x\u{85}calc\x07".as_bytes());
        assert!(
            !evs.iter().any(|ev| matches!(ev, TermEvent::Cwd(_))),
            "{evs:?}"
        );
        let evs = e.feed(b"\x1b]9;9;C:\\x\rcalc\x07");
        assert!(
            !evs.iter().any(|ev| matches!(ev, TermEvent::Cwd(_))),
            "{evs:?}"
        );
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

    fn sel(block: bool, r0: usize, b0: usize, r1: usize, b1: usize) -> SelOut {
        SelOut::Set(SelSpec {
            block,
            start: Anchor { row: r0, b: b0 },
            end: Anchor { row: r1, b: b1 },
        })
    }

    #[test]
    fn a_set_selection_reads_back_as_text() {
        let mut e = Emulator::new(20, 3);
        e.feed(b"hello world");
        e.sel_apply(sel(false, 0, 0, 0, 5));
        assert_eq!(e.sel_text(), "hello");
        assert!(e.has_selection());
    }

    #[test]
    fn an_empty_spec_clears() {
        let mut e = Emulator::new(20, 3);
        e.feed(b"hello world");
        e.sel_apply(sel(false, 0, 0, 0, 5));
        e.sel_apply(sel(false, 0, 2, 0, 2));
        assert!(!e.has_selection());
    }

    #[test]
    fn clear_removes_it() {
        let mut e = Emulator::new(20, 3);
        e.feed(b"hello world");
        e.sel_apply(sel(false, 0, 0, 0, 5));
        e.sel_apply(SelOut::Clear);
        assert!(!e.has_selection());
        assert_eq!(e.sel_text(), "");
    }

    #[test]
    fn a_selection_survives_new_output_below() {
        let mut e = Emulator::new(20, 3);
        e.feed(b"hello world");
        e.sel_apply(sel(false, 0, 0, 0, 5));
        for _ in 0..5 {
            e.feed(b"\r\nnext");
        }
        assert_eq!(e.sel_text(), "hello");
    }

    #[test]
    fn a_selection_in_the_scrollback_survives_a_screen_clear_below() {
        let mut e = Emulator::new(10, 3);
        for i in 0..10 {
            e.feed(format!("line{i}\r\n").as_bytes());
        }
        e.sel_apply(sel(false, 1, 0, 1, 5));
        assert_eq!(e.sel_text(), "line1");
        // ED 0 clears only over screen lines; alacritty clears the whole selection on ED 2.
        e.feed(b"\x1b[J");
        assert_eq!(e.sel_text(), "line1");
    }

    #[test]
    fn an_erase_over_the_selection_clears_it() {
        let mut e = Emulator::new(20, 3);
        e.feed(b"hello world");
        e.sel_apply(sel(false, 0, 0, 0, 5));
        e.feed(b"\x1b[2K");
        assert!(!e.has_selection());
    }

    #[test]
    fn a_block_selection_is_rectangular() {
        let mut e = Emulator::new(20, 3);
        e.feed(b"abcdef\r\nghijkl");
        e.sel_apply(sel(true, 0, 2, 1, 4));
        assert_eq!(e.sel_text(), "cd\r\nij");
    }

    #[test]
    fn extend_keeps_the_far_end() {
        let mut e = Emulator::new(20, 3);
        e.feed(b"0123456789abcdef");
        e.sel_apply(sel(false, 0, 2, 0, 5));
        e.sel_apply(SelOut::Extend(Anchor { row: 0, b: 9 }));
        assert_eq!(e.sel_text(), "2345678");
        e.sel_apply(sel(false, 0, 2, 0, 5));
        e.sel_apply(SelOut::Extend(Anchor { row: 0, b: 0 }));
        assert_eq!(e.sel_text(), "01234");
    }

    #[test]
    fn a_wide_character_selects_whole() {
        let mut e = Emulator::new(20, 3);
        e.feed("日本x".as_bytes());
        e.sel_apply(sel(false, 0, 1, 0, 3));
        assert_eq!(e.sel_text(), "日本");
    }

    #[test]
    fn the_end_boundary_cols_selects_to_the_row_end() {
        let mut e = Emulator::new(10, 3);
        e.feed(b"abcdefg");
        e.sel_apply(sel(false, 0, 3, 0, 10));
        assert_eq!(e.sel_text(), "defg");
    }

    #[test]
    fn sel_spans_follow_the_viewport() {
        let mut e = Emulator::new(10, 3);
        for i in 0..9 {
            e.feed(format!("l{i}\r\n").as_bytes());
        }
        e.feed(b"l9");
        e.scroll_view(2);
        let top = e.viewport_top();
        e.sel_apply(sel(false, top + 1, 0, top + 1, 2));
        assert_eq!(
            e.sel_spans(),
            vec![RowSpan {
                row: 1,
                col: 0,
                width: 2
            }]
        );
        e.sel_apply(sel(false, top - 2, 0, top - 2, 2));
        assert!(e.sel_spans().is_empty());
    }

    #[test]
    fn the_env_finds_words_lines_and_links() {
        let mut e = Emulator::new(40, 3);
        e.feed(b"git status\r\nsee https://example.com/x now");
        let env = e.sel_env();
        let w = env.word_at(0, 1, true).unwrap();
        assert_eq!(
            (w.start, w.end),
            (Anchor { row: 0, b: 0 }, Anchor { row: 0, b: 3 })
        );
        let l = env.link_at(1, 10).unwrap();
        assert_eq!(
            (l.start, l.end),
            (Anchor { row: 1, b: 4 }, Anchor { row: 1, b: 25 })
        );
        assert!(env.link_at(1, 1).is_none());
        let mut e = Emulator::new(10, 3);
        e.feed(b"aaaaaaaaaabbbbb");
        assert_eq!(e.sel_env().line_at(1), (0, 1));
    }

    #[test]
    fn an_osc8_link_is_found_from_the_cell() {
        let mut e = Emulator::new(40, 3);
        e.feed(b"x \x1b]8;;https://a.example/p\x1b\\click\x1b]8;;\x1b\\ y");
        let l = e.sel_env().link_at(0, 4).unwrap();
        assert_eq!(
            (l.start, l.end),
            (Anchor { row: 0, b: 2 }, Anchor { row: 0, b: 7 })
        );
    }

    #[test]
    fn the_env_reports_widths() {
        let mut e = Emulator::new(10, 3);
        e.feed("日a".as_bytes());
        let env = e.sel_env();
        assert_eq!(env.cell_width(0, 0), 2);
        assert_eq!(env.cell_width(0, 1), 0);
        assert_eq!(env.cell_width(0, 2), 1);
    }
}
