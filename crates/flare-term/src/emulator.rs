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
use flare_render::decor::{Decor, DecorInk, RowSpan};
use flare_render::grid::Grid;
use flare_shells::cwd::{has_control, parse_osc9};

use crate::alt_click::{MoveRow, MoveScreen, move_to_cell_sequence};
use crate::cells::{
    BufferLike, CellInfo, ScratchCell, buffer_row_cells, char_at_cell, logical_line,
};
use crate::click_caret::{ClickGate, MouseTracking, arrow_keys, caret_click_allowed, caret_delta};
use crate::find::{CountJob, FindMatch, FindOpts, Finder, index_of, scroll_to_show};
use crate::grid_map::snapshot_grid;
use crate::link_scan::{LinkCtx, LinkMark, LinkScanner, LinkTarget, Piece};
use crate::link_target::{TermPathAt, menu_link, shown_link_over};
use crate::links::find_links;
use crate::osc8::is_web_link;
use crate::osc52::{OSC52_MAX, Osc52, parse_osc52};
use crate::path_links::{Found, PathLinks, path_candidates};
use crate::replies::{Ground, colour_query_replies, xtversion_reply};
use crate::scan::{ScanEvent, Scanner};
use crate::sel_machine::{Anchor, SelEnv, SelOut, SelSpec, Span};
use crate::sel_text::selection_text;
use crate::selection_edit::{SelectionGate, selection_delete_allowed, selection_delete_keys};
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
    find_query: String,
    find_job: Option<CountJob>,
    /// A recount after output: the previous count stays on show until the new one is done, so
    /// the bar never flashes "none" or a partial number while the same query is counted again.
    find_recount: bool,
    find_hits: Vec<FindMatch>,
    find_active: Option<FindMatch>,
    links: LinkScanner,
    /// The marks of the last `link_marks` call; None before the first scan.
    link_last: Option<Vec<LinkMark>>,
}

/// What the find bar shows: the counted matches (at most `HIGHLIGHT_LIMIT`), the active match
/// and its place among the counted (None when it is not one of them).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindView {
    pub matches: Vec<FindMatch>,
    pub active: Option<FindMatch>,
    pub count: usize,
    pub index: Option<usize>,
    /// True when the count has walked the whole buffer, or no search runs.
    pub done: bool,
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
            find_query: String::new(),
            find_job: None,
            find_recount: false,
            find_hits: Vec::new(),
            find_active: None,
            links: LinkScanner::new(),
            link_last: None,
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
        if let Some(vrow) = row.checked_sub(self.emu.viewport_top())
            && vrow < self.emu.view_rows()
            && let Some(mark) = self.emu.link_at_view(vrow, col)
            && let (Some(a), Some(b)) = (mark.pieces.first(), mark.pieces.last())
        {
            return Some(Span {
                start: Anchor {
                    row: a.row,
                    b: a.col,
                },
                end: Anchor {
                    row: b.row,
                    b: b.col + b.width,
                },
            });
        }
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

    /// Selects one match: a Simple selection over its cells. The end is the last cell's
    /// boundary plus one, so a match ending at a row's last cell stays on that row.
    fn select_match(&mut self, m: FindMatch) {
        let cols = self.cols();
        if cols == 0 {
            return;
        }
        let last = m.row * cols + m.col + m.size.max(1) - 1;
        self.sel_apply(SelOut::Set(SelSpec {
            block: false,
            start: Anchor {
                row: m.row,
                b: m.col,
            },
            end: Anchor {
                row: last / cols,
                b: last % cols + 1,
            },
        }));
    }

    /// The match a search starts from: the active one, else a zero-size match at the selection's
    /// start (xterm starts from the terminal's selection).
    fn find_anchor(&self) -> Option<FindMatch> {
        if let Some(a) = self.find_active {
            return Some(a);
        }
        let ((row, b), _, _) = self.sel_bounds()?;
        Some(FindMatch {
            row,
            col: b,
            size: 0,
        })
    }

    /// Makes `found` the active match, selects it and says how far to scroll to show it.
    /// No match clears the active one and leaves the selection as it is.
    fn find_land(&mut self, found: Option<FindMatch>) -> Option<(FindMatch, Option<i32>)> {
        let Some(m) = found else {
            self.find_active = None;
            return None;
        };
        self.find_active = Some(m);
        self.select_match(m);
        let scroll = scroll_to_show(m.row, self.viewport_top(), self.view_rows());
        Some((m, scroll))
    }

    /// A new query: forgets the counted matches and starts a new count, but keeps the active
    /// match, so the incremental search that follows starts at it. No scroll, no selection.
    pub fn find_set(&mut self, query: &str) {
        if query.is_empty() {
            self.find_clear();
            return;
        }
        self.find_query = query.to_string();
        self.find_hits.clear();
        self.find_recount = false;
        self.find_restart();
    }

    fn find_restart(&mut self) {
        let total = self.total_rows();
        self.find_job =
            Finder::new(&self.find_query, FindOpts::default()).map(|f| CountJob::new(f, total));
    }

    /// Enter (`incremental: false`) and typing (`incremental: true`): the next match after the
    /// anchor. Returns the match and the scroll the caller applies as `scroll_view(-scroll)`.
    pub fn find_next(
        &mut self,
        query: &str,
        incremental: bool,
    ) -> Option<(FindMatch, Option<i32>)> {
        let f = Finder::new(query, FindOpts::default())?;
        let prev = self.find_anchor();
        let cols = self.cols();
        let found = {
            let b = TermBuf::all(&self.term);
            f.next(&b, cols, prev.as_ref(), !incremental)
        };
        self.find_land(found)
    }

    /// Shift+Enter and the up button: the match before the anchor.
    pub fn find_prev(&mut self, query: &str) -> Option<(FindMatch, Option<i32>)> {
        let f = Finder::new(query, FindOpts::default())?;
        let prev = self.find_anchor();
        let cols = self.cols();
        let found = {
            let b = TermBuf::all(&self.term);
            f.previous(&b, cols, prev.as_ref(), true)
        };
        self.find_land(found)
    }

    /// Runs one slice of the count job. True when the counted matches or the done state changed.
    pub fn find_step(&mut self, now: &dyn Fn() -> f64) -> bool {
        let cols = self.cols();
        let Some(job) = self.find_job.as_mut() else {
            return false;
        };
        let before_hits = self.find_hits.clone();
        let before_done = job.done();
        let done = {
            let b = TermBuf::all(&self.term);
            job.step(&b, cols, now)
        };
        if done || !self.find_recount {
            self.find_hits = job.results();
        }
        if done {
            self.find_recount = false;
        }
        self.find_hits != before_hits || done != before_done
    }

    /// A fresh count after output or a resize, WITHOUT scrolling or moving the active match.
    pub fn find_refresh(&mut self) {
        if !self.find_query.is_empty() {
            self.find_recount = true;
            self.find_restart();
        }
    }

    /// Closing the bar: the highlights and the active match go; the selection stays (D17).
    pub fn find_clear(&mut self) {
        self.find_query.clear();
        self.find_job = None;
        self.find_recount = false;
        self.find_hits.clear();
        self.find_active = None;
    }

    pub fn find_view(&self) -> FindView {
        FindView {
            matches: self.find_hits.clone(),
            active: self.find_active,
            count: self.find_hits.len(),
            index: self.find_active.and_then(|a| index_of(&self.find_hits, &a)),
            done: self.find_job.as_ref().is_none_or(|j| j.done()),
        }
    }

    /// The visible pieces on viewport rows: every counted match, then the active match's.
    pub fn find_spans(&self) -> (Vec<RowSpan>, Vec<RowSpan>) {
        let cols = self.cols();
        let top = self.viewport_top();
        let rows = self.view_rows();
        let all = self
            .find_hits
            .iter()
            .flat_map(|m| visible_pieces(*m, cols, top, rows))
            .collect();
        let active = self
            .find_active
            .map(|m| visible_pieces(m, cols, top, rows))
            .unwrap_or_default();
        (all, active)
    }
}

/// What a right click lands on (Prism Terminal's `termContextAt`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TermContext {
    pub selection: String,
    pub link: Option<LinkMark>,
    /// What the menu copies and opens for `link` (an OSC 8 label that shows another address: that one).
    pub link_text: Option<String>,
    pub path: Option<TermPathAt>,
}

impl Emulator {
    /// The marks on the viewport (D5). Only the logical lines on screen are scanned, and a line
    /// that did not change is not scanned again.
    pub fn link_marks(
        &mut self,
        paths: &mut PathLinks,
        owner: &str,
        cwd: Option<&str>,
        now_ms: u64,
        ink_key: u32,
    ) -> Vec<LinkMark> {
        let cols = self.cols();
        let rows = self.view_rows();
        if cols == 0 || rows == 0 {
            self.link_last = Some(Vec::new());
            return Vec::new();
        }
        let top = self.viewport_top();
        let last = top + rows - 1;
        let hist = self.history_size();
        let term = &self.term;
        let buf = TermBuf::window(term, top, last);
        let hyperlink = |row: usize, col: usize| {
            if col >= cols {
                return None;
            }
            let cell = &term.grid()[Point::new(abs_to_line(hist, row), Column(col))];
            cell.hyperlink()
                .map(|h| (h.id().to_string(), h.uri().to_string()))
        };
        let mut ctx = LinkCtx {
            paths,
            owner,
            cwd,
            now_ms,
            ink_key,
            hyperlink: &hyperlink,
        };
        let marks = self.links.scan(&buf, cols, top, last, &mut ctx);
        self.link_last = Some(marks.clone());
        marks
    }

    /// A path answer arrived: the lines that waited for it are scanned again.
    pub fn links_revisit(&mut self) {
        self.links.revisit();
    }

    /// Drops every cached scan.
    pub fn links_repaint(&mut self) {
        self.links.repaint();
    }

    pub fn links_hidden(&mut self, hidden: bool) {
        self.links.set_hidden(hidden);
    }

    /// Logical lines scanned so far (a test and the speed guard read it).
    pub fn lines_scanned(&self) -> usize {
        self.links.lines_scanned()
    }

    /// The mark under a viewport cell, from the last `link_marks` (None before the first scan).
    pub fn link_at_view(&self, row: usize, col: usize) -> Option<LinkMark> {
        let abs = self.viewport_top() + row;
        self.link_last
            .as_ref()?
            .iter()
            .find(|m| {
                m.pieces
                    .iter()
                    .any(|p| p.row == abs && col >= p.col && col < p.col + p.width)
            })
            .cloned()
    }

    /// The logical line of a mark and the char range the mark covers on it.
    fn mark_chars(&self, mark: &LinkMark) -> Option<(crate::cells::LogicalLine, usize, usize)> {
        let cols = self.cols();
        let (a, b): (&Piece, &Piece) = (mark.pieces.first()?, mark.pieces.last()?);
        let buf = TermBuf::window(&self.term, a.row, b.row);
        let ll = logical_line(&buf, cols, a.row);
        let s = (a.row - ll.first) * cols + a.col;
        let e = (b.row - ll.first) * cols + b.col + b.width;
        let from = *ll.index.get(s)?;
        let to = *ll.index.get(e.min(ll.index.len() - 1))?;
        Some((ll, from, to))
    }

    /// For a click: the address an OSC 8 label shows (Prism Terminal's `shownLinkOver`).
    pub fn shown_for(&self, mark: &LinkMark) -> Option<String> {
        if !matches!(mark.target, LinkTarget::Osc8 { .. }) {
            return None;
        }
        let (ll, from, to) = self.mark_chars(mark)?;
        shown_link_over(&ll.text, from, to)
    }

    /// What a right click lands on: the selection text, the link under the cell and, when there
    /// is none and a folder is known, a path that exists. Nothing is statted here.
    pub fn context_at(
        &self,
        row: usize,
        col: usize,
        paths: &mut PathLinks,
        owner: &str,
        cwd: Option<&str>,
        now_ms: u64,
    ) -> TermContext {
        let mut ctx = TermContext {
            selection: self.sel_text(),
            link: None,
            link_text: None,
            path: None,
        };
        let cols = self.cols();
        if cols == 0 || row >= self.view_rows() {
            return ctx;
        }
        let abs = self.viewport_top() + row;
        let mark = self
            .link_at_view(row, col)
            .or_else(|| self.plain_link_at(abs, col));
        if let Some(m) = mark
            && !matches!(m.target, LinkTarget::Path { .. })
        {
            let shown = self.shown_for(&m);
            ctx.link_text = Some(menu_link(&m.target, shown.as_deref()));
            ctx.link = Some(m);
            return ctx;
        }
        let Some(cwd) = cwd else { return ctx };
        let buf = TermBuf::window(&self.term, abs, abs);
        let ll = logical_line(&buf, cols, abs);
        let c = char_at_cell(&ll, cols, abs as i32, col as i32);
        let Some(cand) = path_candidates(&ll.text)
            .into_iter()
            .find(|p| p.start <= c && c < p.end)
        else {
            return ctx;
        };
        if let Found::Hit(f) = paths.find_path(owner, cwd, &cand.path, now_ms) {
            ctx.path = Some(TermPathAt {
                kind: f.hit.kind,
                abs: f.hit.abs,
                text: cand.path,
                base: f.base,
            });
        }
        ctx
    }

    /// A web link found in the line's text, for a right click before any scan ran.
    fn plain_link_at(&self, abs: usize, col: usize) -> Option<LinkMark> {
        let cols = self.cols();
        let buf = TermBuf::window(&self.term, abs, abs);
        let ll = logical_line(&buf, cols, abs);
        let c = char_at_cell(&ll, cols, abs as i32, col as i32);
        let chars: Vec<char> = ll.text.chars().collect();
        let m = find_links(&ll.text)
            .into_iter()
            .find(|m| m.start <= c && c < m.end)?;
        let n = ll.widths.len();
        let s = (0..n).find(|&i| ll.index[i] >= m.start && ll.widths[i] > 0)?;
        let e = (0..n)
            .find(|&i| ll.index[i] >= m.end)
            .unwrap_or(n)
            .max(s + 1);
        let mut pieces = Vec::new();
        let mut i = s;
        while i < e {
            let r = i / cols;
            let stop = e.min((r + 1) * cols);
            pieces.push(Piece {
                row: ll.first + r,
                col: i % cols,
                width: stop - i,
            });
            i = stop;
        }
        Some(LinkMark {
            target: LinkTarget::Web(chars[m.start..m.end.min(chars.len())].iter().collect()),
            pieces,
        })
    }
}

/// The inputs of one frame's decoration (Task 16d). `paths`, `owner`, `cwd`, `now_ms` and
/// `ink_key` go to the link scan; `hover` is the viewport cell under the pointer.
pub struct DecorReq<'a> {
    pub paths: &'a mut PathLinks,
    pub owner: &'a str,
    pub cwd: Option<&'a str>,
    pub now_ms: u64,
    pub ink_key: u32,
    pub hover: Option<(usize, usize)>,
    pub find_open: bool,
    pub ink: Option<DecorInk>,
}

impl Emulator {
    /// Everything one frame paints over the grid, on viewport rows. The link marks come from
    /// `link_marks`, so a frame scans only the logical lines that changed.
    pub fn decor(&mut self, req: &mut DecorReq) -> Decor {
        let marks = self.link_marks(&mut *req.paths, req.owner, req.cwd, req.now_ms, req.ink_key);
        let top = self.viewport_top();
        let rows = self.view_rows();
        let cols = self.cols();
        let on_view = |pieces: &[Piece]| -> Vec<RowSpan> {
            pieces
                .iter()
                .filter_map(|p| {
                    let r = p.row.checked_sub(top)?;
                    (r < rows && p.col < cols).then_some(RowSpan {
                        row: r,
                        col: p.col,
                        width: p.width.min(cols.saturating_sub(p.col)),
                    })
                })
                .collect()
        };
        let links: Vec<RowSpan> = marks.iter().flat_map(|m| on_view(&m.pieces)).collect();
        let hover = match req.hover {
            Some((row, col)) => self
                .link_at_view(row, col)
                .map(|m| on_view(&m.pieces))
                .unwrap_or_default(),
            None => Vec::new(),
        };
        let (find, find_active) = if req.find_open {
            self.find_spans()
        } else {
            (Vec::new(), Vec::new())
        };
        Decor {
            selection: self.sel_spans(),
            find,
            find_active,
            links,
            hover,
            ink: req.ink,
        }
    }
}

/// A match's pieces that fall on the viewport `top..top + rows`, as viewport rows.
impl Emulator {
    /// The absolute row and column of the cursor.
    fn cursor_abs(&self) -> (usize, usize) {
        let (line, col) = self.cursor();
        (line_to_abs(self.history_size(), Line(line)), col)
    }

    /// True when any row after the logical line ending at `last` holds a printed cell.
    fn text_below(&self, buf: &TermBuf, last: usize) -> bool {
        let cols = self.cols();
        let mut scratch = ScratchCell::default();
        (last + 1..total_rows(&self.term)).any(|y| {
            buffer_row_cells(buf.line(y), cols, &mut scratch)
                .is_some_and(|cells| cells.iter().any(|c| !c.chars.is_empty()))
        })
    }

    /// The keys a plain click at viewport (`row`, boundary `b`) sends, or "" (the caret click).
    pub fn click_caret_plan(
        &self,
        row: usize,
        b: usize,
        button: u8,
        clicks: u32,
        modifiers: bool,
        dragged: bool,
    ) -> String {
        let cols = self.cols();
        if cols == 0 || row >= self.view_rows() {
            return String::new();
        }
        let mode = self.mode();
        let buf = TermBuf::all(&self.term);
        let (cur_row, cur_col) = self.cursor_abs();
        let line = logical_line(&buf, cols, cur_row);
        let abs = self.viewport_top() + row;
        let off_line = abs < line.first || abs > line.last;
        let on_link = !off_line && {
            let env = TermSelEnv {
                emu: self,
                buf: TermBuf::all(&self.term),
            };
            let lc = env.line_cells(abs);
            let col = b.min(cols - 1);
            let at = (abs - lc.first) * cols + col;
            let hyper =
                at < lc.cells.len() && env.hyperlink_at(&lc, at).is_some_and(|h| is_web_link(&h.1));
            let c = char_at_cell(&line, cols, abs as i32, col as i32);
            hyper
                || find_links(&line.text)
                    .iter()
                    .any(|m| m.start <= c && c < m.end)
        };
        let gate = ClickGate {
            button,
            detail: clicks,
            modifiers,
            dragged,
            mouse_tracking: mode.mouse,
            alternate: mode.alt_screen,
            scrolled_back: self.is_scrolled_back(),
            cursor_hidden: !mode.cursor_visible,
            text_below: self.text_below(&buf, line.last),
            off_line,
            on_link,
        };
        if !caret_click_allowed(&gate) {
            return String::new();
        }
        let target = (abs - line.first) * cols + b;
        let cursor_cell = (cur_row - line.first) * cols + cur_col;
        arrow_keys(
            caret_delta(&line.widths, cursor_cell, target, line.text_end),
            mode.app_cursor,
        )
    }

    /// The arrow keys an Alt+click at viewport (`row`, `col`) sends (xterm `altClickMovesCursor`,
    /// `moveToCellSequence`), or "" while the view is scrolled back (xterm: `ybase === ydisp`).
    pub fn alt_click_plan(&self, row: usize, col: usize) -> String {
        let cols = self.cols();
        let rows = self.view_rows();
        if cols == 0 || rows == 0 || self.is_scrolled_back() {
            return String::new();
        }
        let mode = self.mode();
        let hist = self.history_size();
        let buf = TermBuf::window(&self.term, hist, hist + rows - 1);
        let mut scratch = ScratchCell::default();
        let screen: Vec<MoveRow> = (0..rows)
            .map(|r| {
                let line = buf.line(hist + r);
                MoveRow {
                    wrapped: line.is_some_and(|l| l.is_wrapped()),
                    cells: buffer_row_cells(line, cols, &mut scratch).unwrap_or_default(),
                }
            })
            .collect();
        let (line, cur_col) = self.cursor();
        let cur_row = usize::try_from(line).unwrap_or(0).min(rows - 1);
        let s = MoveScreen {
            cols,
            rows: &screen,
            has_scrollback: !mode.alt_screen,
            app_cursor: mode.app_cursor,
        };
        move_to_cell_sequence(
            &s,
            (cur_col.min(cols - 1), cur_row),
            (col.min(cols - 1), row.min(rows - 1)),
        )
    }

    /// The keys Backspace or Delete sends over the selection. None: not allowed (the key is the
    /// shell's). Some(""): allowed but nothing deletable. Some(keys): send them.
    pub fn selection_delete_plan(&self) -> Option<String> {
        let (s, e, _block) = self.sel_bounds()?;
        let cols = self.cols();
        if cols == 0 {
            return None;
        }
        let mode = self.mode();
        let buf = TermBuf::all(&self.term);
        let (cur_row, cur_col) = self.cursor_abs();
        let line = logical_line(&buf, cols, cur_row);
        let on_line = s.0 >= line.first && e.0 <= line.last && s.0 <= e.0;
        let gate = SelectionGate {
            mouse_tracking: mode.mouse,
            alternate: mode.alt_screen,
            scrolled_back: self.is_scrolled_back(),
            cursor_hidden: !mode.cursor_visible,
            text_below: self.text_below(&buf, line.last),
            on_line,
        };
        if !selection_delete_allowed(&gate) {
            return None;
        }
        let a = (s.0 - line.first) * cols + s.1;
        let z = (e.0 - line.first) * cols + e.1;
        let cursor_cell = (cur_row - line.first) * cols + cur_col;
        Some(selection_delete_keys(
            &line.widths,
            cursor_cell,
            a,
            z,
            line.text_end,
            mode.app_cursor,
        ))
    }
}

fn visible_pieces(m: FindMatch, cols: usize, top: usize, rows: usize) -> Vec<RowSpan> {
    m.pieces(cols)
        .into_iter()
        .filter_map(|(row, col, width)| {
            let r = row.checked_sub(top)?;
            (r < rows).then_some(RowSpan { row: r, col, width })
        })
        .collect()
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

    /// A clock that moves one unit per read, so a slice holds a handful of rows.
    fn find_clock() -> impl Fn() -> f64 {
        let t = std::cell::Cell::new(0.0f64);
        move || {
            t.set(t.get() + 1.0);
            t.get()
        }
    }

    /// Runs the count to its end, as the frame pump does.
    fn count_all(e: &mut Emulator) {
        let now = find_clock();
        for _ in 0..100_000 {
            if e.find_view().done {
                return;
            }
            e.find_step(&now);
        }
        panic!("the count never finished");
    }

    #[test]
    fn typing_selects_the_first_match_and_keeps_it_while_it_still_matches() {
        let mut e = Emulator::new(20, 3);
        e.feed(b"foo bar foo");
        for q in ["f", "fo", "foo"] {
            e.find_set(q);
            let (m, _) = e.find_next(q, true).unwrap();
            assert_eq!((m.row, m.col), (0, 0));
        }
    }

    #[test]
    fn enter_after_typing_moves_to_the_second() {
        let mut e = Emulator::new(20, 3);
        e.feed(b"foo bar foo");
        e.find_set("foo");
        e.find_next("foo", true);
        let (m, _) = e.find_next("foo", false).unwrap();
        assert_eq!((m.row, m.col), (0, 8));
    }

    #[test]
    fn enter_steps_and_wraps() {
        let mut e = Emulator::new(20, 3);
        e.feed(b"foo foo foo");
        let got: Vec<usize> = (0..4)
            .map(|_| e.find_next("foo", false).unwrap().0.col)
            .collect();
        assert_eq!(got, vec![0, 4, 8, 0]);
    }

    #[test]
    fn previous_steps_back() {
        let mut e = Emulator::new(20, 3);
        e.feed(b"foo foo foo");
        e.find_next("foo", false);
        e.find_next("foo", false);
        let (m, _) = e.find_prev("foo").unwrap();
        assert_eq!(m.col, 0);
    }

    #[test]
    fn the_match_is_selected() {
        let mut e = Emulator::new(20, 3);
        e.feed(b"foo bar foo");
        e.find_next("foo", false);
        assert_eq!(e.sel_text(), "foo");
    }

    #[test]
    fn a_match_outside_the_view_asks_for_a_scroll_to_the_middle() {
        let mut e = Emulator::new(10, 3);
        e.feed(b"foo\r\n");
        for _ in 0..40 {
            e.feed(b"row\r\n");
        }
        let top = e.viewport_top();
        let (m, scroll) = e.find_next("foo", false).unwrap();
        assert_eq!(m.row, 0);
        assert_eq!(scroll, crate::find::scroll_to_show(0, top, e.view_rows()));
        assert!(scroll.unwrap() < 0);
    }

    #[test]
    fn a_match_in_view_asks_for_no_scroll() {
        let mut e = Emulator::new(20, 3);
        e.feed(b"foo");
        assert_eq!(e.find_next("foo", false).unwrap().1, None);
    }

    #[test]
    fn the_count_grows_with_steps_and_caps_at_1000() {
        let mut e = Emulator::new(20, 3);
        for _ in 0..3000 {
            e.feed(b"foo\r\n");
        }
        e.find_set("foo");
        count_all(&mut e);
        let v = e.find_view();
        assert_eq!(v.count, 1000);
        assert!(v.done);
        // The 1001st match is active; it is not among the counted, so it has no index.
        for _ in 0..1001 {
            e.find_next("foo", false);
        }
        assert_eq!(e.find_view().index, None);
    }

    #[test]
    fn refresh_keeps_the_active_match_and_does_not_scroll() {
        let mut e = Emulator::new(20, 10);
        e.feed(b"foo");
        e.find_set("foo");
        e.find_next("foo", false);
        let active = e.find_view().active;
        e.feed(b"\r\nfoo");
        e.find_refresh();
        assert_eq!(e.find_view().active, active);
        assert_eq!(e.display_offset(), 0);
        count_all(&mut e);
        assert_eq!(e.find_view().count, 2);
    }

    #[test]
    fn a_recount_after_output_keeps_the_old_count_until_it_is_done() {
        let mut e = Emulator::new(20, 10);
        e.feed(b"foo\r\nfoo");
        e.find_set("foo");
        count_all(&mut e);
        assert_eq!(e.find_view().count, 2);
        e.feed(b"\r\nfoo");
        e.find_refresh();
        // Counting again: the bar keeps showing the last full count, never 0 or a partial one.
        let v = e.find_view();
        assert!(!v.done);
        assert_eq!(v.count, 2);
        count_all(&mut e);
        assert_eq!(e.find_view().count, 3);
        // A new query starts from nothing.
        e.find_set("fo");
        assert_eq!(e.find_view().count, 0);
    }

    #[test]
    fn clear_drops_highlights_but_not_the_selection() {
        let mut e = Emulator::new(20, 3);
        e.feed(b"foo bar");
        e.find_set("foo");
        count_all(&mut e);
        e.find_next("foo", false);
        assert!(!e.find_spans().0.is_empty());
        e.find_clear();
        assert_eq!(e.find_spans(), (Vec::new(), Vec::new()));
        assert!(e.has_selection());
    }

    #[test]
    fn highlights_cover_the_alternate_screen() {
        let mut e = Emulator::new(20, 3);
        e.feed(b"\x1b[?1049h");
        e.feed(b"foo");
        e.find_set("foo");
        count_all(&mut e);
        e.find_next("foo", false);
        assert_eq!(
            e.find_spans().0,
            vec![RowSpan {
                row: 0,
                col: 0,
                width: 3
            }]
        );
    }

    #[test]
    fn an_empty_query_clears() {
        let mut e = Emulator::new(20, 3);
        e.feed(b"foo");
        e.find_set("foo");
        count_all(&mut e);
        e.find_next("foo", false);
        e.find_set("");
        let v = e.find_view();
        assert!(v.matches.is_empty());
        assert_eq!(v.active, None);
        assert_eq!(v.count, 0);
    }

    fn marks(e: &mut Emulator) -> Vec<LinkMark> {
        e.link_marks(&mut PathLinks::new(), "t1", None, 0, 1)
    }

    #[test]
    fn marks_cover_the_viewport_only() {
        let mut e = Emulator::new(40, 4);
        for i in 0..40 {
            let nl = if i < 39 { "\r\n" } else { "" };
            e.feed(format!("see https://a.io/{i}{nl}").as_bytes());
        }
        let m = marks(&mut e);
        assert!(m.len() <= 4 && !m.is_empty());
        assert_eq!(e.lines_scanned(), 4);
    }

    #[test]
    fn a_wrapped_link_is_one_mark_with_two_pieces() {
        let mut e = Emulator::new(10, 4);
        e.feed(b"https://example.com/abc");
        let m = marks(&mut e);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].pieces.len(), 3);
        assert_eq!(
            m[0].target,
            LinkTarget::Web("https://example.com/abc".into())
        );
    }

    #[test]
    fn an_osc8_link_comes_from_the_cell() {
        let mut e = Emulator::new(40, 3);
        e.feed(b"\x1b]8;;https://x.io/a\x07label\x1b]8;;\x07");
        let m = marks(&mut e);
        assert_eq!(m.len(), 1);
        assert_eq!(
            m[0].target,
            LinkTarget::Osc8 {
                uri: "https://x.io/a".into()
            }
        );
        assert_eq!(
            m[0].pieces,
            vec![Piece {
                row: 0,
                col: 0,
                width: 5
            }]
        );
    }

    #[test]
    fn an_alternate_screen_osc8_label_is_marked() {
        let mut e = Emulator::new(40, 3);
        e.feed(b"\x1b[?1049h\x1b]8;;https://x.io/a\x07label\x1b]8;;\x07");
        assert_eq!(marks(&mut e).len(), 1);
    }

    #[test]
    fn a_hyperlink_does_not_survive_being_overwritten() {
        let mut e = Emulator::new(40, 3);
        e.feed(b"\x1b]8;;https://x.io/a\x07label\x1b]8;;\x07");
        assert_eq!(marks(&mut e).len(), 1);
        e.feed(b"\rplain");
        assert!(marks(&mut e).is_empty());
    }

    #[test]
    fn context_over_a_link_gives_the_whole_wrapped_url() {
        let mut e = Emulator::new(10, 4);
        e.feed(b"https://example.com/abc");
        marks(&mut e);
        let c = e.context_at(1, 2, &mut PathLinks::new(), "t1", None, 0);
        assert_eq!(c.link_text.as_deref(), Some("https://example.com/abc"));
        assert!(c.link.is_some() && c.path.is_none());
    }

    fn primed(e: &mut Emulator) -> PathLinks {
        let mut pl = PathLinks::new();
        e.link_marks(&mut pl, "t1", Some("C:\\w"), 0, 1);
        pl.advance(40);
        if let Some(ask) = pl.take_ask() {
            let hits = ask
                .paths
                .iter()
                .map(|p| {
                    Some(crate::path_open::PathHit {
                        kind: crate::path_open::PathKind::File,
                        abs: format!("C:\\w\\{p}"),
                    })
                })
                .collect();
            pl.answer(&ask, Some(hits), 50);
        }
        pl
    }

    #[test]
    fn context_over_a_path_that_exists_offers_it() {
        let mut e = Emulator::new(40, 3);
        e.feed(b"see a.txt here");
        let mut pl = primed(&mut e);
        let c = e.context_at(0, 5, &mut pl, "t1", Some("C:\\w"), 60);
        assert!(c.link.is_none());
        let p = c.path.expect("a path");
        assert_eq!((p.text.as_str(), p.base.as_str()), ("a.txt", "C:\\w"));
        let c = e.context_at(0, 1, &mut pl, "t1", Some("C:\\w"), 60);
        assert!(c.link.is_none() && c.path.is_none());
    }

    #[test]
    fn context_over_an_unknown_path_offers_nothing_and_stats_nothing() {
        let mut e = Emulator::new(40, 3);
        e.feed(b"see a.txt here");
        let mut pl = PathLinks::new();
        let c = e.context_at(0, 5, &mut pl, "t1", Some("C:\\w"), 0);
        assert!(c.path.is_none());
        pl.advance(100);
        assert!(pl.take_ask().is_none());
    }

    #[test]
    fn context_carries_the_selection() {
        let mut e = Emulator::new(40, 3);
        e.feed(b"hello world");
        e.sel_apply(sel(false, 0, 0, 0, 5));
        let c = e.context_at(0, 1, &mut PathLinks::new(), "t1", None, 0);
        assert_eq!(c.selection, "hello");
    }

    #[test]
    fn an_osc8_label_showing_another_host_copies_what_it_shows() {
        let mut e = Emulator::new(60, 3);
        e.feed(b"\x1b]8;;https://evil.io\x07see https://github.com/x\x1b]8;;\x07");
        marks(&mut e);
        let c = e.context_at(0, 1, &mut PathLinks::new(), "t1", None, 0);
        assert_eq!(c.link_text.as_deref(), Some("https://github.com/x"));
    }

    fn decor_of(
        e: &mut Emulator,
        pl: &mut PathLinks,
        hover: Option<(usize, usize)>,
        find_open: bool,
    ) -> Decor {
        e.decor(&mut DecorReq {
            paths: pl,
            owner: "t1",
            cwd: None,
            now_ms: 0,
            ink_key: 1,
            hover,
            find_open,
            ink: None,
        })
    }

    #[test]
    fn selection_find_and_links_all_appear_in_one_decor() {
        let mut e = Emulator::new(40, 4);
        e.feed(b"see https://example.com/x alpha\r\nalpha");
        e.find_set("alpha");
        count_all(&mut e);
        e.find_next("alpha", true);
        e.sel_apply(sel(false, 0, 0, 0, 2));
        let d = decor_of(&mut e, &mut PathLinks::new(), None, true);
        assert!(!d.selection.is_empty());
        assert!(!d.find.is_empty());
        assert!(!d.find_active.is_empty());
        assert!(!d.links.is_empty());
    }

    #[test]
    fn find_pieces_appear_only_while_the_bar_is_open() {
        let mut e = Emulator::new(40, 4);
        e.feed(b"alpha beta alpha");
        e.find_set("alpha");
        count_all(&mut e);
        e.find_next("alpha", true);
        let closed = decor_of(&mut e, &mut PathLinks::new(), None, false);
        assert!(closed.find.is_empty() && closed.find_active.is_empty());
        let open = decor_of(&mut e, &mut PathLinks::new(), None, true);
        assert!(!open.find.is_empty() && !open.find_active.is_empty());
    }

    #[test]
    fn rows_are_viewport_rows() {
        let mut e = Emulator::new(10, 3);
        for i in 0..9 {
            e.feed(format!("l{i}\r\n").as_bytes());
        }
        e.feed(b"l9");
        e.scroll_view(3);
        let top = e.viewport_top();
        e.sel_apply(sel(false, top + 1, 0, top + 1, 2));
        let d = decor_of(&mut e, &mut PathLinks::new(), None, false);
        assert_eq!(
            d.selection,
            vec![RowSpan {
                row: 1,
                col: 0,
                width: 2
            }]
        );
    }

    #[test]
    fn a_hovered_link_is_in_hover_too() {
        let mut e = Emulator::new(40, 3);
        e.feed(b"see https://a.io/x now");
        let d = decor_of(&mut e, &mut PathLinks::new(), Some((0, 6)), false);
        assert!(!d.links.is_empty());
        assert_eq!(d.hover, d.links);
        let d = decor_of(&mut e, &mut PathLinks::new(), Some((0, 0)), false);
        assert!(d.hover.is_empty());
    }

    #[test]
    fn a_big_scrollback_frame_stays_fast() {
        let mut e = Emulator::new(80, 24);
        for i in 0..10_200 {
            e.feed(format!("https://x.io/{i}\r\n").as_bytes());
        }
        let mut pl = PathLinks::new();
        decor_of(&mut e, &mut pl, None, false);
        let first = e.lines_scanned();
        assert!(first <= 24, "the first frame scanned {first} lines");
        for _ in 0..9 {
            decor_of(&mut e, &mut pl, None, false);
        }
        assert_eq!(e.lines_scanned(), first);
    }

    // ---- Task 16e: click and delete plans ----

    const L: &str = "\x1b[D";

    fn prompt(e: &mut Emulator, text: &str) {
        e.feed(format!("PS> {text}").as_bytes());
    }

    #[test]
    fn a_click_right_of_the_text_moves_the_caret_to_the_end() {
        let mut e = Emulator::new(40, 5);
        prompt(&mut e, "abc");
        assert_eq!(e.cursor().1, 7);
        assert_eq!(e.click_caret_plan(0, 20, 0, 1, false, false), "");
        assert_eq!(
            e.click_caret_plan(0, 5, 0, 1, false, false),
            format!("{L}{L}")
        );
    }

    #[test]
    fn a_click_on_a_wide_character_counts_it_once() {
        let mut e = Emulator::new(40, 5);
        prompt(&mut e, "日本");
        assert_eq!(e.cursor().1, 8);
        assert_eq!(e.click_caret_plan(0, 6, 0, 1, false, false), L);
    }

    #[test]
    fn an_alt_click_moves_the_cursor_by_arrows_as_xterm() {
        let mut e = Emulator::new(40, 5);
        prompt(&mut e, "abc");
        // Same row: five cells left.
        assert_eq!(e.alt_click_plan(0, 2), L.repeat(5));
        // The row below on the normal screen: rights counted across the wrap, (40 - 7) + 1 - 1.
        assert_eq!(e.alt_click_plan(1, 0), "\x1b[C".repeat(33));
        // Unlike the plain click, it is not limited to the typed line or to printed text.
        assert_eq!(e.alt_click_plan(0, 30), "\x1b[C".repeat(23));
    }

    #[test]
    fn an_alt_click_on_the_alternate_screen_goes_down_then_across() {
        let mut e = Emulator::new(40, 5);
        e.feed(b"\x1b[?1049h\x1b[?1h\x1b[H");
        assert_eq!(e.alt_click_plan(2, 3), "\x1bOB\x1bOB\x1bOC\x1bOC\x1bOC");
    }

    #[test]
    fn an_alt_click_while_scrolled_back_sends_nothing() {
        let mut e = Emulator::new(20, 3);
        for i in 0..10 {
            e.feed(format!("line {i}\r\n").as_bytes());
        }
        e.scroll_view(2);
        assert!(e.is_scrolled_back());
        assert_eq!(e.alt_click_plan(0, 0), "");
    }

    #[test]
    fn every_refusal_sends_nothing() {
        let plain = |e: &Emulator| e.click_caret_plan(0, 5, 0, 1, false, false);
        let mut e = Emulator::new(40, 5);
        prompt(&mut e, "abc");
        assert!(!plain(&e).is_empty());
        assert_eq!(e.click_caret_plan(0, 5, 1, 1, false, false), "");
        assert_eq!(e.click_caret_plan(0, 5, 0, 2, false, false), "");
        assert_eq!(e.click_caret_plan(0, 5, 0, 1, true, false), "");
        assert_eq!(e.click_caret_plan(0, 5, 0, 1, false, true), "");
        let mut m = Emulator::new(40, 5);
        prompt(&mut m, "abc");
        m.feed(b"\x1b[?1000h");
        assert_eq!(plain(&m), "");
        let mut a = Emulator::new(40, 5);
        prompt(&mut a, "abc");
        a.feed(b"\x1b[?1049h");
        a.feed(b"PS> abc");
        assert_eq!(plain(&a), "");
        let mut h = Emulator::new(40, 5);
        prompt(&mut h, "abc");
        h.feed(b"\x1b[?25l");
        assert_eq!(plain(&h), "");
        let mut b = Emulator::new(40, 5);
        for _ in 0..8 {
            b.feed(b"x\r\n");
        }
        prompt(&mut b, "abc");
        b.scroll_view(2);
        assert_eq!(b.click_caret_plan(4, 5, 0, 1, false, false), "");
        let mut t = Emulator::new(40, 5);
        prompt(&mut t, "abc");
        t.feed(b"\x1b[3;1Hbelow\x1b[1;8H");
        assert_eq!(plain(&t), "");
        let mut o = Emulator::new(40, 5);
        o.feed(b"older\r\n");
        prompt(&mut o, "abc");
        assert_eq!(o.click_caret_plan(0, 2, 0, 1, false, false), "");
        let mut l = Emulator::new(60, 5);
        l.feed(b"PS> see https://example.com/a ok");
        assert_eq!(l.click_caret_plan(0, 12, 0, 1, false, false), "");
        assert!(!l.click_caret_plan(0, 5, 0, 1, false, false).is_empty());
    }

    #[test]
    fn the_application_cursor_form_is_used_when_asked() {
        let mut e = Emulator::new(40, 5);
        e.feed(b"\x1b[?1h");
        prompt(&mut e, "abc");
        assert_eq!(e.click_caret_plan(0, 6, 0, 1, false, false), "\x1bOD");
    }

    #[test]
    fn a_click_across_a_wrapped_prompt_line_is_one_line() {
        let mut e = Emulator::new(10, 5);
        prompt(&mut e, "abcdefghij");
        assert_eq!(e.cursor(), (1, 4));
        let keys = e.click_caret_plan(0, 6, 0, 1, false, false);
        assert_eq!(keys, L.repeat(8));
    }

    #[test]
    fn selection_delete_walks_to_the_end_then_deletes() {
        let mut e = Emulator::new(40, 5);
        e.feed(b"echo hello");
        e.sel_apply(sel(false, 0, 7, 0, 10));
        assert_eq!(e.selection_delete_plan(), Some("\x7f\x7f\x7f".to_string()));
        let mut m = Emulator::new(40, 5);
        m.feed(b"echo hello\x1b[4D");
        m.sel_apply(sel(false, 0, 7, 0, 10));
        assert_eq!(
            m.selection_delete_plan(),
            Some(format!("{}{}", "\x1b[C".repeat(4), "\x7f".repeat(3)))
        );
    }

    #[test]
    fn selection_delete_is_refused_off_the_edited_line() {
        let mut e = Emulator::new(40, 5);
        e.feed(b"older line\r\nPS> abc");
        e.sel_apply(sel(false, 0, 0, 0, 5));
        assert_eq!(e.selection_delete_plan(), None);
    }

    #[test]
    fn selection_delete_is_refused_in_the_cases_of_the_gate() {
        let setup = |extra: &[u8]| {
            let mut e = Emulator::new(40, 5);
            e.feed(b"echo hello");
            e.feed(extra);
            e.sel_apply(sel(false, 0, 7, 0, 10));
            e
        };
        assert!(setup(b"").selection_delete_plan().is_some());
        assert_eq!(setup(b"\x1b[?1000h").selection_delete_plan(), None);
        assert_eq!(setup(b"\x1b[?25l").selection_delete_plan(), None);
        assert_eq!(
            setup(b"\x1b[3;1Hbelow\x1b[1;11H").selection_delete_plan(),
            None
        );
        let mut a = Emulator::new(40, 5);
        a.feed(b"\x1b[?1049hecho hello");
        a.sel_apply(sel(false, 0, 7, 0, 10));
        assert_eq!(a.selection_delete_plan(), None);
        let mut s = Emulator::new(40, 5);
        for _ in 0..8 {
            s.feed(b"x\r\n");
        }
        s.feed(b"echo hello");
        let top = s.viewport_top();
        s.sel_apply(sel(false, top + 4, 7, top + 4, 10));
        assert!(s.selection_delete_plan().is_some());
        s.scroll_view(2);
        assert_eq!(s.selection_delete_plan(), None);
    }

    #[test]
    fn nothing_deletable_is_allowed_and_empty() {
        let mut e = Emulator::new(40, 5);
        e.feed(b"echo hello");
        e.sel_apply(sel(false, 0, 20, 0, 25));
        assert_eq!(e.selection_delete_plan(), Some(String::new()));
    }
}
