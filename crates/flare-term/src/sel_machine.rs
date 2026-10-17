//! The selection state machine (click, drag, shift-extend), ported from xterm's
//! `SelectionService` (`handleMouseDown`, `_handleMouseMove`, `_dragScroll`,
//! `_getMouseEventScrollAmount`). Pure: no alacritty, no clock.

/// A cell boundary: `b` in 0..=cols of absolute row `row` (D3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Anchor {
    pub row: usize,
    pub b: usize,
}

/// `[start, end)` of cells on one or more rows, as boundaries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: Anchor,
    pub end: Anchor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelSpec {
    pub block: bool,
    pub start: Anchor,
    pub end: Anchor,
}

impl SelSpec {
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

/// What the machine asks of the buffer.
pub trait SelEnv {
    fn cols(&self) -> usize;
    fn total_rows(&self) -> usize;
    /// 0 for a wide character's spacer, 2 for the wide cell, 1 otherwise.
    fn cell_width(&self, row: usize, col: usize) -> u8;
    fn word_at(&self, row: usize, col: usize, allow_whitespace_only: bool) -> Option<Span>;
    /// First and last row of the logical line holding `row`.
    fn line_at(&self, row: usize) -> (usize, usize);
    /// The link (web, OSC 8 or path) under the cell, as a span.
    fn link_at(&self, row: usize, col: usize) -> Option<Span>;
}

/// The boundary a pointer means: left half of a cell -> `col`, right half -> `col + 1`; over a
/// wide character the first cell means the boundary before it and the second the boundary after it.
pub fn boundary_of(env: &dyn SelEnv, row: usize, col: usize, right: bool) -> usize {
    let cols = env.cols();
    let b = match env.cell_width(row, col) {
        2 => col,
        0 => col + 1,
        _ => {
            if right {
                col + 1
            } else {
                col
            }
        }
    };
    b.min(cols)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelOut {
    Nothing,
    Clear,
    Set(SelSpec),
    Extend(Anchor),
}

#[derive(Clone, Copy, Debug)]
pub struct Press {
    pub row: usize,
    pub col: usize,
    pub right: bool,
    pub clicks: u32,
    pub shift: bool,
    pub alt: bool,
}

/// `row` may be < 0 or >= total; `past_css_px` is the distance past the viewport edge in CSS px
/// (0 inside, positive below, negative above).
#[derive(Clone, Copy, Debug)]
pub struct Drag {
    pub row: i64,
    pub col: usize,
    pub right: bool,
    pub past_css_px: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct ViewBox {
    pub top: usize,
    pub rows: usize,
}

pub const DRAG_SCROLL_MS: u64 = 50;

/// xterm `_getMouseEventScrollAmount`: 0 inside; else the offset clamped to +-50, divided by 50,
/// `sign + round(o * 14)` with JS rounding. Positive = below the viewport = toward newer output.
pub fn drag_scroll_amount(past_css_px: f64) -> i32 {
    if past_css_px == 0.0 || past_css_px.is_nan() {
        return 0;
    }
    let offset = past_css_px.clamp(-50.0, 50.0) / 50.0;
    let sign = if past_css_px < 0.0 { -1 } else { 1 };
    sign + (offset * 14.0 + 0.5).floor() as i32
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    None,
    Char,
    Word,
    Line,
}

pub struct SelMachine {
    mode: Mode,
    orig: Option<Span>,
    start: Option<Anchor>,
    block: bool,
    dragging: bool,
    scroll: i32,
    last_end: Option<Anchor>,
}

impl Default for SelMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl SelMachine {
    pub fn new() -> Self {
        Self {
            mode: Mode::None,
            orig: None,
            start: None,
            block: false,
            dragging: false,
            scroll: 0,
            last_end: None,
        }
    }

    /// Left press. Shift with a selection extends; 1 click starts (and clears), 2 select the
    /// link or word, 3 the logical line, 4 and more do nothing.
    pub fn press(&mut self, env: &dyn SelEnv, p: &Press, has_selection: bool) -> SelOut {
        self.dragging = false;
        self.scroll = 0;
        self.mode = Mode::None;
        self.orig = None;
        self.last_end = None;
        if p.clicks >= 4 {
            return SelOut::Nothing;
        }
        let cols = env.cols();
        if p.shift && has_selection {
            let b = boundary_of(env, p.row, p.col, p.right);
            return SelOut::Extend(Anchor { row: p.row, b });
        }
        match p.clicks {
            2 => {
                let span = env
                    .link_at(p.row, p.col)
                    .or_else(|| env.word_at(p.row, p.col, true));
                let Some(span) = span else {
                    return SelOut::Nothing;
                };
                self.begin(Mode::Word, Some(span), span.start, p.alt);
                SelOut::Set(self.spec(span.start, span.end))
            }
            3 => {
                let (first, last) = env.line_at(p.row);
                let span = Span {
                    start: Anchor { row: first, b: 0 },
                    end: Anchor { row: last, b: cols },
                };
                self.begin(Mode::Line, Some(span), span.start, p.alt);
                SelOut::Set(self.spec(span.start, span.end))
            }
            _ => {
                let a = Anchor {
                    row: p.row,
                    b: boundary_of(env, p.row, p.col, p.right),
                };
                self.begin(Mode::Char, None, a, p.alt);
                self.last_end = Some(a);
                SelOut::Clear
            }
        }
    }

    fn begin(&mut self, mode: Mode, orig: Option<Span>, start: Anchor, block: bool) {
        self.mode = mode;
        self.orig = orig;
        self.start = Some(start);
        self.block = block;
        self.dragging = true;
    }

    fn spec(&self, start: Anchor, end: Anchor) -> SelSpec {
        SelSpec {
            block: self.block,
            start,
            end,
        }
    }

    /// Pointer moved with the button held.
    pub fn drag(&mut self, env: &dyn SelEnv, d: &Drag) -> SelOut {
        if !self.dragging || self.mode == Mode::None {
            return SelOut::Nothing;
        }
        let total = env.total_rows();
        if total == 0 {
            return SelOut::Nothing;
        }
        let cols = env.cols();
        let row = d.row.clamp(0, total as i64 - 1) as usize;
        self.scroll = drag_scroll_amount(d.past_css_px);
        let past = d.past_css_px != 0.0 && !self.block;
        let b = if past {
            if d.past_css_px > 0.0 { cols } else { 0 }
        } else {
            boundary_of(env, row, d.col, d.right)
        };
        let word_col = if past { None } else { Some(d.col) };
        self.extend_to(env, row, b, word_col)
    }

    fn extend_to(
        &mut self,
        env: &dyn SelEnv,
        row: usize,
        b: usize,
        word_col: Option<usize>,
    ) -> SelOut {
        let cols = env.cols();
        let end = Anchor { row, b };
        self.last_end = Some(end);
        match self.mode {
            Mode::None => SelOut::Nothing,
            Mode::Char => match self.start {
                Some(s) => SelOut::Set(self.spec(s, end)),
                None => SelOut::Nothing,
            },
            Mode::Word => {
                let Some(orig) = self.orig else {
                    return SelOut::Nothing;
                };
                let pw = word_col
                    .and_then(|c| env.word_at(row, c, true))
                    .unwrap_or(Span { start: end, end });
                if pw.start < orig.start {
                    SelOut::Set(self.spec(pw.start, orig.end))
                } else {
                    SelOut::Set(self.spec(orig.start, pw.end))
                }
            }
            Mode::Line => {
                let Some(orig) = self.orig else {
                    return SelOut::Nothing;
                };
                let (first, last) = env.line_at(row);
                if first < orig.start.row {
                    SelOut::Set(self.spec(Anchor { row: first, b: 0 }, orig.end))
                } else {
                    SelOut::Set(self.spec(orig.start, Anchor { row: last, b: cols }))
                }
            }
        }
    }

    /// Every DRAG_SCROLL_MS while dragging: the lines to scroll (positive = toward newer).
    pub fn scroll_lines(&self) -> i32 {
        if self.dragging { self.scroll } else { 0 }
    }

    /// After the caller scrolled: the end moves to the last (or first) visible row, column `cols`
    /// (or 0); a block selection keeps its end column.
    pub fn after_scroll(&mut self, env: &dyn SelEnv, view: &ViewBox) -> SelOut {
        if !self.dragging || self.mode == Mode::None || self.scroll == 0 {
            return SelOut::Nothing;
        }
        let cols = env.cols();
        let (row, edge) = if self.scroll > 0 {
            (view.top + view.rows.saturating_sub(1), cols)
        } else {
            (view.top, 0)
        };
        let b = if self.block {
            self.last_end.map_or(edge, |a| a.b)
        } else {
            edge
        };
        self.extend_to(env, row, b, None)
    }

    /// Ends the drag.
    pub fn release(&mut self) {
        self.dragging = false;
        self.scroll = 0;
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging
    }
}

/// Counts presses like a browser's `detail`.
pub struct ClickCounter {
    window_ms: u64,
    dx: f64,
    dy: f64,
    last: Option<(u64, f64, f64, u8, u32)>,
}

impl ClickCounter {
    pub fn new(window_ms: u64, dx: f64, dy: f64) -> Self {
        Self {
            window_ms,
            dx,
            dy,
            last: None,
        }
    }

    /// 1 for a first press; +1 when the same button pressed within `window_ms` and within
    /// +-dx/2, +-dy/2 of the last press.
    pub fn press(&mut self, now_ms: u64, x: f64, y: f64, button: u8) -> u32 {
        let n = match self.last {
            Some((t, lx, ly, b, n))
                if b == button
                    && now_ms.saturating_sub(t) <= self.window_ms
                    && (x - lx).abs() <= self.dx / 2.0
                    && (y - ly).abs() <= self.dy / 2.0 =>
            {
                n + 1
            }
            _ => 1,
        };
        self.last = Some((now_ms, x, y, button, n));
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cells::CellInfo;
    use crate::word_at::word_range;

    struct FakeEnv {
        cols: usize,
        lines: Vec<&'static str>,
        wrapped: Vec<bool>,
        wide: Vec<(usize, usize)>,
        links: Vec<(usize, usize, usize)>,
        total: usize,
    }

    impl FakeEnv {
        fn new(lines: Vec<&'static str>) -> Self {
            let n = lines.len();
            Self {
                cols: 10,
                lines,
                wrapped: vec![false; n],
                wide: vec![],
                links: vec![],
                total: n,
            }
        }
        fn cells(&self, row: usize) -> Vec<CellInfo> {
            let mut v: Vec<CellInfo> = self
                .lines
                .get(row)
                .copied()
                .unwrap_or("")
                .chars()
                .map(|c| CellInfo {
                    chars: c.to_string(),
                    width: 1,
                })
                .collect();
            while v.len() < self.cols.max(14) {
                v.push(CellInfo {
                    chars: String::new(),
                    width: 1,
                });
            }
            v
        }
    }

    impl SelEnv for FakeEnv {
        fn cols(&self) -> usize {
            self.cols
        }
        fn total_rows(&self) -> usize {
            self.total
        }
        fn cell_width(&self, row: usize, col: usize) -> u8 {
            for &(r, c) in &self.wide {
                if r == row && c == col {
                    return 2;
                }
                if r == row && c + 1 == col {
                    return 0;
                }
            }
            1
        }
        fn word_at(&self, row: usize, col: usize, ws: bool) -> Option<Span> {
            word_range(&self.cells(row), col, ws).map(|(s, e)| Span {
                start: Anchor { row, b: s },
                end: Anchor { row, b: e },
            })
        }
        fn line_at(&self, row: usize) -> (usize, usize) {
            let mut first = row;
            while first > 0 && self.wrapped.get(first - 1).copied().unwrap_or(false) {
                first -= 1;
            }
            let mut last = row;
            while self.wrapped.get(last).copied().unwrap_or(false) {
                last += 1;
            }
            (first, last)
        }
        fn link_at(&self, row: usize, col: usize) -> Option<Span> {
            self.links
                .iter()
                .find(|&&(r, s, e)| r == row && col >= s && col < e)
                .map(|&(r, s, e)| Span {
                    start: Anchor { row: r, b: s },
                    end: Anchor { row: r, b: e },
                })
        }
    }

    fn press(row: usize, col: usize, clicks: u32) -> Press {
        Press {
            row,
            col,
            right: false,
            clicks,
            shift: false,
            alt: false,
        }
    }
    fn drag(row: i64, col: usize, right: bool) -> Drag {
        Drag {
            row,
            col,
            right,
            past_css_px: 0.0,
        }
    }
    fn a(row: usize, b: usize) -> Anchor {
        Anchor { row, b }
    }
    fn set(m: &SelOut) -> SelSpec {
        match m {
            SelOut::Set(s) => *s,
            other => panic!("not Set: {other:?}"),
        }
    }

    fn env() -> FakeEnv {
        FakeEnv::new(vec!["", "", "", "", "", "", ""])
    }

    #[test]
    fn a_press_clears_and_a_drag_sets_between_boundaries() {
        let e = env();
        let mut m = SelMachine::new();
        assert_eq!(m.press(&e, &press(2, 3, 1), false), SelOut::Clear);
        let s = set(&m.drag(&e, &drag(2, 6, true)));
        assert_eq!(
            s,
            SelSpec {
                block: false,
                start: a(2, 3),
                end: a(2, 7)
            }
        );
    }

    #[test]
    fn a_press_in_the_right_half_starts_after_the_cell() {
        let e = env();
        let mut m = SelMachine::new();
        let mut p = press(0, 3, 1);
        p.right = true;
        m.press(&e, &p, false);
        let s = set(&m.drag(&e, &drag(0, 5, false)));
        assert_eq!((s.start, s.end), (a(0, 4), a(0, 5)));
    }

    #[test]
    fn releasing_without_moving_leaves_an_empty_selection() {
        let e = env();
        let mut m = SelMachine::new();
        m.press(&e, &press(1, 3, 1), false);
        let s = set(&m.drag(&e, &drag(1, 3, false)));
        assert!(s.is_empty());
    }

    #[test]
    fn alt_makes_a_block() {
        let e = env();
        let mut m = SelMachine::new();
        let mut p = press(1, 1, 1);
        p.alt = true;
        m.press(&e, &p, false);
        assert!(set(&m.drag(&e, &drag(3, 4, false))).block);
    }

    #[test]
    fn a_double_click_selects_the_word_and_a_drag_extends_by_words() {
        let e = FakeEnv::new(vec!["git status now"]);
        let mut m = SelMachine::new();
        let s = set(&m.press(&e, &press(0, 5, 2), false));
        assert_eq!((s.start, s.end), (a(0, 4), a(0, 10)));
        let s = set(&m.drag(&e, &drag(0, 11, false)));
        assert_eq!((s.start, s.end), (a(0, 4), a(0, 14)));
        let s = set(&m.drag(&e, &drag(0, 1, false)));
        assert_eq!((s.start, s.end), (a(0, 0), a(0, 10)));
    }

    #[test]
    fn a_double_click_on_a_link_selects_the_link() {
        let mut e = FakeEnv::new(vec!["see http://x.y/abcdef"]);
        e.links.push((0, 6, 20));
        let mut m = SelMachine::new();
        let s = set(&m.press(&e, &press(0, 10, 2), false));
        assert_eq!((s.start, s.end), (a(0, 6), a(0, 20)));
    }

    #[test]
    fn a_double_click_on_blanks_selects_the_run() {
        let e = FakeEnv::new(vec!["a   b"]);
        let mut m = SelMachine::new();
        let s = set(&m.press(&e, &press(0, 2, 2), false));
        assert_eq!((s.start, s.end), (a(0, 1), a(0, 4)));
    }

    #[test]
    fn a_triple_click_selects_the_logical_line() {
        let mut e = env();
        e.wrapped[3] = true;
        let mut m = SelMachine::new();
        let s = set(&m.press(&e, &press(4, 2, 3), false));
        assert_eq!((s.start, s.end), (a(3, 0), a(4, 10)));
    }

    #[test]
    fn dragging_after_a_triple_click_extends_by_whole_lines() {
        let e = env();
        let mut m = SelMachine::new();
        m.press(&e, &press(2, 2, 3), false);
        let s = set(&m.drag(&e, &drag(5, 1, false)));
        assert_eq!((s.start, s.end), (a(2, 0), a(5, 10)));
        let s = set(&m.drag(&e, &drag(0, 1, false)));
        assert_eq!((s.start, s.end), (a(0, 0), a(2, 10)));
    }

    #[test]
    fn a_fourth_click_does_nothing() {
        let e = env();
        let mut m = SelMachine::new();
        assert_eq!(m.press(&e, &press(1, 1, 4), false), SelOut::Nothing);
        assert!(!m.is_dragging());
    }

    #[test]
    fn shift_extends_an_existing_selection() {
        let e = env();
        let mut m = SelMachine::new();
        let mut p = press(3, 4, 1);
        p.shift = true;
        assert_eq!(m.press(&e, &p, true), SelOut::Extend(a(3, 4)));
        assert_eq!(m.press(&e, &p, false), SelOut::Clear);
    }

    #[test]
    fn a_wide_character_is_taken_whole() {
        let mut e = env();
        e.wide.push((0, 2));
        for right in [false, true] {
            assert_eq!(boundary_of(&e, 0, 2, right), 2);
            assert_eq!(boundary_of(&e, 0, 3, right), 4);
        }
        assert_eq!(boundary_of(&e, 0, 0, false), 0);
        assert_eq!(boundary_of(&e, 0, 0, true), 1);
    }

    #[test]
    fn past_the_bottom_sets_the_end_to_the_row_end_and_a_scroll_amount() {
        let e = env();
        let mut m = SelMachine::new();
        m.press(&e, &press(1, 2, 1), false);
        let d = Drag {
            row: 7,
            col: 3,
            right: false,
            past_css_px: 20.0,
        };
        let s = set(&m.drag(&e, &d));
        assert_eq!(s.end.b, 10);
        assert_eq!(m.scroll_lines(), drag_scroll_amount(20.0));
        assert!(m.scroll_lines() > 0);

        let mut m = SelMachine::new();
        let mut p = press(1, 2, 1);
        p.alt = true;
        m.press(&e, &p, false);
        let s = set(&m.drag(&e, &d));
        assert_eq!(s.end.b, 3);
    }

    #[test]
    fn drag_scroll_amounts_follow_xterm() {
        for (px, want) in [
            (0.0, 0),
            (1.0, 1),
            (5.0, 2),
            (25.0, 8),
            (50.0, 15),
            (100.0, 15),
            (-5.0, -2),
            (-50.0, -15),
        ] {
            assert_eq!(drag_scroll_amount(px), want, "{px}");
        }
    }

    #[test]
    fn after_a_scroll_the_end_follows_the_viewport_edge() {
        let mut e = FakeEnv::new(vec![]);
        e.total = 200;
        let view = ViewBox { top: 100, rows: 24 };
        let mut m = SelMachine::new();
        m.press(&e, &press(110, 2, 1), false);
        m.drag(
            &e,
            &Drag {
                row: 124,
                col: 3,
                right: false,
                past_css_px: 10.0,
            },
        );
        let s = set(&m.after_scroll(&e, &view));
        assert_eq!(s.end, a(123, 10));

        let mut m = SelMachine::new();
        m.press(&e, &press(110, 2, 1), false);
        m.drag(
            &e,
            &Drag {
                row: 99,
                col: 3,
                right: false,
                past_css_px: -10.0,
            },
        );
        let s = set(&m.after_scroll(&e, &view));
        assert_eq!(s.end, a(100, 0));
    }

    #[test]
    fn release_stops_the_drag_and_the_scroll() {
        let e = env();
        let mut m = SelMachine::new();
        m.press(&e, &press(1, 2, 1), false);
        m.drag(
            &e,
            &Drag {
                row: 7,
                col: 3,
                right: false,
                past_css_px: 20.0,
            },
        );
        m.release();
        assert!(!m.is_dragging());
        assert_eq!(m.scroll_lines(), 0);
    }

    #[test]
    fn clicks_count_like_a_browser() {
        let mut c = ClickCounter::new(500, 4.0, 4.0);
        assert_eq!(c.press(0, 10.0, 10.0, 0), 1);
        assert_eq!(c.press(100, 10.0, 10.0, 0), 2);
        assert_eq!(c.press(200, 10.0, 10.0, 0), 3);
        assert_eq!(c.press(300, 10.0, 10.0, 0), 4);
        assert_eq!(c.press(900, 10.0, 10.0, 0), 1);
        assert_eq!(c.press(900, 20.0, 10.0, 0), 1);
        assert_eq!(c.press(950, 20.0, 10.0, 2), 1);
    }
}
