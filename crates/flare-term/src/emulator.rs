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
use flare_shells::cwd::{has_control, parse_cwd_report};

use crate::alt_click::{MoveRow, MoveScreen, move_to_cell_sequence};
use crate::cells::{
    BufferLike, CellInfo, LineLike, ScratchCell, buffer_row_cells, char_at_cell, logical_line,
    logical_line_in,
};
use crate::click_caret::{ClickGate, MouseTracking, arrow_keys, caret_click_allowed, caret_delta};
use crate::dev_server::{ClearWatch, DevState, LocalServer, SCAN_GAP_MS, local_server_in};
use crate::find::{CountJob, Dir, FindMatch, FindOpts, Finder, StepJob, index_of, scroll_to_show};
use crate::grid_map::{snapshot_grid, snapshot_grid_into};
use crate::link_scan::{LinkCtx, LinkMark, LinkScanner, LinkTarget, Piece};
use crate::link_target::{TermPathAt, menu_link, shown_link_over};
use crate::links::find_links;
use crate::osc8::is_web_link;
use crate::osc52::{OSC52_MAX, Osc52, parse_osc52};
use crate::path_links::{PathLinks, path_candidates_in};
use crate::replies::{
    Ground, colour_query_replies, decrqm_reply, dsr_theme_reply, ground_mode, xtversion_reply,
};
use crate::scan::{Erase, ScanEvent, Scanner};
use crate::sel_machine::{Anchor, SelEnv, SelOut, SelSpec, Span};
use crate::sel_text::selection_text;
use crate::selection_edit::{SelectionGate, selection_delete_allowed, selection_delete_keys};
use crate::term_buf::{TermBuf, abs_to_line, line_to_abs, total_rows, viewport_top};
use crate::version::{ALACRITTY_TERMINAL_VERSION, FLARE_TERM_VERSION};
use crate::word_at::word_range;
use flare_theme::reports::{THEME_REPORTS_OFF, ThemeReports, note_theme_mode, theme_reports};

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
    /// OSC 9;9 folder report (and OSC 7 on macOS).
    Cwd(String),
    /// OSC 52 write, at most `OSC52_MAX` chars.
    ClipboardWrite(String),
    /// OSC 777 or OSC 7501 passed up raw (the agent code parses it).
    Osc {
        ident: u32,
        data: String,
    },
    Started,
    StartFailed,
    Exited(Option<u32>),
    /// One per drained batch of pty output (the handle queues it, never the emulator).
    Output,
    /// A person typed: a key, text or a paste (PT `markTouched`).
    Typed,
    /// The encoded bytes of Enter, Esc, Ctrl+C or a digit 1 to 9 that a user key sent.
    AnswerKey(String),
    /// `TermHandle::resize` ran (output we caused by a refit is not work).
    Resized,
    /// A resuming tab's terminal is shown now (the agent painted, a key, an exit, the ceiling).
    Revealed,
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
    /// What the program asked of theme reports (?2031h) and the mode it was last told.
    reports: ThemeReports,
    find_query: String,
    find_job: Option<CountJob>,
    /// A recount after output: the previous count stays on show until the new one is done, so
    /// the bar never flashes "none" or a partial number while the same query is counted again.
    find_recount: bool,
    find_hits: Vec<FindMatch>,
    find_active: Option<FindMatch>,
    /// The search Enter, the arrows or typing started, walked a slice at a time; None when none runs.
    search: Option<StepJob>,
    links: LinkScanner,
    /// Hears the screen being cleared (Surfaces stage 7, the Preview chip).
    dev_clear: ClearWatch,
    dev_clears_applied: u64,
    /// The newest local server address found on the live screen.
    dev_found: Option<LocalServer>,
    /// Every local address on the live screen at the last reading, with the screen row (0 = top) its line ended on.
    /// Screen rows, not absolute ones: those shift with every line once the scrollback is full.
    dev_prev: Vec<(String, usize)>,
    /// Changes when `dev_found` changes to another address or is cleared, never otherwise.
    dev_seq: u64,
    /// Moves when the held address is printed again (see `DevState::reprint`).
    dev_reprint: u64,
    /// Output arrived since the screen was last read for an address.
    dev_dirty: bool,
    dev_scanned: Option<Instant>,
    /// The marks of the last `link_marks` call; None before the first scan.
    link_last: Option<Vec<LinkMark>>,
    /// Bumped on every change of the selection (a copy in flight reads it). A redraw or a scroll that
    /// moves the selection does not bump it.
    sel_gen: u64,
}

/// What the find bar shows: the counted matches (at most `HIGHLIGHT_LIMIT`), the active match
/// and its place among the counted (None when it is not one of them).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindView {
    pub matches: Vec<FindMatch>,
    pub active: Option<FindMatch>,
    pub count: usize,
    pub index: Option<usize>,
    /// True when the count has walked the whole buffer.
    pub done: bool,
    /// True while a search (Enter, the arrows, typing) is walking; the active match changes only
    /// when it lands, so the readout keeps its text until then.
    pub searching: bool,
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
            reports: THEME_REPORTS_OFF,
            find_query: String::new(),
            find_job: None,
            find_recount: false,
            find_hits: Vec::new(),
            find_active: None,
            search: None,
            links: LinkScanner::new(),
            dev_clear: ClearWatch::new(),
            dev_clears_applied: 0,
            dev_found: None,
            dev_prev: Vec::new(),
            dev_seq: 0,
            dev_reprint: 0,
            dev_dirty: false,
            dev_scanned: None,
            link_last: None,
            sel_gen: 0,
        }
    }

    /// The local dev server address on the live screen (the newest), and a counter that moves only when that changes.
    /// The screen is read here when output has arrived since the last reading and at least `SCAN_GAP_MS` have passed;
    /// otherwise the last answer stands and [`Emulator::dev_pending`] says a reading is still owed.
    pub fn dev_state(&mut self) -> DevState {
        if self.dev_clear.clears() != self.dev_clears_applied {
            self.dev_clears_applied = self.dev_clear.clears();
            self.dev_prev.clear();
            if self.dev_found.take().is_some() {
                self.dev_seq += 1;
            }
        }
        if self.dev_dirty
            && self
                .dev_scanned
                .is_none_or(|t| t.elapsed().as_millis() as u64 >= SCAN_GAP_MS)
        {
            self.dev_scan();
        }
        DevState {
            server: self.dev_found.clone(),
            seq: self.dev_seq,
            reprint: self.dev_reprint,
        }
    }

    /// Output arrived that `dev_state` has not read yet (it was asked again too soon).
    pub fn dev_pending(&self) -> bool {
        self.dev_dirty
    }

    /// Reads the live screen for the newest local server address. The rows are the screen's own (history is never
    /// read: a cleared screen is kept there), the row the cursor is on is left out (it is what is being typed, or a
    /// suggestion painted after it) and so is the alternate screen. An address is NEWS when it differs from the one
    /// held and was not on the screen at the last reading; the same address printed again is not news but moves
    /// `reprint` (a server restarted on its port, which the chip needs to hear about once it is gone). What is compared
    /// is the screen's own rows and the text, so a full scrollback, where every line shifts every absolute row, changes
    /// nothing.
    fn dev_scan(&mut self) {
        self.dev_dirty = false;
        self.dev_scanned = Some(Instant::now());
        let rows = self.view_rows();
        let cols = self.cols();
        if rows == 0 || cols == 0 || self.mode().alt_screen {
            return;
        }
        let hist = self.history_size();
        let (cursor_row, _) = self.cursor();
        let cursor = hist + usize::try_from(cursor_row).unwrap_or(0);
        let (top, last) = (hist, hist + rows - 1);
        let buf = TermBuf::window_in(&self.term, top, last, false);
        let mut cur: Vec<(String, usize)> = Vec::new();
        let mut best: Option<LocalServer> = None;
        let mut y = top;
        while y <= last {
            let ll = logical_line_in(&buf, cols, y, false);
            // The group of rows the cursor is in is input, not output.
            if !(ll.first..=ll.last).contains(&cursor)
                && let Some(found) = local_server_in(&ll.text)
            {
                cur.push((found.url.clone(), ll.last.saturating_sub(top)));
                best = Some(found);
            }
            y = ll.last.max(y) + 1;
        }
        let prev = std::mem::replace(&mut self.dev_prev, cur);
        let Some(found) = best else { return };
        match &self.dev_found {
            Some(old) if *old == found => {
                let count = |v: &[(String, usize)]| v.iter().filter(|e| e.0 == found.url).count();
                let lowest = |v: &[(String, usize)]| {
                    v.iter().filter(|e| e.0 == found.url).map(|e| e.1).max()
                };
                let (now_n, was_n) = (count(&self.dev_prev), count(&prev));
                // Another print of it, or one lower than the lowest was (a scroll only moves it up).
                if now_n > was_n || lowest(&self.dev_prev) > lowest(&prev) {
                    self.dev_reprint += 1;
                }
            }
            Some(_) if prev.iter().any(|e| e.0 == found.url) => {}
            _ => {
                self.dev_found = Some(found);
                self.dev_seq += 1;
            }
        }
    }

    /// What the program asked of theme reports, and the mode it was last told.
    pub fn theme_reports(&self) -> ThemeReports {
        self.reports
    }

    pub fn set_theme_reports(&mut self, s: ThemeReports) {
        self.reports = s;
    }

    /// What OSC 10 and 11 queries are answered with.
    pub fn set_colours(&mut self, fg: &str, bg: &str) {
        self.fg = fg.to_string();
        self.bg = bg.to_string();
    }

    pub fn colours_are(&self, fg: &str, bg: &str) -> bool {
        self.fg == fg && self.bg == bg
    }

    /// Scans the chunk, then feeds `vte` in slices that end at each scanner event, acting on the
    /// event between slices. Returns the events in stream order.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<TermEvent> {
        self.dev_clear.feed(bytes);
        self.dev_dirty = true;
        let had_sel = self.term.selection.is_some();
        let scanned = self.scanner.scan(bytes);
        let mut out = Vec::new();
        let mut at = 0;
        for ev in scanned {
            if let ScanEvent::SelectionGuard { start, end, erase } = ev {
                let (start, end) = (start.min(bytes.len()), end.min(bytes.len()));
                if start > at {
                    self.processor.advance(&mut self.term, &bytes[at..start]);
                    at = start;
                }
                let saved = self.term.selection.clone();
                let moved = if erase == Erase::Screen {
                    self.viewport_used_lines()
                } else {
                    0
                };
                if end > at {
                    self.processor.advance(&mut self.term, &bytes[at..end]);
                    at = end;
                }
                self.restore_selection(saved, moved);
                continue;
            }
            let end = match &ev {
                ScanEvent::Osc { end, .. }
                | ScanEvent::Xtversion { end }
                | ScanEvent::ThemeQuery { end }
                | ScanEvent::Decrqm { end, .. }
                | ScanEvent::PrivateMode { end, .. }
                | ScanEvent::Ris { end } => (*end).min(bytes.len()),
                ScanEvent::SelectionGuard { .. } => continue,
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
        if had_sel != self.term.selection.is_some() {
            self.sel_changed();
        }
        out
    }

    fn act(&mut self, ev: ScanEvent, out: &mut Vec<TermEvent>) {
        match ev {
            ScanEvent::SelectionGuard { .. } => {}
            ScanEvent::ThemeQuery { .. } => {
                let mode = ground_mode(&self.bg);
                out.push(TermEvent::Reply(dsr_theme_reply(mode).into_bytes()));
                self.reports = note_theme_mode(self.reports, mode);
            }
            ScanEvent::Decrqm { mode, .. } => {
                if mode == 2031 {
                    out.push(TermEvent::Reply(
                        decrqm_reply(2031, self.reports.on).into_bytes(),
                    ));
                }
            }
            ScanEvent::PrivateMode { set, .. } => {
                self.reports = theme_reports(self.reports, set, ground_mode(&self.bg));
            }
            ScanEvent::Ris { .. } => self.reports = THEME_REPORTS_OFF,
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
                    // OSC 9;9 everywhere; on macOS also OSC 7 (`parse_cwd_report`).
                    7 | 9 => {
                        if let Some(p) = parse_cwd_report(ident, data).filter(|p| !has_control(p)) {
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
                            if ident == 11 || rs.len() > 1 {
                                let mode = ground_mode(&self.bg);
                                self.reports = note_theme_mode(self.reports, mode);
                            }
                            out.extend(rs.into_iter().map(|r| TermEvent::Reply(r.into_bytes())));
                        }
                    }
                    777 | 7501 => out.push(TermEvent::Osc {
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
                Event::PtyWrite(s) => {
                    let r = normalise_reply(s);
                    // alacritty does not know mode 2031 and answers "reset"; the answer is
                    // Flare's, whenever alacritty gets round to it (a synchronized update holds
                    // its reply back until the block ends).
                    if r != b"\x1b[?2031;0$y" {
                        out.push(TermEvent::Reply(r));
                    }
                }
                Event::Title(t) => out.push(TermEvent::Title(t)),
                Event::ResetTitle => out.push(TermEvent::ResetTitle),
                Event::Bell => out.push(TermEvent::Bell),
                _ => {}
            }
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        // alacritty drops the selection when the column count changes; xterm keeps it (decision
        // O23), so it is read before and set again after with its columns clamped to the width.
        let kept = if cols != self.cols() {
            self.sel_bounds()
        } else {
            None
        };
        crate::resize::resize_conpty(&mut self.term, cols, rows);
        if let Some((s, e, block)) = kept
            && self.term.selection.is_none()
        {
            let c = self.cols();
            self.sel_apply(SelOut::Set(SelSpec {
                block,
                start: Anchor {
                    row: s.0,
                    b: s.1.min(c),
                },
                end: Anchor {
                    row: e.0,
                    b: e.1.min(c),
                },
            }));
        }
    }

    /// Screen lines `clear_viewport` (ED 2) pushes into the history: down to the last line that
    /// holds anything, counted the way alacritty counts it (none on the alternate screen).
    fn viewport_used_lines(&self) -> usize {
        if self.term.mode().contains(TermMode::ALT_SCREEN) {
            return 0;
        }
        let grid = self.term.grid();
        for line in (0..self.rows()).rev() {
            for col in 0..self.cols() {
                let at = Point::new(Line(line as i32), Column(col));
                if !alacritty_terminal::grid::GridCell::is_empty(&grid[at]) {
                    return line + 1;
                }
            }
        }
        0
    }

    /// Puts back the selection an erase took away (decision O23). `moved` is the number of lines an
    /// ED 2 scrolled into the history: the selection rides up with the text it was on.
    fn restore_selection(&mut self, saved: Option<Selection>, moved: usize) {
        let Some(sel) = saved else { return };
        let lines = Line(self.rows() as i32);
        self.term.selection = if moved == 0 {
            Some(sel)
        } else {
            sel.rotate(&self.term, &(Line(0)..lines), moved as i32)
        };
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

    /// Bytes the scrollback holds: `history_size * cols * size_of::<Cell>()` (M10 Task 66).
    pub fn scrollback_bytes(&self) -> usize {
        self.history_size() * self.cols() * size_of::<alacritty_terminal::term::cell::Cell>()
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

    /// [`Emulator::snapshot`] into a grid the caller keeps and refills each frame, so the copy under
    /// the lock allocates nothing.
    pub fn snapshot_into(&self, out: &mut Grid) {
        snapshot_grid_into(&self.term, out);
    }

    /// The cells of one screen line as text, trailing blanks trimmed, wide spacers skipped.
    pub fn row_text(&self, line: i32) -> String {
        self.row_text_with(line, false)
    }

    /// `row_text`, with `ghosts` true blanking the cells that are not typed ink: dim (SGR 2), reverse
    /// video (a cursor block drawn over a character) and bright-black grey, which is how an agent paints
    /// a placeholder or a suggested prompt in an empty box.
    fn row_text_with(&self, line: i32, ghosts: bool) -> String {
        use alacritty_terminal::vte::ansi::{Color, NamedColor};
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
            if ghosts
                && (cell.flags.intersects(Flags::DIM | Flags::INVERSE)
                    || matches!(
                        cell.fg,
                        Color::Named(NamedColor::BrightBlack | NamedColor::DimBlack)
                            | Color::Indexed(8)
                    ))
            {
                s.push(' ');
                continue;
            }
            s.push(if cell.c == '\0' { ' ' } else { cell.c });
            if let Some(z) = cell.zerowidth() {
                s.extend(z.iter());
            }
        }
        s.trim_end().to_string()
    }

    /// `text_tail` with placeholder ink blanked (see `row_text_with`): what the prompt-draft check reads.
    pub fn typed_tail(&self, rows: usize) -> Vec<String> {
        let top = -(self.history_size() as i32);
        let bottom = self.rows() as i32 - 1;
        let Some(last) = (top..=bottom)
            .rev()
            .find(|&l| !self.row_text(l).trim().is_empty())
        else {
            return vec![self.row_text(top)];
        };
        let start = (last - rows.max(1) as i32 + 1).max(top);
        // A rule row of the prompt box keeps its plain text: an ANSI-only theme draws it in grey or dim,
        // and blanking it would hide the box the draft check looks for.
        (start..=last)
            .map(|l| {
                let plain = self.row_text(l);
                let t = plain.trim();
                if t.chars().count() >= 20 && t.chars().all(|c| c == '\u{2500}') {
                    plain
                } else {
                    self.row_text_with(l, true)
                }
            })
            .collect()
    }

    /// PT `setScreenTail`: scan from the bottom screen line up through the history for the last
    /// line whose text is not blank; return the `rows` lines ending at it (trailing blanks
    /// trimmed, blanks above it kept), clipped at the top of the buffer. Every line blank: the
    /// single top line.
    pub fn text_tail(&self, rows: usize) -> Vec<String> {
        let top = -(self.history_size() as i32);
        let bottom = self.rows() as i32 - 1;
        let mut last = None;
        for line in (top..=bottom).rev() {
            let t = self.row_text(line);
            if !t.trim().is_empty() {
                last = Some(line);
                break;
            }
        }
        let Some(last) = last else {
            return vec![self.row_text(top)];
        };
        let start = (last - rows.max(1) as i32 + 1).max(top);
        (start..=last).map(|l| self.row_text(l)).collect()
    }

    /// The question watch's rows: the live screen and a screen of scrollback above it, whatever
    /// the viewport shows.
    pub fn watch_rows(&self) -> Vec<String> {
        let rows = self.rows();
        let top = -(self.history_size().min(rows) as i32);
        (top..rows as i32).map(|l| self.row_text(l)).collect()
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
        if !matches!(out, SelOut::Nothing) {
            self.sel_changed();
        }
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

    /// The generation of the selection: it changes when the selection is made, moved by the user,
    /// cleared or dropped by the terminal. A redraw that keeps it does not change it.
    pub fn sel_generation(&self) -> u64 {
        self.sel_gen
    }

    fn sel_changed(&mut self) {
        self.sel_gen = self.sel_gen.wrapping_add(1);
    }

    /// The rows the selection covers, from its first row now.
    pub fn sel_row_count(&self) -> usize {
        self.sel_bounds()
            .map_or(0, |(s, e, _)| e.0.saturating_sub(s.0) + 1)
    }

    /// The text of the selection rows `rel` (counted from its first row now), cut so that the pieces
    /// for consecutive ranges concatenate to `sel_text()`. A piece that starts below the first row
    /// begins with the line break that sel_text() puts there. The flag: the piece's last row soft-wraps
    /// into the row after it.
    pub fn sel_text_rows(&self, rel: std::ops::Range<usize>) -> (String, bool) {
        let Some((s, e, block)) = self.sel_bounds() else {
            return (String::new(), false);
        };
        if block && s.1.min(e.1) == s.1.max(e.1) {
            return (String::new(), false);
        }
        let total = e.0.saturating_sub(s.0) + 1;
        let lo = rel.start.min(total);
        let hi = rel.end.min(total);
        if lo >= hi {
            return (String::new(), false);
        }
        let (a, c) = (s.0 + lo, s.0 + hi - 1);
        let cols = self.cols();
        let buf = TermBuf::all(&self.term);
        let wrapped = |y: usize| buf.line(y).is_some_and(LineLike::is_wrapped);
        let mut out = String::new();
        if lo > 0 && (block || !wrapped(a)) {
            out.push_str("\r\n");
        }
        let text = if block {
            selection_text(&buf, cols, (a, s.1), (c, e.1), true)
        } else {
            let start = (a, if lo == 0 { s.1 } else { 0 });
            let end = (c, if hi == total { e.1 } else { cols });
            selection_text(&buf, cols, start, end, false)
        };
        out.push_str(&text);
        (out, wrapped(c + 1))
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
        self.sel_changed();
        self.term.selection = None;
    }

    /// Selects the whole buffer, history and screen (macOS Cmd+A); nothing on an empty grid.
    pub fn sel_all(&mut self) {
        let total = self.history_size() + self.rows();
        if total == 0 || self.cols() == 0 {
            return;
        }
        self.sel_apply(SelOut::Set(SelSpec {
            block: false,
            start: Anchor { row: 0, b: 0 },
            end: Anchor {
                row: total - 1,
                b: self.cols(),
            },
        }));
    }

    /// Selects one match: a Simple selection over its cells. The end is the last cell's
    /// boundary plus one, so a match ending at a row's last cell stays on that row.
    fn select_match(&mut self, m: FindMatch) {
        let Some((start, end)) = self.match_bounds(m) else {
            return;
        };
        self.sel_apply(SelOut::Set(SelSpec {
            block: false,
            start: Anchor {
                row: start.0,
                b: start.1,
            },
            end: Anchor {
                row: end.0,
                b: end.1,
            },
        }));
    }

    /// The (row, boundary) start and end `select_match` selects for a match.
    fn match_bounds(&self, m: FindMatch) -> Option<((usize, usize), (usize, usize))> {
        let cols = self.cols();
        if cols == 0 {
            return None;
        }
        let last = m.row * cols + m.col + m.size.max(1) - 1;
        Some(((m.row, m.col), (last / cols, last % cols + 1)))
    }

    /// The find bar closed: the highlights go, and so does the selection when it is the one the active
    /// match put there (a Ctrl+C with it would copy the match instead of interrupting the program, and
    /// Backspace could delete it). A selection the owner made themselves stays.
    pub fn find_close(&mut self) {
        let found = self
            .find_active
            .and_then(|m| self.match_bounds(m))
            .zip(self.sel_bounds());
        if let Some(((start, end), (s, e, block))) = found
            && !block
            && s == start
            && e == end
        {
            self.sel_clear();
        }
        self.find_clear();
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
    /// match, so the incremental search that follows starts at it. No scroll, no selection. A
    /// search still walking for the old query is cancelled.
    pub fn find_set(&mut self, query: &str) {
        if query.is_empty() {
            self.find_clear();
            return;
        }
        self.find_query = query.to_string();
        self.find_hits.clear();
        self.find_recount = false;
        self.search = None;
        self.find_restart();
    }

    fn find_restart(&mut self) {
        let total = self.total_rows();
        self.find_job =
            Finder::new(&self.find_query, FindOpts::default()).map(|f| CountJob::new(f, total));
    }

    /// Starts a search from the anchor, cancelling one that runs. `incremental` (typing) keeps the
    /// active match when it still matches; Enter and the arrows (`false`) step past it. The walk
    /// happens in `find_step`, a slice at a time; the match lands then.
    pub fn find_start(&mut self, query: &str, dir: Dir, incremental: bool) {
        self.search = None;
        let Some(f) = Finder::new(query, FindOpts::default()) else {
            return;
        };
        let prev = self.find_anchor();
        let total = self.total_rows();
        self.search = Some(StepJob::new(f, dir, prev, !incremental, total));
    }

    /// Enter (`incremental: false`) and typing (`incremental: true`): the next match after the
    /// anchor, answered at once. Returns the match and the scroll the caller applies as
    /// `scroll_view(-scroll)`.
    pub fn find_next(
        &mut self,
        query: &str,
        incremental: bool,
    ) -> Option<(FindMatch, Option<i32>)> {
        self.find_now(query, Dir::Next, !incremental)
    }

    /// Shift+Enter and the up button: the match before the anchor, answered at once.
    pub fn find_prev(&mut self, query: &str) -> Option<(FindMatch, Option<i32>)> {
        self.find_now(query, Dir::Prev, true)
    }

    /// A search run to its end in this call (no budget).
    fn find_now(
        &mut self,
        query: &str,
        dir: Dir,
        same_term: bool,
    ) -> Option<(FindMatch, Option<i32>)> {
        let f = Finder::new(query, FindOpts::default())?;
        let prev = self.find_anchor();
        let cols = self.cols();
        let total = self.total_rows();
        let found = {
            let b = TermBuf::all(&self.term);
            let mut job = StepJob::new(f, dir, prev, same_term, total);
            job.run(&b, cols)
        };
        self.find_land(found)
    }

    /// Whether the count or a search still has slices to run.
    pub fn find_busy(&self) -> bool {
        self.search.is_some() || self.find_job.as_ref().is_some_and(|j| !j.done())
    }

    /// Runs ONE slice: the running search's, else the count job's, so one UI turn holds one slice.
    /// Returns the scroll that a search landing asks for (`scroll_view(-scroll)`), None when
    /// nothing landed or none is needed.
    pub fn find_step(&mut self, now: &dyn Fn() -> f64) -> Option<i32> {
        let cols = self.cols();
        if self.search.is_some() {
            let slice = {
                let b = TermBuf::all(&self.term);
                self.search
                    .as_mut()
                    .and_then(|search| search.step(&b, cols, now))
            };
            let found = slice?;
            self.search = None;
            return self.find_land(found).and_then(|(_, scroll)| scroll);
        }
        if let Some(job) = self.find_job.as_mut() {
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
        }
        None
    }

    /// A fresh count after output or a resize, WITHOUT scrolling or moving the active match. A
    /// search that runs starts again from its anchor, as the count does.
    pub fn find_refresh(&mut self) {
        if !self.find_query.is_empty() {
            self.find_recount = true;
            self.find_restart();
            if let Some(search) = self.search.as_mut() {
                search.restart();
            }
        }
    }

    /// Closing the bar: the highlights and the active match go; the selection stays (D17).
    pub fn find_clear(&mut self) {
        self.find_query.clear();
        self.find_job = None;
        self.search = None;
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
            searching: self.search.is_some(),
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
        // Extended over rows broken by hand too: the scan reads such a group whole.
        let buf = TermBuf::window_in(term, top, last, true);
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

    /// When a line whose path named nothing is due a second look (a frame then scans it again), if any is.
    pub fn links_next_retry(&self) -> Option<u64> {
        self.links.next_retry()
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
        // Across rows a program broke by hand too (`cells::by_hand`), as the scan reads them.
        let buf = TermBuf::window_in(&self.term, abs, abs, true);
        let ll = logical_line_in(&buf, cols, abs, true);
        let c = char_at_cell(&ll, cols, abs as i32, col as i32);
        let candidates = path_candidates_in(&ll.text, None, &ll.joins);
        if let Some((cand, f)) = paths
            .paths_found(owner, cwd, candidates, now_ms)
            .into_iter()
            .find(|(p, _)| p.start <= c && c < p.end)
        {
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

    /// A busy 480 x 135 screen: coloured, bold and wide cells on every row.
    fn busy_emulator() -> Emulator {
        let mut e = Emulator::new(480, 135);
        let mut s = String::new();
        for i in 0..400 {
            s.push_str(&format!(
                "\x1b[3{}m\x1b[1m{i:05} the quick brown fox \x1b[0m\x1b[44m jumps \x1b[0m over \u{4e2d}\u{6587} {}\r\n",
                i % 8,
                "0123456789".repeat(40)
            ));
        }
        e.feed(s.as_bytes());
        e
    }

    #[test]
    fn snapshot_into_equals_snapshot_and_survives_a_resize() {
        let mut e = busy_emulator();
        let mut buf = Grid::new(3, 3);
        e.snapshot_into(&mut buf);
        assert_eq!(buf, e.snapshot());
        // Refilled from a buffer that held something else, and after the screen changed size.
        e.feed(b"\x1b[2J\x1b[Hnew");
        e.snapshot_into(&mut buf);
        assert_eq!(buf, e.snapshot());
        e.resize(100, 30);
        e.snapshot_into(&mut buf);
        assert_eq!((buf.cols(), buf.rows()), (100, 30));
        assert_eq!(buf, e.snapshot());
    }

    #[test]
    fn snapshot_holds_the_lock_under_1_ms() {
        // The UI copies the visible rows under the emulator's lock while the parser waits for it, so the
        // hold must be short: 480 x 135 is a maximised window on a 4K screen at 100%. The copy refills
        // a reused buffer, so it allocates nothing. The 90th percentile of 300 holds, not the worst one,
        // because a test must not fail on a scheduler hiccup.
        let emu = alacritty_terminal::sync::FairMutex::new(busy_emulator());
        let mut buf = Grid::new(1, 1);
        emu.lock().snapshot_into(&mut buf);
        let mut holds = Vec::new();
        for _ in 0..300 {
            let t = std::time::Instant::now();
            emu.lock().snapshot_into(&mut buf);
            holds.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        holds.sort_by(|a, b| a.total_cmp(b));
        let p90 = holds[holds.len() * 9 / 10];
        let mut fresh = Vec::new();
        for _ in 0..300 {
            let t = std::time::Instant::now();
            drop(emu.lock().snapshot());
            fresh.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        fresh.sort_by(|a, b| a.total_cmp(b));
        println!(
            "480 x 135 hold, p90: reused buffer {p90:.3} ms, fresh grid {:.3} ms",
            fresh[fresh.len() * 9 / 10]
        );
        let limit = if cfg!(debug_assertions) { 40.0 } else { 1.0 };
        assert!(
            p90 < limit,
            "the 90th percentile hold was {p90:.3} ms (limit {limit} ms)"
        );
    }

    #[test]
    fn scrollback_matches_the_reference_terminals_line_count() {
        // Prism Terminal's xterm is built with `scrollback: 10000` (core/renderer/components/
        // TerminalPanel.tsx, v0.55.6). Parity of lines stays; the memory diet leaves it alone because
        // alacritty's cell cannot be made smaller (size_of::<Cell>() is what it is).
        assert_eq!(Emulator::SCROLLBACK, 10_000);
    }
    #[test]
    fn scrollback_bytes_follow_the_history_and_the_width() {
        let mut e = Emulator::new(80, 24);
        assert_eq!(e.scrollback_bytes(), 0);
        for i in 0..100 {
            e.feed(format!("line {i}\r\n").as_bytes());
        }
        let cell = size_of::<alacritty_terminal::term::cell::Cell>();
        assert_eq!(e.scrollback_bytes(), e.history_size() * 80 * cell);
        assert!(e.history_size() >= 70);
    }
    #[test]
    fn select_all_takes_the_history_and_the_screen() {
        let mut e = Emulator::new(20, 4);
        for i in 0..10 {
            e.feed(format!("row {i}\r\n").as_bytes());
        }
        e.sel_all();
        let text = e.sel_text();
        assert!(text.starts_with("row 0"), "{text:?}");
        assert!(text.contains("row 9"), "{text:?}");
        assert!(e.has_selection());
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

    #[cfg(not(windows))]
    #[test]
    fn osc_7_is_the_folder_report_on_macos() {
        let mut e = Emulator::new(80, 24);
        assert_eq!(
            e.feed(b"\x1b]7;file:///Users/me/a%20b\x1b\\"),
            vec![TermEvent::Cwd("/Users/me/a b".into())]
        );
        assert_eq!(
            e.feed(b"\x1b]7;file://localhost/tmp\x07"),
            vec![TermEvent::Cwd("/tmp".into())]
        );
        // Another machine's folder (a shell in ssh) and an encoded CR are never a folder here.
        for bad in [
            &b"\x1b]7;file://far-away/home/me\x07"[..],
            b"\x1b]7;file:///tmp/x%0Dcalc\x07",
        ] {
            let evs = e.feed(bad);
            assert!(
                !evs.iter().any(|ev| matches!(ev, TermEvent::Cwd(_))),
                "{evs:?}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn osc_7_is_no_folder_report_on_windows() {
        let mut e = Emulator::new(80, 24);
        assert!(e.feed(b"\x1b]7;file:///C:/Users/me\x07").is_empty());
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
    fn osc_7501_is_passed_up_raw_with_either_terminator_and_across_chunks() {
        let mut e = Emulator::new(80, 24);
        let report = |data: &str| TermEvent::Osc {
            ident: 7501,
            data: data.into(),
        };
        assert_eq!(
            e.feed(b"\x1b]7501;state=working:app=agy\x1b\\"),
            vec![report("state=working:app=agy")]
        );
        assert_eq!(
            e.feed(b"\x1b]7501;state=idle\x07"),
            vec![report("state=idle")]
        );
        assert!(e.feed(b"ab\x1b]7501;state=bl").is_empty());
        assert_eq!(
            e.feed(b"ocked:kind=question\x1b\\cd"),
            vec![report("state=blocked:kind=question")]
        );
        // The feature query is passed up too (the agent code reads it as no report); nothing is answered.
        let evs = e.feed(b"\x1b]7501;?\x1b\\");
        assert!(replies(&evs).is_empty());
    }

    #[test]
    fn a_7501_inside_another_osc_is_that_osc_text() {
        let mut e = Emulator::new(80, 24);
        let evs = e.feed(b"\x1b]0;x ]7501;state=working y\x07");
        assert_eq!(
            evs,
            vec![TermEvent::Title("x ]7501;state=working y".into())]
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
        let evs = e.feed(b"\x1b[?1004h\x1b[?9001h");
        assert!(replies(&evs).is_empty());
    }

    const DARK_DSR: &[u8] = b"\x1b[?997;1n";
    const LIGHT_DSR: &[u8] = b"\x1b[?997;2n";

    #[test]
    fn csi_996n_answers_the_ground_mode() {
        let mut e = Emulator::new(80, 24);
        assert_eq!(replies(&e.feed(b"\x1b[?996n")), vec![DARK_DSR.to_vec()]);
        e.set_colours("#3d2a1f", "#e6d8c0");
        assert_eq!(replies(&e.feed(b"\x1b[?996n")), vec![LIGHT_DSR.to_vec()]);
        assert_eq!(
            e.theme_reports().last,
            Some(flare_theme::marks::Mode::Light)
        );
        assert!(!e.theme_reports().on);
    }

    #[test]
    fn decrqm_2031_answers_set_after_2031h_and_reset_after_2031l() {
        let mut e = Emulator::new(80, 24);
        assert_eq!(
            replies(&e.feed(b"\x1b[?2031$p")),
            vec![b"\x1b[?2031;2$y".to_vec()]
        );
        e.feed(b"\x1b[?2031h");
        assert!(e.theme_reports().on);
        assert_eq!(
            replies(&e.feed(b"\x1b[?2031$p")),
            vec![b"\x1b[?2031;1$y".to_vec()]
        );
        e.feed(b"\x1b[?2031l");
        assert_eq!(
            replies(&e.feed(b"\x1b[?2031$p")),
            vec![b"\x1b[?2031;2$y".to_vec()]
        );
    }

    #[test]
    fn another_decrqm_is_left_to_alacritty() {
        let mut e = Emulator::new(80, 24);
        let r = replies(&e.feed(b"\x1b[?2004$p"));
        assert_eq!(r, vec![b"\x1b[?2004;2$y".to_vec()]);
    }

    #[test]
    fn replies_keep_stream_order_with_da1_last() {
        let mut e = Emulator::new(80, 24);
        let got = replies(&e.feed(
            b"\x1b]11;?\x07\x1b]10;?\x07\x1b[?996n\x1b[?2031h\x1b[?2031$p\x1b[>q\x1b[>q\x1b[c",
        ));
        assert_eq!(got.len(), 7, "{got:?}");
        assert!(got[0].starts_with(b"\x1b]11;"));
        assert!(got[1].starts_with(b"\x1b]10;"));
        assert_eq!(got[2], DARK_DSR);
        assert_eq!(got[3], b"\x1b[?2031;1$y");
        assert_eq!(got[4], xt());
        assert_eq!(got[5], xt());
        assert_eq!(got[6], b"\x1b[?1;2c");
    }

    #[test]
    fn ris_turns_theme_reports_off() {
        let mut e = Emulator::new(80, 24);
        e.feed(b"\x1b[?2031h");
        assert!(e.theme_reports().on);
        e.feed(b"\x1bc");
        assert_eq!(e.theme_reports(), THEME_REPORTS_OFF);
    }

    #[test]
    fn osc11_notes_the_mode() {
        let mut e = Emulator::new(80, 24);
        e.set_colours("#3d2a1f", "#e6d8c0");
        e.feed(b"\x1b]11;?\x07");
        assert_eq!(
            e.theme_reports().last,
            Some(flare_theme::marks::Mode::Light)
        );
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

    fn lines(e: &mut Emulator, n: usize) {
        let all: Vec<String> = (1..=n).map(|i| format!("line{i}")).collect();
        e.feed(all.join("\r\n").as_bytes());
    }

    #[test]
    fn an_empty_title_is_reported_as_an_empty_title() {
        // Recorded behaviour of alacritty-terminal: an empty OSC 0/2 reaches us as the event
        // checked below; either way the app sees Title("").
        let mut e = Emulator::new(80, 24);
        e.feed(b"\x1b]0;x\x07");
        assert_eq!(
            e.feed(b"\x1b]0;\x07"),
            vec![TermEvent::Title(String::new())]
        );
        assert_eq!(
            e.feed(b"\x1b]2;\x07"),
            vec![TermEvent::Title(String::new())]
        );
    }

    #[test]
    fn a_real_reset_title_is_still_a_reset() {
        let mut e = Emulator::new(80, 24);
        e.feed(b"\x1b[22t");
        e.feed(b"\x1b]0;other\x07");
        assert_eq!(e.feed(b"\x1b[23t"), vec![TermEvent::ResetTitle]);
    }

    #[test]
    fn the_tail_ends_at_the_last_text_not_the_bottom_of_the_screen() {
        let mut e = Emulator::new(20, 10);
        e.feed(b"a\r\nb\r\nc");
        assert_eq!(e.text_tail(16), vec!["a", "b", "c"]);
        let mut e = Emulator::new(20, 10);
        lines(&mut e, 30);
        assert_eq!(e.history_size(), 20);
        assert_eq!(e.text_tail(4), vec!["line27", "line28", "line29", "line30"]);
        let t = e.text_tail(16);
        assert_eq!((t[0].as_str(), t[15].as_str()), ("line15", "line30"));
        assert_eq!(t.len(), 16);
    }

    /// Claude Code's empty prompt box: a dim placeholder after the mark (with its first letter drawn as the
    /// cursor, reverse video) is not a draft; typed text is (NEW-E).
    #[test]
    fn a_dim_placeholder_in_an_empty_prompt_box_is_not_a_draft() {
        let rule = "\u{2500}".repeat(30);
        let screen = |input: &str| {
            let mut e = Emulator::new(60, 8);
            e.feed(format!("done\r\n{rule}\r\n{input}\r\n{rule}\r\n  auto mode on").as_bytes());
            e
        };
        let draft = |e: &Emulator| flare_agent::question::prompt_has_draft(&e.typed_tail(16));
        let plain = |e: &Emulator| flare_agent::question::prompt_has_draft(&e.text_tail(16));
        // dim placeholder, first letter reversed as the cursor
        let e = screen("\u{276f} \x1b[7mT\x1b[0m\x1b[2mry \"fix lint errors\"\x1b[0m");
        assert!(plain(&e), "the plain-text read mistakes it for a draft");
        assert!(!draft(&e));
        // grey (bright black) suggestion
        let e = screen("\u{276f} \x1b[90mrun the tests\x1b[0m");
        assert!(!draft(&e));
        // a real draft, cursor block after it
        let e = screen("\u{276f} fix the thing\x1b[7m \x1b[0m");
        assert!(draft(&e));
        // typed text before a dim tail still counts
        let e = screen("\u{276f} half\x1b[2m a suggestion\x1b[0m");
        assert!(draft(&e));
    }

    /// An ANSI-only theme draws the box rules in bright black or dim: the box must still be found (R3).
    #[test]
    fn a_grey_or_dim_rule_still_shows_a_draft_in_the_box() {
        let rule = "\u{2500}".repeat(30);
        for paint in ["\x1b[90m", "\x1b[2m", "\x1b[38;5;8m"] {
            let mut e = Emulator::new(60, 8);
            e.feed(
                format!(
                    "done\r\n{paint}{rule}\x1b[0m\r\n\u{276f} fix it\r\n{paint}{rule}\x1b[0m\r\n  auto"
                )
                .as_bytes(),
            );
            assert!(
                flare_agent::question::prompt_has_draft(&e.typed_tail(16)),
                "{paint:?}"
            );
            let mut e = Emulator::new(60, 8);
            e.feed(
                format!(
                    "done\r\n{paint}{rule}\x1b[0m\r\n\u{276f} \x1b[2mTry x\x1b[0m\r\n{paint}{rule}\x1b[0m\r\n  auto"
                )
                .as_bytes(),
            );
            assert!(
                !flare_agent::question::prompt_has_draft(&e.typed_tail(16)),
                "{paint:?} empty"
            );
        }
    }

    #[test]
    fn an_all_blank_buffer_gives_the_single_top_line() {
        let e = Emulator::new(20, 10);
        assert_eq!(e.text_tail(16), vec![String::new()]);
    }

    #[test]
    fn blanks_above_the_last_line_are_kept() {
        let mut e = Emulator::new(20, 10);
        e.feed(b"first\x1b[6;1Hlast");
        assert_eq!(e.text_tail(16), vec!["first", "", "", "", "", "last"]);
    }

    #[test]
    fn wide_characters_and_trailing_blanks_are_trimmed() {
        let mut e = Emulator::new(20, 10);
        e.feed("日本 ".as_bytes());
        assert_eq!(e.text_tail(4), vec!["日本"]);
    }

    #[test]
    fn the_watch_rows_are_the_live_screen_and_one_screen_of_history() {
        let mut e = Emulator::new(20, 10);
        lines(&mut e, 25);
        let w = e.watch_rows();
        assert_eq!(w.len(), 20);
        assert_eq!(w.last().map(String::as_str), Some("line25"));
        e.scroll_view(5);
        assert_eq!(e.watch_rows(), w);
    }

    #[test]
    fn a_widget_taller_than_the_pane_is_found() {
        // The `run2-02-right` widget rows of question_widget_screens.json.
        let widget = [
            "←  ☐ Colour  ☐ Size  ☐ Shape  ✔ Submit  →",
            "",
            "Pick a size?",
            "",
            "❯ 1. Small",
            "     Small",
            "  2. Large",
            "     Large",
            "  3. Type something.",
            "  4. Chat about this",
            "",
            "Enter to select · Tab/Arrow keys to navigate · Esc to cancel",
        ];
        let mut e = Emulator::new(80, 8);
        e.feed(widget.join("\r\n").as_bytes());
        let found = flare_agent::question_widget::extract_question(&e.watch_rows());
        assert_eq!(
            found,
            Some(flare_agent::question_widget::ShownQuestion::Text(
                "Pick a size?".into()
            ))
        );
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

    #[test]
    fn decrqm_2031_inside_a_synchronized_update_gets_one_answer() {
        let mut e = Emulator::new(80, 24);
        let first = replies(&e.feed(b"\x1b[?2026h\x1b[?2031$p"));
        assert_eq!(first, vec![b"\x1b[?2031;2$y".to_vec()]);
        let d = e.sync_deadline().expect("deadline");
        assert!(replies(&e.release_sync_if_due(d)).is_empty());
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
    fn an_erase_over_the_selection_keeps_it_and_copies_the_new_text() {
        let mut e = Emulator::new(20, 3);
        e.feed(b"hello world");
        e.sel_apply(sel(false, 0, 0, 0, 5));
        e.feed(b"\x1b[2K");
        assert!(e.has_selection());
        assert_eq!(e.sel_text(), "");
        e.feed(b"\rHELLO");
        assert_eq!(e.sel_text(), "HELLO");
    }

    #[test]
    fn el_inside_a_selection_keeps_it() {
        let mut e = Emulator::new(20, 3);
        e.feed(b"hello world");
        e.sel_apply(sel(false, 0, 0, 0, 11));
        e.feed(b"\x1b[1;6H\x1b[K");
        assert!(e.has_selection());
        assert_eq!(e.sel_text(), "hello");
    }

    /// Claude-style: cursor home and rewrite the whole screen, many times.
    #[test]
    fn a_selection_survives_a_cursor_home_redraw_of_the_whole_screen() {
        let mut e = Emulator::new(20, 24);
        for i in 0..20 {
            e.feed(format!("row {i:02}\r\n").as_bytes());
        }
        e.sel_apply(sel(false, 3, 0, 5, 6));
        for pass in 0..3 {
            e.feed(b"\x1b[H");
            for i in 0..20 {
                e.feed(format!("\x1b[2Kline {pass}{i:02}\r\n").as_bytes());
            }
            e.feed(b"\x1b[J");
            assert!(e.has_selection(), "pass {pass}");
        }
        assert_eq!(e.sel_text(), "line 203\r\nline 204\r\nline 2");
    }

    #[test]
    fn a_selection_follows_the_text_when_history_trims() {
        let mut e = Emulator::new(10, 3);
        for i in 0..6 {
            e.feed(format!("line{i}\r\n").as_bytes());
        }
        e.sel_apply(sel(false, 2, 0, 2, 5));
        assert_eq!(e.sel_text(), "line2");
        for i in 6..30 {
            e.feed(format!("line{i}\r\n").as_bytes());
        }
        assert_eq!(e.sel_text(), "line2");
    }

    #[test]
    fn ed2_keeps_the_selection_on_the_text_it_moved_into_history() {
        let mut e = Emulator::new(10, 4);
        e.feed(b"aaa\r\nbbb\r\nccc");
        e.sel_apply(sel(false, 1, 0, 1, 3));
        assert_eq!(e.sel_text(), "bbb");
        e.feed(b"\x1b[2J");
        assert!(e.has_selection());
        assert_eq!(e.sel_text(), "bbb");
        e.feed(b"\x1b[Hnew");
        assert_eq!(e.sel_text(), "bbb");
    }

    #[test]
    fn ed2_on_a_selection_over_the_screen_keeps_it_in_history() {
        let mut e = Emulator::new(10, 4);
        for i in 0..8 {
            e.feed(format!("l{i}\r\n").as_bytes());
        }
        e.sel_apply(sel(false, 5, 0, 6, 2));
        let before = e.sel_text();
        assert_eq!(before, "l5\r\nl6");
        e.feed(b"\x1b[2J");
        assert_eq!(e.sel_text(), before);
    }

    #[test]
    fn ed3_clears_a_selection_in_the_cleared_history() {
        let mut e = Emulator::new(10, 3);
        for i in 0..10 {
            e.feed(format!("line{i}\r\n").as_bytes());
        }
        e.sel_apply(sel(false, 1, 0, 1, 5));
        e.feed(b"\x1b[3J");
        assert!(!e.has_selection());
    }

    #[test]
    fn the_alt_screen_clears_the_selection() {
        let mut e = Emulator::new(10, 3);
        e.feed(b"hello");
        e.sel_apply(sel(false, 0, 0, 0, 5));
        e.feed(b"\x1b[?1049h");
        assert!(!e.has_selection());
    }

    #[test]
    fn ris_clears_the_selection() {
        let mut e = Emulator::new(10, 3);
        e.feed(b"hello");
        e.sel_apply(sel(false, 0, 0, 0, 5));
        e.feed(b"\x1bc");
        assert!(!e.has_selection());
    }

    #[test]
    fn a_column_resize_clamps_the_selection() {
        let mut e = Emulator::new(20, 3);
        e.feed(b"hello world foo");
        e.sel_apply(sel(false, 0, 6, 0, 15));
        e.resize(10, 3);
        assert!(e.has_selection());
        let (s, end, _) = e.sel_bounds().expect("selection");
        assert_eq!(s.1, 6);
        assert_eq!(end.1, 10);
        e.resize(20, 3);
        assert!(e.has_selection());
    }

    #[test]
    fn a_column_resize_drops_a_selection_that_became_empty() {
        let mut e = Emulator::new(20, 3);
        e.feed(b"hello world foo");
        e.sel_apply(sel(false, 0, 12, 0, 15));
        e.resize(10, 3);
        assert!(!e.has_selection());
    }

    #[test]
    fn a_row_resize_keeps_the_selection() {
        let mut e = Emulator::new(20, 6);
        e.feed(b"hello world");
        e.sel_apply(sel(false, 0, 0, 0, 5));
        e.resize(20, 4);
        assert_eq!(e.sel_text(), "hello");
        e.resize(20, 8);
        assert_eq!(e.sel_text(), "hello");
    }

    #[test]
    fn a_block_selection_survives_a_redraw() {
        let mut e = Emulator::new(20, 4);
        e.feed(b"abcdef\r\nghijkl");
        e.sel_apply(sel(true, 0, 2, 1, 4));
        e.feed(b"\x1b[H\x1b[2Kabcdef\r\n\x1b[2Kghijkl\x1b[J");
        assert!(e.has_selection());
        assert_eq!(e.sel_text(), "cd\r\nij");
    }

    #[test]
    fn find_still_selects_a_match() {
        let mut e = Emulator::new(20, 3);
        e.feed(b"hello world");
        e.find_next("world", false);
        assert!(e.has_selection());
        assert_eq!(e.sel_text(), "world");
    }

    #[test]
    fn an_insert_line_above_moves_the_selection_down() {
        let mut e = Emulator::new(10, 5);
        e.feed(b"one\r\ntwo\r\nthree");
        e.sel_apply(sel(false, 1, 0, 1, 3));
        assert_eq!(e.sel_text(), "two");
        e.feed(b"\x1b[1;1H\x1b[L");
        assert_eq!(e.sel_text(), "two");
        let (s, _, _) = e.sel_bounds().expect("selection");
        assert_eq!(s.0, 2);
    }

    #[test]
    fn a_guard_split_across_two_feeds_still_keeps_the_selection() {
        let mut e = Emulator::new(20, 3);
        e.feed(b"hello world");
        e.sel_apply(sel(false, 0, 0, 0, 5));
        e.feed(b"\x1b[2");
        e.feed(b"Kx");
        assert!(e.has_selection());
    }

    /// 200 selections over a static screen (history and screen, simple and block): the text
    /// `sel_text` returns is the text captured into the fixture before the redraw change.
    #[test]
    fn copy_text_is_unchanged_for_an_unchanged_screen() {
        let mut e = Emulator::new(24, 8);
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = move |m: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % m as u64) as usize
        };
        for i in 0..40 {
            let n = next(24);
            let body: String = (0..n)
                .map(|k| (b'a' + ((i + k) % 26) as u8) as char)
                .collect();
            e.feed(format!("{i:02} {body}\r\n").as_bytes());
        }
        let total = e.history_size() + e.rows();
        let mut got = String::new();
        for k in 0..200 {
            let (r0, r1) = (next(total), next(total));
            let (b0, b1) = (next(25), next(25));
            e.sel_apply(sel(k % 3 == 0, r0, b0, r1, b1));
            got.push_str(&format!("{:?}\n", e.sel_text()));
        }
        if std::env::var_os("FLARE_WRITE_SEL_FIXTURE").is_some() {
            std::fs::write(
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/sel_text_200.txt"
                ),
                &got,
            )
            .expect("write fixture");
        }
        let want = include_str!("../tests/fixtures/sel_text_200.txt");
        assert_eq!(got.replace("\r\n", "\n"), want.replace("\r\n", "\n"));
    }

    /// Chunked reads (random sizes, wrapped and block selections over history and screen) join to
    /// exactly `sel_text()`.
    #[test]
    fn chunked_text_equals_sel_text() {
        let mut e = Emulator::new(16, 6);
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move |m: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % m as u64) as usize
        };
        for i in 0..120 {
            let n = next(40);
            let body: String = (0..n)
                .map(|k| (b'a' + ((i + k) % 26) as u8) as char)
                .collect();
            e.feed(format!("{i:03} {body}\r\n").as_bytes());
        }
        let total = e.history_size() + e.rows();
        for case in 0..200 {
            let block = next(2) == 0;
            let (mut r0, mut r1) = (next(total), next(total));
            if r1 < r0 {
                std::mem::swap(&mut r0, &mut r1);
            }
            e.sel_apply(sel(block, r0, next(17), r1, next(17)));
            let want = e.sel_text();
            let chunk = 1 + next(300);
            let rows = e.sel_row_count();
            let mut got = String::new();
            let mut from = 0;
            while from < rows {
                let end = (from + chunk).min(rows);
                got.push_str(&e.sel_text_rows(from..end).0);
                from = end;
            }
            assert_eq!(got, want, "case {case} (block {block}, chunk {chunk})");
        }
    }

    #[test]
    fn the_selection_generation_moves_on_a_change_and_not_on_output() {
        let mut e = Emulator::new(20, 3);
        e.feed(b"hello world");
        let g0 = e.sel_generation();
        e.sel_apply(sel(false, 0, 0, 0, 5));
        let g1 = e.sel_generation();
        assert_ne!(g0, g1, "a new selection is a change");
        e.feed(b"\r\nmore output below");
        assert_eq!(
            e.sel_generation(),
            g1,
            "output that keeps the selection is not"
        );
        e.sel_clear();
        assert_ne!(e.sel_generation(), g1, "a clear is a change");
        assert_eq!(e.sel_row_count(), 0);
    }

    #[test]
    fn sel_row_count_follows_the_selection() {
        let mut e = Emulator::new(10, 4);
        e.feed(b"aaaa\r\nbbbb\r\ncccc");
        assert_eq!(e.sel_row_count(), 0);
        e.sel_apply(sel(false, 0, 2, 2, 3));
        assert_eq!(e.sel_row_count(), 3);
        assert_eq!(e.sel_text_rows(1..2).0, "\r\nbbbb");
        assert_eq!(e.sel_text_rows(5..9).0, "");
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
    fn a_new_query_cancels_the_running_job() {
        let mut e = Emulator::new(20, 3);
        for _ in 0..300 {
            e.feed(b"row of text\r\n");
        }
        e.feed(b"foo bar");
        let ticks = std::cell::Cell::new(0.0f64);
        // Every read of the clock is 5 ms later: one row per slice, so the walk cannot finish.
        let slow = || {
            ticks.set(ticks.get() + 5.0);
            ticks.get()
        };
        e.find_start("foo", Dir::Next, true);
        e.find_step(&slow);
        assert!(
            e.find_view().searching,
            "one slice does not finish the walk"
        );
        assert_eq!(e.find_view().active, None, "nothing has landed yet");
        e.find_set("bar");
        assert!(
            !e.find_view().searching,
            "the new query cancels the old walk"
        );
        e.find_start("bar", Dir::Next, true);
        assert_eq!(
            e.find_step(&|| 0.0),
            None,
            "a match in view does not scroll"
        );
        assert_eq!(e.sel_text(), "bar");
    }

    #[test]
    fn a_search_with_a_match_near_the_top_lands_in_one_step() {
        let mut e = Emulator::new(20, 3);
        e.feed(b"foo\r\n");
        for _ in 0..300 {
            e.feed(b"row of text\r\n");
        }
        e.find_start("foo", Dir::Next, true);
        let ticks = std::cell::Cell::new(0.0f64);
        let slow = || {
            ticks.set(ticks.get() + 5.0);
            ticks.get()
        };
        e.find_step(&slow);
        assert!(!e.find_view().searching);
        assert_eq!(e.find_view().active.map(|m| (m.row, m.col)), Some((0, 0)));
    }

    #[test]
    #[ignore = "release timing"]
    fn typing_a_letter_with_no_match_holds_the_ui_thread_under_10_ms() {
        let mut e = Emulator::new(40, 5);
        for i in 0..10_000 {
            e.feed(format!("line {i} of the scrollback\r\n").as_bytes());
        }
        let t0 = Instant::now();
        let clock = move || t0.elapsed().as_secs_f64() * 1000.0;
        let mut longest = 0.0f64;
        let started = Instant::now();
        e.find_set("zq");
        e.find_start("zq", Dir::Next, true);
        longest = longest.max(started.elapsed().as_secs_f64() * 1000.0);
        while e.find_busy() {
            let turn = Instant::now();
            e.find_step(&clock);
            longest = longest.max(turn.elapsed().as_secs_f64() * 1000.0);
        }
        eprintln!("longest UI-thread turn over 10,000 lines: {longest:.2} ms");
        assert!(longest < 10.0, "the longest turn took {longest:.2} ms");
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

    /// The reading with the gap between two readings waived (a test does not sleep).
    fn dev(e: &mut Emulator) -> DevState {
        e.dev_scanned = None;
        e.dev_state()
    }

    fn dev_url(e: &mut Emulator) -> Option<String> {
        dev(e).server.map(|s| s.url)
    }

    #[test]
    fn the_emulator_reads_a_local_server_address_off_its_screen() {
        let mut e = Emulator::new(60, 8);
        e.feed(b"plain output\r\n");
        assert_eq!(dev(&mut e).server, None);
        assert_eq!(dev(&mut e).seq, 0);
        // Split across two chunks, with colour inside the port.
        e.feed(b"  Local: http://localhost:\x1b[1m51");
        e.feed(b"73\x1b[22m/\r\n");
        let s = dev(&mut e);
        assert_eq!(
            s.server.map(|x| x.url),
            Some("http://localhost:5173/".into())
        );
        assert_eq!(s.seq, 1);
        // Asking again, and more output that names nothing local, changes nothing.
        for i in 0..5 {
            e.feed(format!("request {i} https://example.com:8443/x\r\n").as_bytes());
        }
        let again = dev(&mut e);
        assert_eq!(again.seq, 1);
        assert!(again.server.is_some());
    }

    #[test]
    fn the_row_the_cursor_is_on_is_input_and_never_read() {
        let mut e = Emulator::new(80, 8);
        e.feed(b"PS> echo http://localhost:6303/three");
        assert_eq!(dev_url(&mut e), None, "typed, not entered");
        e.feed(b"\r\n");
        assert_eq!(
            dev_url(&mut e),
            Some("http://localhost:6303/".into()),
            "entered"
        );
    }

    #[test]
    fn a_suggestion_painted_after_the_cursor_and_typed_over_is_not_read() {
        let mut e = Emulator::new(80, 8);
        // The shell paints a history suggestion after what is typed and moves the cursor back over it.
        e.feed(b"PS> echo http://\x1b[90mlocalhost:6303/three\x1b[0m\x1b[21D");
        assert_eq!(dev_url(&mut e), None);
        // The person types something else; the rest of the suggestion is erased; Enter.
        e.feed(b"example.com/\x1b[K\r\n");
        assert_eq!(dev_url(&mut e), None);
    }

    #[test]
    fn an_address_that_scrolled_off_the_screen_is_still_the_one_held() {
        let mut e = Emulator::new(60, 6);
        e.feed(b"Local: http://localhost:5173/\r\n");
        assert_eq!(dev(&mut e).seq, 1);
        for i in 0..40 {
            e.feed(format!("line {i}\r\n").as_bytes());
        }
        let s = dev(&mut e);
        assert_eq!(
            s.server.map(|x| x.port),
            Some(5173),
            "held after it left the screen"
        );
        assert_eq!(s.seq, 1);
        // The same address printed again is not news (a request log must not reset the chip).
        e.feed(b"GET http://localhost:5173/api 200\r\nLocal: http://localhost:5173/\r\n");
        assert_eq!(dev(&mut e).seq, 1);
        // Another address is.
        e.feed(b"Local: http://localhost:5174/\r\n");
        let s = dev(&mut e);
        assert_eq!(s.server.map(|x| x.port), Some(5174));
        assert_eq!(s.seq, 2);
    }

    #[test]
    fn a_cleared_screen_forgets_the_address_though_the_history_still_holds_it() {
        let mut e = Emulator::new(60, 6);
        e.feed(b"Local: http://localhost:5173/\r\n");
        assert!(dev(&mut e).server.is_some());
        e.feed(b"\x1b[2J\x1b[H");
        let s = dev(&mut e);
        assert_eq!(s.server, None);
        assert_eq!(s.seq, 2);
        // It does not come back by itself on later output, and a new one is found.
        e.feed(b"PS> \r\nnothing\r\n");
        assert_eq!(dev(&mut e).server, None);
        e.feed(b"Local: http://127.0.0.1:3000/\r\n");
        let s = dev(&mut e);
        assert_eq!(s.server.map(|x| x.port), Some(3000));
        assert_eq!(s.seq, 3);
    }

    #[test]
    fn the_same_address_printed_again_moves_reprint_not_seq() {
        let mut e = Emulator::new(60, 6);
        e.feed(b"Local: http://localhost:3000/\r\n");
        let s = dev(&mut e);
        assert_eq!((s.seq, s.reprint), (1, 0));
        // Nothing new on the screen: nothing moves.
        e.feed(b"compiled\r\n");
        let s = dev(&mut e);
        assert_eq!((s.seq, s.reprint), (1, 0));
        // The server stopped and started again, without a clear: the address is printed a second time.
        e.feed(b"^C\r\nLocal: http://localhost:3000/\r\n");
        let s = dev(&mut e);
        assert_eq!((s.seq, s.reprint), (1, 1));
        // And once the first has scrolled off, a third print is heard too.
        for i in 0..20 {
            e.feed(format!("line {i}\r\n").as_bytes());
        }
        let _ = dev(&mut e);
        e.feed(b"Local: http://localhost:3000/\r\n");
        let s = dev(&mut e);
        assert_eq!(s.seq, 1);
        assert!(s.reprint > 1);
    }

    #[test]
    fn a_full_scrollback_does_not_hide_a_newer_address() {
        let mut e = Emulator::new(60, 8);
        for i in 0..Emulator::SCROLLBACK + 50 {
            e.feed(format!("log {i}\r\n").as_bytes());
        }
        e.feed(b"Local: http://localhost:3000/\r\n");
        assert_eq!(dev(&mut e).server.map(|s| s.port), Some(3000));
        // A few lines scroll by, then a banner whose address sits above the bottom.
        for i in 0..3 {
            e.feed(format!("more {i}\r\n").as_bytes());
        }
        e.feed(b"Local: http://localhost:3001/\r\nready\r\nwatching\r\n");
        let s = dev(&mut e);
        assert_eq!(s.server.map(|s| s.port), Some(3001));
        assert_eq!(s.seq, 2);
    }

    #[test]
    fn a_full_screen_program_clearing_its_own_screen_keeps_the_address() {
        let mut e = Emulator::new(60, 6);
        e.feed(b"Local: http://localhost:3000/\r\n");
        assert_eq!(dev(&mut e).seq, 1);
        e.feed(b"\x1b[?1049h\x1b[2J\x1b[3Jhello\x1b[?1049l");
        let s = dev(&mut e);
        assert_eq!(s.server.map(|s| s.port), Some(3000));
        assert_eq!(s.seq, 1);
    }

    #[test]
    fn reading_a_huge_screen_full_of_addresses_stays_cheap() {
        // A maximised 4K window: about 400 x 120, every line a URL-like path (rule 16's slice budget).
        let mut e = Emulator::new(400, 120);
        for i in 0..300 {
            e.feed(
                format!("GET https://example.com:8443/some/path/{i}/and/more?x=1&y=2 200 12ms\r\n")
                    .as_bytes(),
            );
        }
        e.feed(b"Local: http://localhost:3000/\r\n");
        // The best of five full readings (`dev` drops the cache each time): a shared CI runner can stall any one of
        // them past the budget, while a reading that is really slow is slow every time.
        let mut took = std::time::Duration::MAX;
        for _ in 0..5 {
            let t = Instant::now();
            let s = dev(&mut e);
            took = took.min(t.elapsed());
            assert_eq!(s.server.map(|s| s.port), Some(3000));
        }
        assert!(
            took < std::time::Duration::from_millis(30),
            "the quickest of five readings took {took:?}"
        );
    }

    #[test]
    fn a_long_address_wrapped_over_rows_is_read_whole() {
        let mut e = Emulator::new(20, 6);
        e.feed(b"http://localhost:5173/some/long/path?with=query\r\n");
        assert_eq!(dev_url(&mut e), Some("http://localhost:5173/".into()));
    }

    #[test]
    fn nothing_is_read_on_the_alternate_screen_and_no_remote_address_ever() {
        let mut e = Emulator::new(60, 6);
        e.feed(b"\x1b[?1049hhttp://localhost:5173/\r\n\r\n");
        assert_eq!(
            dev_url(&mut e),
            None,
            "a full-screen program's screen is not output"
        );
        let mut e = Emulator::new(60, 6);
        e.feed(b"see http://example.com:3000/ and http://localhost.evil.com:3000/\r\n");
        e.feed(b"http://localhost:3000@evil.test/ https://evil.test/localhost:3000\r\n");
        assert_eq!(dev_url(&mut e), None);
    }

    #[test]
    fn a_reading_asked_for_too_soon_is_owed_not_lost() {
        let mut e = Emulator::new(60, 6);
        e.feed(b"hello\r\n");
        let _ = e.dev_state();
        assert!(!e.dev_pending());
        e.feed(b"Local: http://localhost:5173/\r\n");
        let soon = e.dev_state();
        assert_eq!(soon.server, None, "inside the gap the last answer stands");
        assert!(e.dev_pending(), "and a reading is still owed");
        e.dev_scanned = None;
        assert!(e.dev_state().server.is_some());
        assert!(!e.dev_pending());
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
    fn closing_find_drops_the_selection_the_match_made_so_ctrl_c_is_the_programs() {
        let mut e = Emulator::new(20, 3);
        e.feed(b"foo bar");
        e.find_set("foo");
        count_all(&mut e);
        e.find_next("foo", false);
        assert!(e.has_selection(), "the active match is selected");
        e.find_close();
        assert!(
            !e.has_selection(),
            "closing the bar must not leave the match selected"
        );
        assert_eq!(e.find_spans(), (Vec::new(), Vec::new()));
    }

    #[test]
    fn closing_find_keeps_a_selection_the_owner_made() {
        let mut e = Emulator::new(20, 3);
        e.feed(b"foo bar");
        e.find_set("foo");
        count_all(&mut e);
        e.find_next("foo", false);
        // The owner then selects "bar" themselves.
        e.sel_apply(SelOut::Set(SelSpec {
            block: false,
            start: Anchor { row: 0, b: 4 },
            end: Anchor { row: 0, b: 7 },
        }));
        e.find_close();
        assert!(e.has_selection(), "the owner's own selection stays");
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

    // Owner, 2026-10-21 (PT #250, core 0.44.2): Claude Code breaks its own lines at the window's
    // width and writes each row on its own, so the next row is never marked wrapped, and a long
    // path became two pieces that each named nothing.
    const CUT_EXE: &str = "C:\\t\\scratch\\a.exe";

    /// Scans, answers every question with only `CUT_EXE` existing, and scans again.
    fn cut_marks(e: &mut Emulator) -> (PathLinks, Vec<LinkMark>) {
        let mut pl = PathLinks::new();
        for round in 0..4u64 {
            e.links_revisit();
            e.link_marks(&mut pl, "t1", Some("C:\\w"), round * 50, 1);
            pl.advance(round * 50 + 45);
            while let Some(ask) = pl.take_ask() {
                let hits = ask
                    .paths
                    .iter()
                    .map(|p| {
                        (p == CUT_EXE).then(|| crate::path_open::PathHit {
                            kind: crate::path_open::PathKind::File,
                            abs: p.clone(),
                        })
                    })
                    .collect();
                pl.answer(&ask, Some(hits), round * 50 + 46);
            }
        }
        e.links_revisit();
        let m = e.link_marks(&mut pl, "t1", Some("C:\\w"), 300, 1);
        (pl, m)
    }

    fn cut_is_one_link(m: &[LinkMark]) {
        let paths: Vec<&LinkMark> = m
            .iter()
            .filter(|m| matches!(m.target, LinkTarget::Path { .. }))
            .collect();
        assert_eq!(paths.len(), 1, "{m:?}");
        assert_eq!(
            paths[0].target,
            LinkTarget::Path {
                text: CUT_EXE.into()
            }
        );
        assert_eq!(
            paths[0].pieces,
            vec![
                Piece {
                    row: 0,
                    col: 3,
                    width: 9
                },
                Piece {
                    row: 1,
                    col: 0,
                    width: 9
                }
            ]
        );
    }

    #[test]
    fn a_path_broken_by_hand_at_the_edge_is_one_link() {
        let mut e = Emulator::new(12, 4);
        // The first row exactly as wide as the window, then a new line: not wrapped.
        e.feed(b"at C:\\t\\scra\r\ntch\\a.exe");
        let (mut pl, m) = cut_marks(&mut e);
        cut_is_one_link(&m);
        // The right-click over its second row offers the whole file.
        let c = e.context_at(1, 2, &mut pl, "t1", Some("C:\\w"), 300);
        assert_eq!(c.path.map(|p| p.text).as_deref(), Some(CUT_EXE));
        let c = e.context_at(0, 5, &mut pl, "t1", Some("C:\\w"), 300);
        assert_eq!(c.path.map(|p| p.text).as_deref(), Some(CUT_EXE));
    }

    #[test]
    fn a_path_broken_by_hand_on_the_alternate_screen_inks_each_row() {
        let mut e = Emulator::new(12, 4);
        e.feed(b"\x1b[?1049h\x1b[Hat C:\\t\\scra\r\ntch\\a.exe");
        let (_, m) = cut_marks(&mut e);
        cut_is_one_link(&m);
    }

    #[test]
    fn a_web_link_is_carried_across_a_row_broken_by_hand_review_37() {
        let mut e = Emulator::new(17, 4);
        e.feed(b"go https://a.io/x\r\n/y/z");
        let m = marks(&mut e);
        let web: Vec<&LinkTarget> = m
            .iter()
            .map(|m| &m.target)
            .filter(|t| matches!(t, LinkTarget::Web(_)))
            .collect();
        assert_eq!(web, vec![&LinkTarget::Web("https://a.io/x/y/z".into())]);
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
