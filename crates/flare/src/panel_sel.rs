//! Selecting text in the Text panel (no Slint, no Win32).
//!
//! The panel draws only the rows in view and recycles them as it scrolls, so no Slint element can hold a selection.
//! Here a selection is two positions in the file's own lines, a line and a character in it (`Pos`), so it survives
//! scrolling, wrapping on and off and a splitter drag that wraps the rows again. `RowMap` says which rows a line was
//! wrapped into; the window reports the pointer and draws each row's share of the highlight (`Paint`).
//!
//! The text is the shaped text the panel shows (`preview::shape_line`): tabs as four spaces and a very long line cut.
//! Soft-wrapped rows of one line join without a break; only the ends of real lines become line breaks.

use flare_surfaces::wrap::{MIN_COLS, display_cols, wrap_line};
use flare_term::word_at::WORD_SEPARATORS;

/// The longest label a frame carries (`ask_agent`'s cap): the file's name is cut first, so the lines always show.
const LABEL_MAX: usize = 64;

/// A place between two characters of a line: `ch` characters into line `line` (both from 0).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    pub line: usize,
    pub ch: usize,
}

impl Pos {
    pub fn new(line: usize, ch: usize) -> Self {
        Self { line, ch }
    }
}

/// The columns one character takes, as the wrap counts them.
fn width_of(c: char) -> usize {
    let mut buf = [0u8; 4];
    display_cols(c.encode_utf8(&mut buf))
}

/// How many characters `s` has.
fn chars(s: &str) -> usize {
    s.chars().count()
}

/// The byte where character `ch` of `s` starts (the length when `ch` is past the end).
fn byte_at(s: &str, ch: usize) -> usize {
    s.char_indices().nth(ch).map_or(s.len(), |(b, _)| b)
}

/// The rows the lines are drawn in: one per line, or the rows each line wraps to at `cols`.
#[derive(Clone, Debug, Default)]
pub struct RowMap {
    /// 0: the rows are the lines.
    cols: usize,
    /// The first row of each line (wrapped only).
    first: Vec<usize>,
    rows: usize,
}

impl RowMap {
    /// The rows are the lines.
    pub fn lines(count: usize) -> Self {
        Self {
            cols: 0,
            first: Vec::new(),
            rows: count,
        }
    }

    /// The rows the lines were wrapped into at `cols` (`panel_ctl::wrap_lines`): `nums` holds each row's line number,
    /// 0 on a continuation row.
    pub fn wrapped(cols: usize, nums: &[i32]) -> Self {
        if cols == 0 {
            return Self::lines(nums.iter().filter(|n| **n > 0).count());
        }
        let first = nums
            .iter()
            .enumerate()
            .filter(|(_, n)| **n > 0)
            .map(|(r, _)| r)
            .collect();
        Self {
            cols: cols.max(MIN_COLS),
            first,
            rows: nums.len(),
        }
    }

    pub fn row_count(&self) -> usize {
        self.rows
    }

    /// The first row of `line`.
    fn first_row(&self, line: usize) -> usize {
        if self.cols == 0 {
            line
        } else {
            self.first.get(line).copied().unwrap_or(self.rows)
        }
    }

    /// The line drawn on `row`.
    fn line_of(&self, row: usize) -> usize {
        if self.cols == 0 {
            row
        } else {
            self.first.partition_point(|&r| r <= row).saturating_sub(1)
        }
    }

    /// The pieces `line` is drawn in, one per row: the wrap the panel made, made again.
    fn parts<'a>(&self, line: &'a str) -> Vec<&'a str> {
        if self.cols == 0 {
            vec![line]
        } else {
            wrap_line(line, self.cols)
        }
    }

    /// The place under the pointer: row `row` of the view and `col` columns from the text's left edge (a fraction
    /// picks the nearer side of a character). Above the first row is the start, below the last the end.
    pub fn pos_at<L: AsRef<str>>(&self, lines: &[L], row: i64, col: f32) -> Pos {
        let Some(last) = lines.len().checked_sub(1) else {
            return Pos::default();
        };
        if row < 0 {
            return Pos::default();
        }
        let row = usize::try_from(row).unwrap_or(usize::MAX);
        if row >= self.rows {
            return Pos::new(last, chars(lines[last].as_ref()));
        }
        let line = self.line_of(row).min(last);
        let text = lines[line].as_ref();
        let parts = self.parts(text);
        let k = (row - self.first_row(line)).min(parts.len() - 1);
        let start: usize = parts[..k].iter().map(|p| chars(p)).sum();
        Pos::new(line, start + ch_at_col(parts[k], col))
    }

    /// Where `pos` is drawn: its row and its column in that row. A place where a wrap cut the line is the start of
    /// the next row.
    pub fn place<L: AsRef<str>>(&self, lines: &[L], pos: Pos) -> (usize, usize) {
        let Some(last) = lines.len().checked_sub(1) else {
            return (0, 0);
        };
        let line = pos.line.min(last);
        let parts = self.parts(lines[line].as_ref());
        let mut start = 0;
        for (k, part) in parts.iter().enumerate() {
            let n = chars(part);
            if pos.ch < start + n || k + 1 == parts.len() {
                let within = pos.ch.saturating_sub(start).min(n);
                let col = display_cols(&part[..byte_at(part, within)]);
                return (self.first_row(line) + k, col);
            }
            start += n;
        }
        (self.first_row(line), 0)
    }
}

/// The character `col` columns into `row`: the nearer edge of the character under it.
fn ch_at_col(row: &str, col: f32) -> usize {
    if col <= 0.0 || col.is_nan() {
        return 0;
    }
    let mut at = 0.0f32;
    for (i, c) in row.chars().enumerate() {
        let w = width_of(c) as f32;
        if col < at + w / 2.0 {
            return i;
        }
        at += w;
    }
    chars(row)
}

/// What a press or a drag picks: characters, words (a double click) or lines (a triple click).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Unit {
    #[default]
    Char,
    Word,
    Line,
}

/// The selection: where it started (`anchor`) and where it ends now (`head`), in either order.
#[derive(Clone, Debug, Default)]
pub struct TextSel {
    anchor: Option<Pos>,
    head: Option<Pos>,
    unit: Unit,
    /// The word or line a double or triple click picked: a drag keeps it whole.
    orig: Option<(Pos, Pos)>,
    dragging: bool,
}

impl TextSel {
    /// The selected span, start first; None when nothing is selected.
    pub fn range(&self) -> Option<(Pos, Pos)> {
        let (a, h) = (self.anchor?, self.head?);
        (a != h).then(|| if a < h { (a, h) } else { (h, a) })
    }

    pub fn has(&self) -> bool {
        self.range().is_some()
    }

    /// Where the selection's moving end is (the caret a click left), if anywhere.
    pub fn head(&self) -> Option<Pos> {
        self.head
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    /// Lets go of the selection and the caret. True when something was selected.
    pub fn clear(&mut self) -> bool {
        let had = self.has();
        *self = Self::default();
        had
    }

    /// A left press at `at`: one click puts the caret there, two pick the word, three the line (a fourth starts over).
    /// With Shift and a caret, the selection runs from where it started to here.
    pub fn press<L: AsRef<str>>(&mut self, lines: &[L], at: Pos, clicks: u32, shift: bool) {
        let at = clamp(lines, at);
        self.dragging = true;
        if shift && self.anchor.is_some() {
            self.head = Some(at);
            self.unit = Unit::Char;
            self.orig = None;
            return;
        }
        let (unit, span) = match (clicks.max(1) - 1) % 3 {
            1 => (Unit::Word, word(lines, at)),
            2 => (Unit::Line, line_span(lines, at.line)),
            _ => (Unit::Char, (at, at)),
        };
        self.unit = unit;
        self.orig = (unit != Unit::Char).then_some(span);
        self.anchor = Some(span.0);
        self.head = Some(span.1);
    }

    /// The pointer moved to `at` with the button held: the selection follows, by whole words or lines after a double
    /// or triple click.
    pub fn drag<L: AsRef<str>>(&mut self, lines: &[L], at: Pos) {
        if !self.dragging {
            return;
        }
        let at = clamp(lines, at);
        let span = match self.unit {
            Unit::Char => {
                self.head = Some(at);
                return;
            }
            Unit::Word => word(lines, at),
            Unit::Line => line_span(lines, at.line),
        };
        let Some(orig) = self.orig else { return };
        if span.0 < orig.0 {
            self.anchor = Some(orig.1);
            self.head = Some(span.0);
        } else {
            self.anchor = Some(orig.0);
            self.head = Some(span.1.max(orig.1));
        }
    }

    /// The button went up.
    pub fn release(&mut self) {
        self.dragging = false;
    }

    /// Ctrl+A: every line.
    pub fn select_all<L: AsRef<str>>(&mut self, lines: &[L]) {
        *self = Self::default();
        if let Some(last) = lines.len().checked_sub(1) {
            self.anchor = Some(Pos::default());
            self.head = Some(Pos::new(last, chars(lines[last].as_ref())));
        }
    }

    /// A file link's line: the whole line with its line break, as a triple click picks it.
    pub fn mark_line<L: AsRef<str>>(&mut self, lines: &[L], line: usize) {
        *self = Self::default();
        let Some(last) = lines.len().checked_sub(1) else {
            return;
        };
        let (a, b) = line_span(lines, line.min(last));
        self.anchor = Some(a);
        self.head = Some(b);
    }

    /// Shift with a key: the moving end goes to `to`; with no caret yet the selection starts at `from`.
    pub fn extend<L: AsRef<str>>(&mut self, lines: &[L], from: Pos, to: Pos) {
        if self.anchor.is_none() {
            self.anchor = Some(clamp(lines, from));
        }
        self.head = Some(clamp(lines, to));
        self.unit = Unit::Char;
        self.orig = None;
        self.dragging = false;
    }
}

/// `at` moved inside the text.
fn clamp<L: AsRef<str>>(lines: &[L], at: Pos) -> Pos {
    let Some(last) = lines.len().checked_sub(1) else {
        return Pos::default();
    };
    let line = at.line.min(last);
    Pos::new(line, at.ch.min(chars(lines[line].as_ref())))
}

/// The end of the text.
pub fn end_of<L: AsRef<str>>(lines: &[L]) -> Pos {
    lines.len().checked_sub(1).map_or(Pos::default(), |last| {
        Pos::new(last, chars(lines[last].as_ref()))
    })
}

/// A separator between words, as the terminal's double click has them (xterm's `wordSeparator`), or any space.
fn is_separator(c: char) -> bool {
    c.is_whitespace() || WORD_SEPARATORS.contains(c)
}

/// The word under `at`: a run of letters between separators, a run of spaces, or one separator alone. A place past
/// a line's end picks its last character's word; an empty line, or no line at all (an empty file), the empty span.
fn word<L: AsRef<str>>(lines: &[L], at: Pos) -> (Pos, Pos) {
    let cs: Vec<char> = lines
        .get(at.line)
        .map_or_else(Vec::new, |l| l.as_ref().chars().collect());
    if cs.is_empty() {
        return (at, at);
    }
    let i = at.ch.min(cs.len() - 1);
    let same: Box<dyn Fn(char) -> bool> = if cs[i].is_whitespace() {
        Box::new(char::is_whitespace)
    } else if is_separator(cs[i]) {
        return (Pos::new(at.line, i), Pos::new(at.line, i + 1));
    } else {
        Box::new(|c| !is_separator(c))
    };
    let mut s = i;
    while s > 0 && same(cs[s - 1]) {
        s -= 1;
    }
    let mut e = i + 1;
    while e < cs.len() && same(cs[e]) {
        e += 1;
    }
    (Pos::new(at.line, s), Pos::new(at.line, e))
}

/// A whole line with its line break (the last line has none). No lines (an empty file) is the empty span at the start.
fn line_span<L: AsRef<str>>(lines: &[L], line: usize) -> (Pos, Pos) {
    let start = Pos::new(line, 0);
    if line + 1 < lines.len() {
        (start, Pos::new(line + 1, 0))
    } else {
        let len = lines.get(line).map_or(0, |l| chars(l.as_ref()));
        (start, Pos::new(line, len))
    }
}

/// The selected text: the shaped lines, a line break where a line ends, nothing where a wrap cut one.
pub fn text_of<L: AsRef<str>>(lines: &[L], (a, b): (Pos, Pos)) -> String {
    let (a, b) = (clamp(lines, a), clamp(lines, b));
    if lines.is_empty() || a >= b {
        return String::new();
    }
    let piece = |line: usize, from: usize, to: Option<usize>| {
        let s = lines[line].as_ref();
        let end = to.map_or(s.len(), |t| byte_at(s, t));
        &s[byte_at(s, from).min(end)..end]
    };
    if a.line == b.line {
        return piece(a.line, a.ch, Some(b.ch)).to_string();
    }
    let mut out = String::from(piece(a.line, a.ch, None));
    for line in &lines[a.line + 1..b.line] {
        out.push('\n');
        out.push_str(line.as_ref());
    }
    out.push('\n');
    out.push_str(piece(b.line, 0, Some(b.ch)));
    out
}

/// The frame's source for a selection: `<name> lines A-B` (or `line A`), counted from 1. A selection that ends at the
/// start of a line (a triple click's) does not count that line. The name is cut first, so the whole label fits the
/// frame's 64 characters.
pub fn label(name: &str, (a, b): (Pos, Pos)) -> String {
    let first = a.line + 1;
    let last = if b.ch == 0 && b.line > a.line {
        b.line
    } else {
        b.line + 1
    };
    let lines = if last <= first {
        format!(" line {first}")
    } else {
        format!(" lines {first}-{last}")
    };
    let room = LABEL_MAX.saturating_sub(chars(&lines));
    let name = name.trim();
    let name: String = if chars(name) > room {
        let mut cut: String = name.chars().take(room.saturating_sub(1)).collect();
        cut.push('\u{2026}');
        cut
    } else {
        name.to_string()
    };
    format!("{name}{lines}")
}

/// What the window draws of a selection: from column `a_col` of row `a_row` to column `b_col` of row `b_row` (the end
/// is not included). Rows between are drawn whole.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Paint {
    pub a_row: usize,
    pub a_col: usize,
    pub b_row: usize,
    pub b_col: usize,
}

/// Where the window draws the span.
pub fn paint<L: AsRef<str>>(map: &RowMap, lines: &[L], (a, b): (Pos, Pos)) -> Paint {
    let (a_row, a_col) = map.place(lines, a);
    let (b_row, b_col) = map.place(lines, b);
    Paint {
        a_row,
        a_col,
        b_row,
        b_col,
    }
}

/// A key that moves the selection's end (with Shift).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    DocStart,
    DocEnd,
}

/// Where `from` goes on `step`: a character left or right (across line ends), a row up or down at the same column,
/// the line's start or end, or the text's. With no lines (an empty file) every step stays at the start.
pub fn step<L: AsRef<str>>(map: &RowMap, lines: &[L], from: Pos, step: Step) -> Pos {
    if lines.is_empty() {
        return Pos::default();
    }
    let from = clamp(lines, from);
    let len = |l: usize| lines.get(l).map_or(0, |s| chars(s.as_ref()));
    match step {
        Step::Left if from.ch > 0 => Pos::new(from.line, from.ch - 1),
        Step::Left if from.line > 0 => Pos::new(from.line - 1, len(from.line - 1)),
        Step::Left => from,
        Step::Right if from.ch < len(from.line) => Pos::new(from.line, from.ch + 1),
        Step::Right if from.line + 1 < lines.len() => Pos::new(from.line + 1, 0),
        Step::Right => from,
        Step::Up | Step::Down => {
            let (row, col) = map.place(lines, from);
            let row = if step == Step::Up {
                if row == 0 {
                    return Pos::new(from.line, 0);
                }
                row - 1
            } else {
                if row + 1 >= map.row_count() {
                    return end_of(lines);
                }
                row + 1
            };
            map.pos_at(lines, i64::try_from(row).unwrap_or(i64::MAX), col as f32)
        }
        Step::Home => Pos::new(from.line, 0),
        Step::End => Pos::new(from.line, len(from.line)),
        Step::DocStart => Pos::default(),
        Step::DocEnd => end_of(lines),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nums_of(lines: &[&str], cols: usize) -> Vec<i32> {
        let mut nums = Vec::new();
        for (i, l) in lines.iter().enumerate() {
            for k in 0..wrap_line(l, cols.max(MIN_COLS)).len() {
                nums.push(if k == 0 { i as i32 + 1 } else { 0 });
            }
        }
        nums
    }

    #[test]
    fn a_click_between_characters_lands_on_the_nearer_edge() {
        let lines = ["hello world"];
        let map = RowMap::lines(1);
        assert_eq!(map.pos_at(&lines, 0, 0.0), Pos::new(0, 0));
        assert_eq!(map.pos_at(&lines, 0, 0.4), Pos::new(0, 0));
        assert_eq!(map.pos_at(&lines, 0, 0.6), Pos::new(0, 1));
        assert_eq!(map.pos_at(&lines, 0, 4.9), Pos::new(0, 5));
        assert_eq!(map.pos_at(&lines, 0, -3.0), Pos::new(0, 0), "the gutter");
        assert_eq!(map.pos_at(&lines, 0, 80.0), Pos::new(0, 11), "past the end");
        assert_eq!(map.pos_at(&lines, -1, 3.0), Pos::new(0, 0), "above");
        assert_eq!(map.pos_at(&lines, 9, 3.0), Pos::new(0, 11), "below");
    }

    #[test]
    fn a_wide_character_takes_two_columns_and_an_emoji_too() {
        let lines = ["a\u{65e5}\u{672c}b", "\u{1f600}x\u{1f600}"];
        let map = RowMap::lines(2);
        // a | 日 (cols 1-2) | 本 (cols 3-4) | b
        assert_eq!(map.pos_at(&lines, 0, 1.9), Pos::new(0, 1));
        assert_eq!(map.pos_at(&lines, 0, 2.1), Pos::new(0, 2));
        assert_eq!(map.pos_at(&lines, 0, 4.2), Pos::new(0, 3));
        assert_eq!(map.place(&lines, Pos::new(0, 3)), (0, 5));
        assert_eq!(map.pos_at(&lines, 1, 2.2), Pos::new(1, 1));
        assert_eq!(map.place(&lines, Pos::new(1, 2)), (1, 3));
        let mut sel = TextSel::default();
        sel.press(&lines, Pos::new(0, 1), 1, false);
        sel.drag(&lines, Pos::new(1, 2));
        assert_eq!(
            text_of(&lines, sel.range().unwrap()),
            "\u{65e5}\u{672c}b\n\u{1f600}x"
        );
    }

    #[test]
    fn wrapped_rows_map_back_to_the_line_and_character_they_show() {
        let lines = ["0123456789abcdefghij", "short"];
        let map = RowMap::wrapped(8, &nums_of(&lines, 8));
        assert_eq!(map.row_count(), 4);
        assert_eq!(map.pos_at(&lines, 1, 2.0), Pos::new(0, 10));
        assert_eq!(map.pos_at(&lines, 2, 1.0), Pos::new(0, 17));
        assert_eq!(map.pos_at(&lines, 3, 1.0), Pos::new(1, 1));
        // A place where the wrap cut the line is the next row's start; the line's end stays on its last row.
        assert_eq!(map.place(&lines, Pos::new(0, 8)), (1, 0));
        assert_eq!(map.place(&lines, Pos::new(0, 20)), (2, 4));
        assert_eq!(map.place(&lines, Pos::new(1, 5)), (3, 5));
    }

    #[test]
    fn soft_wrapped_rows_of_one_line_join_without_a_break() {
        let lines = ["the quick brown fox jumps over", "next"];
        let map = RowMap::wrapped(10, &nums_of(&lines, 10));
        assert!(map.row_count() > 3);
        let mut sel = TextSel::default();
        sel.press(&lines, map.pos_at(&lines, 0, 0.0), 1, false);
        sel.drag(&lines, map.pos_at(&lines, 99, 0.0));
        assert_eq!(
            text_of(&lines, sel.range().unwrap()),
            "the quick brown fox jumps over\nnext"
        );
    }

    #[test]
    fn the_selection_is_kept_in_lines_so_wrapping_on_and_off_keeps_it() {
        let lines = ["0123456789abcdefghij", "short"];
        let mut sel = TextSel::default();
        let wrapped = RowMap::wrapped(8, &nums_of(&lines, 8));
        sel.press(&lines, wrapped.pos_at(&lines, 1, 2.0), 1, false);
        sel.drag(&lines, wrapped.pos_at(&lines, 3, 2.0));
        sel.release();
        let range = sel.range().unwrap();
        assert_eq!(text_of(&lines, range), "abcdefghij\nsh");
        assert_eq!(
            paint(&wrapped, &lines, range),
            Paint {
                a_row: 1,
                a_col: 2,
                b_row: 3,
                b_col: 2
            }
        );
        let flat = RowMap::lines(2);
        assert_eq!(
            paint(&flat, &lines, range),
            Paint {
                a_row: 0,
                a_col: 10,
                b_row: 1,
                b_col: 2
            }
        );
        assert_eq!(text_of(&lines, sel.range().unwrap()), "abcdefghij\nsh");
    }

    #[test]
    fn a_double_click_picks_the_word_and_a_drag_goes_by_whole_words() {
        let lines = ["let (alpha, beta) = gamma;"];
        let mut sel = TextSel::default();
        sel.press(&lines, Pos::new(0, 7), 2, false);
        assert_eq!(text_of(&lines, sel.range().unwrap()), "alpha");
        sel.drag(&lines, Pos::new(0, 22));
        assert_eq!(
            text_of(&lines, sel.range().unwrap()),
            "alpha, beta) = gamma;"
        );
        sel.drag(&lines, Pos::new(0, 1));
        assert_eq!(text_of(&lines, sel.range().unwrap()), "let (alpha");
        // A separator alone, a run of spaces.
        sel.press(&lines, Pos::new(0, 4), 2, false);
        assert_eq!(text_of(&lines, sel.range().unwrap()), "(");
        let spaced = ["a    b"];
        sel.press(&spaced, Pos::new(0, 3), 2, false);
        assert_eq!(text_of(&spaced, sel.range().unwrap()), "    ");
    }

    #[test]
    fn a_triple_click_picks_the_line_with_its_break_and_a_drag_goes_by_lines() {
        let lines = ["one", "two", "three"];
        let mut sel = TextSel::default();
        sel.press(&lines, Pos::new(1, 1), 3, false);
        assert_eq!(text_of(&lines, sel.range().unwrap()), "two\n");
        sel.drag(&lines, Pos::new(2, 0));
        assert_eq!(text_of(&lines, sel.range().unwrap()), "two\nthree");
        sel.drag(&lines, Pos::new(0, 2));
        assert_eq!(text_of(&lines, sel.range().unwrap()), "one\ntwo\n");
        // A fourth click starts over as one.
        sel.press(&lines, Pos::new(0, 1), 4, false);
        assert!(!sel.has());
    }

    #[test]
    fn shift_click_extends_from_where_the_selection_started() {
        let lines = ["abcdef", "ghijkl"];
        let mut sel = TextSel::default();
        sel.press(&lines, Pos::new(0, 2), 1, false);
        sel.release();
        assert!(!sel.has(), "a click is a caret");
        sel.press(&lines, Pos::new(1, 3), 1, true);
        sel.release();
        assert_eq!(text_of(&lines, sel.range().unwrap()), "cdef\nghi");
        sel.press(&lines, Pos::new(0, 0), 1, true);
        assert_eq!(text_of(&lines, sel.range().unwrap()), "ab");
        // Shift with no caret is a plain click.
        let mut fresh = TextSel::default();
        fresh.press(&lines, Pos::new(1, 1), 1, true);
        assert!(!fresh.has());
    }

    #[test]
    fn select_all_takes_every_line_and_clear_says_whether_there_was_one() {
        let lines = ["a", "", "b c"];
        let mut sel = TextSel::default();
        assert!(!sel.clear());
        sel.select_all(&lines);
        assert_eq!(text_of(&lines, sel.range().unwrap()), "a\n\nb c");
        assert!(sel.clear());
        assert!(sel.range().is_none());
        let empty: [&str; 0] = [];
        sel.select_all(&empty);
        assert!(!sel.has());
    }

    #[test]
    fn shift_with_the_arrows_moves_the_end_across_lines_and_rows() {
        let lines = ["abc", "defgh"];
        let map = RowMap::lines(2);
        let mut sel = TextSel::default();
        sel.press(&lines, Pos::new(0, 2), 1, false);
        sel.release();
        let head = sel.head().unwrap();
        let to = step(&map, &lines, head, Step::Right);
        sel.extend(&lines, head, to);
        let to = step(&map, &lines, sel.head().unwrap(), Step::Right);
        sel.extend(&lines, head, to);
        assert_eq!(text_of(&lines, sel.range().unwrap()), "c\n");
        let to = step(&map, &lines, sel.head().unwrap(), Step::End);
        sel.extend(&lines, head, to);
        assert_eq!(text_of(&lines, sel.range().unwrap()), "c\ndefgh");
        let to = step(&map, &lines, sel.head().unwrap(), Step::Up);
        sel.extend(&lines, head, to);
        assert_eq!(
            text_of(&lines, sel.range().unwrap()),
            "c",
            "a row up, the same column cut to the line"
        );
        assert_eq!(
            step(&map, &lines, Pos::new(1, 4), Step::Down),
            Pos::new(1, 5)
        );
        assert_eq!(step(&map, &lines, Pos::new(0, 1), Step::Up), Pos::new(0, 0));
        assert_eq!(
            step(&map, &lines, Pos::new(1, 0), Step::Left),
            Pos::new(0, 3)
        );
        assert_eq!(
            step(&map, &lines, Pos::new(1, 2), Step::DocStart),
            Pos::default()
        );
        // Wrapped: down goes a row, not a line.
        let long = ["0123456789abcdefghij"];
        let wmap = RowMap::wrapped(8, &nums_of(&long, 8));
        assert_eq!(
            step(&wmap, &long, Pos::new(0, 2), Step::Down),
            Pos::new(0, 10)
        );
    }

    #[test]
    fn the_label_names_the_lines_and_cuts_the_name_to_fit() {
        assert_eq!(
            label("notes.md", (Pos::new(11, 0), Pos::new(39, 4))),
            "notes.md lines 12-40"
        );
        assert_eq!(
            label("notes.md", (Pos::new(2, 1), Pos::new(2, 4))),
            "notes.md line 3"
        );
        // A triple click's span ends at the next line's start: that line is not in it.
        assert_eq!(
            label("a.txt", (Pos::new(4, 0), Pos::new(5, 0))),
            "a.txt line 5"
        );
        let long = "x".repeat(100);
        let l = label(&long, (Pos::new(0, 0), Pos::new(1233, 2)));
        assert_eq!(l.chars().count(), 64);
        assert!(l.ends_with("\u{2026} lines 1-1234"), "{l}");
    }

    #[test]
    fn an_empty_file_takes_every_click_drag_and_key_without_a_selection() {
        let empty: [&str; 0] = [];
        let map = RowMap::lines(0);
        let mut sel = TextSel::default();
        for clicks in 1..=3 {
            sel.press(&empty, map.pos_at(&empty, 0, 2.0), clicks, false);
            sel.drag(&empty, map.pos_at(&empty, 3, 5.0));
            sel.release();
            assert!(!sel.has(), "{clicks} clicks");
        }
        for s in [
            Step::Left,
            Step::Right,
            Step::Up,
            Step::Down,
            Step::Home,
            Step::End,
            Step::DocStart,
            Step::DocEnd,
        ] {
            let to = step(&map, &empty, Pos::new(0, 0), s);
            assert_eq!(to, Pos::default(), "{s:?}");
            sel.extend(&empty, Pos::default(), to);
        }
        assert!(!sel.has());
        assert_eq!(text_of(&empty, (Pos::new(0, 0), Pos::new(4, 4))), "");
    }

    #[test]
    fn a_clamped_place_never_runs_past_the_text() {
        let lines = ["ab", "c"];
        let mut sel = TextSel::default();
        sel.press(&lines, Pos::new(7, 9), 1, false);
        assert_eq!(sel.head(), Some(Pos::new(1, 1)));
        assert_eq!(text_of(&lines, (Pos::new(0, 1), Pos::new(9, 9))), "b\nc");
    }

    #[test]
    fn mark_line_picks_the_whole_line_as_a_triple_click_does() {
        let lines = ["one", "two", "three"];
        let mut s = TextSel::default();
        s.mark_line(&lines, 1);
        assert_eq!(s.range(), Some((Pos::new(1, 0), Pos::new(2, 0))));
        s.mark_line(&lines, 9);
        assert_eq!(s.range(), Some((Pos::new(2, 0), Pos::new(2, 5))));
        s.mark_line(&[] as &[&str], 0);
        assert_eq!(s.range(), None);
    }
}
