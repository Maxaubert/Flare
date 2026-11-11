//! The window onto alacritty's buffer as a BufferLike.
//! Cells are not characters (Prism Terminal's termCells.ts, rule 21). The screen counts
//! cells: a wide character is two (the second a spacer of width 0), an emoji is one
//! character, a combining sequence is one cell holding several. Absolute rows count from
//! the oldest history line (row 0) down to the last screen row.

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::Term;
use alacritty_terminal::term::cell::{Cell, Flags};

use crate::cells::{BufferLike, LineLike, ScratchCell, by_hand_rows};

/// How many rows a window grows over a wrapped line, each way.
const EXTEND_MAX: usize = 200;

/// Absolute row to alacritty line. `hist` is `term.history_size()` at the call.
pub fn abs_to_line(hist: usize, abs: usize) -> Line {
    Line(abs as i32 - hist as i32)
}

/// Alacritty line to absolute row (clamped at 0).
pub fn line_to_abs(hist: usize, line: Line) -> usize {
    (line.0 + hist as i32).max(0) as usize
}

/// The absolute row shown on viewport row 0: `history_size - display_offset`.
pub fn viewport_top<T: EventListener>(term: &Term<T>) -> usize {
    term.history_size()
        .saturating_sub(term.grid().display_offset())
}

/// `history_size + screen_lines`.
pub fn total_rows<T: EventListener>(term: &Term<T>) -> usize {
    term.history_size() + term.screen_lines()
}

/// One row of the term, read cell by cell.
struct RowRef<'a> {
    row: &'a alacritty_terminal::grid::Row<Cell>,
    cols: usize,
    wrapped: bool,
}

impl LineLike for RowRef<'_> {
    fn is_wrapped(&self) -> bool {
        self.wrapped
    }

    fn load_cell(&self, x: usize, into: &mut ScratchCell) -> bool {
        if x >= self.cols {
            return false;
        }
        let cell = &self.row[Column(x)];
        let flags = cell.flags;
        into.width = if flags.contains(Flags::WIDE_CHAR) {
            2
        } else if flags.contains(Flags::WIDE_CHAR_SPACER) {
            0
        } else {
            1
        };
        into.chars.clear();
        if flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) || cell.c == ' ' || cell.c == '\0' {
            return true;
        }
        into.chars.push(cell.c);
        if let Some(marks) = cell.zerowidth() {
            into.chars.extend(marks.iter());
        }
        true
    }
}

/// A read-only window onto the term's rows, extended over wrapped lines.
pub struct TermBuf<'a> {
    rows: Vec<RowRef<'a>>,
    first: usize,
    total: usize,
}

/// True when the row above `abs` ends in a cell that wraps onto it.
fn continues_above<T: EventListener>(term: &Term<T>, hist: usize, abs: usize) -> bool {
    if abs == 0 {
        return false;
    }
    let cols = term.columns();
    if cols == 0 {
        return false;
    }
    term.grid()[abs_to_line(hist, abs - 1)][Column(cols - 1)]
        .flags
        .contains(Flags::WRAPLINE)
}

/// True when row `abs` carries on the row above, wrapped or broken by hand at the edge by the
/// program (`cells::by_hand`).
fn goes_on_above<T: EventListener>(
    term: &Term<T>,
    hist: usize,
    abs: usize,
    scratch: &mut ScratchCell,
) -> bool {
    if continues_above(term, hist, abs) {
        return true;
    }
    if abs == 0 {
        return false;
    }
    let cols = term.columns();
    let row = |a: usize, wrapped: bool| RowRef {
        row: &term.grid()[abs_to_line(hist, a)],
        cols,
        wrapped,
    };
    let above = row(abs - 1, continues_above(term, hist, abs - 1));
    by_hand_rows(&above, &row(abs, false), cols, scratch)
}

impl<'a> TermBuf<'a> {
    /// Rows `[first, last]` (absolute, clamped), extended upward while the first row
    /// continues the row above and downward while the next row continues this one, at
    /// most 200 rows each way. `len()` is `total_rows`; `line(y)` is `None` outside.
    pub fn window<T: EventListener>(term: &'a Term<T>, first: usize, last: usize) -> Self {
        Self::window_in(term, first, last, false)
    }

    /// `window`, and with `hand` extended over rows broken by hand too (`cells::by_hand`), so a
    /// path a program cut at the edge is read whole wherever its rows start.
    pub fn window_in<T: EventListener>(
        term: &'a Term<T>,
        first: usize,
        last: usize,
        hand: bool,
    ) -> Self {
        let hist = term.history_size();
        let total = total_rows(term);
        if total == 0 {
            return Self {
                rows: Vec::new(),
                first: 0,
                total,
            };
        }
        let mut lo = first.min(total - 1);
        let mut hi = last.min(total - 1).max(lo);
        let mut scratch = ScratchCell::default();
        let mut on = |abs: usize| {
            if hand {
                goes_on_above(term, hist, abs, &mut scratch)
            } else {
                continues_above(term, hist, abs)
            }
        };
        let mut up = 0;
        while lo > 0 && up < EXTEND_MAX && on(lo) {
            lo -= 1;
            up += 1;
        }
        let mut down = 0;
        while hi + 1 < total && down < EXTEND_MAX && on(hi + 1) {
            hi += 1;
            down += 1;
        }
        let cols = term.columns();
        let rows = (lo..=hi)
            .map(|abs| RowRef {
                row: &term.grid()[abs_to_line(hist, abs)],
                cols,
                wrapped: continues_above(term, hist, abs),
            })
            .collect();
        Self {
            rows,
            first: lo,
            total,
        }
    }

    /// The whole buffer (history plus screen). For a click or a search step, never per frame.
    pub fn all<T: EventListener>(term: &'a Term<T>) -> Self {
        let total = total_rows(term);
        Self::window(term, 0, total.saturating_sub(1))
    }
}

impl BufferLike for TermBuf<'_> {
    fn len(&self) -> usize {
        self.total
    }

    fn line(&self, y: usize) -> Option<&dyn LineLike> {
        let i = y.checked_sub(self.first)?;
        self.rows.get(i).map(|r| r as &dyn LineLike)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cells::{buffer_row_cells, logical_line};
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::grid::Scroll;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

    struct Dims(usize, usize);
    impl Dimensions for Dims {
        fn total_lines(&self) -> usize {
            self.1
        }
        fn screen_lines(&self) -> usize {
            self.1
        }
        fn columns(&self) -> usize {
            self.0
        }
    }

    fn term(cols: usize, rows: usize, bytes: &str) -> Term<VoidListener> {
        let mut t = Term::new(Config::default(), &Dims(cols, rows), VoidListener);
        let mut p: Processor = Processor::new();
        p.advance(&mut t, bytes.as_bytes());
        t
    }

    fn row_text(buf: &TermBuf, y: usize, cols: usize) -> String {
        let mut s = ScratchCell::default();
        let cells = buffer_row_cells(buf.line(y), cols, &mut s).unwrap();
        cells.iter().map(|c| c.chars.as_str()).collect()
    }

    #[test]
    fn abs_and_line_round_trip() {
        assert_eq!(line_to_abs(5, abs_to_line(5, 7)), 7);
        assert_eq!(abs_to_line(5, 0), Line(-5));
    }

    #[test]
    fn a_screen_row_reads_as_its_text() {
        let t = term(10, 3, "ab\r\ncd");
        let buf = TermBuf::all(&t);
        assert_eq!(buf.len(), 3);
        assert!(row_text(&buf, 0, 10).starts_with("ab"));
        assert!(row_text(&buf, 1, 10).starts_with("cd"));
    }

    #[test]
    fn a_wide_character_is_two_cells() {
        let t = term(10, 3, "日本");
        let buf = TermBuf::all(&t);
        let mut s = ScratchCell::default();
        let cells = buffer_row_cells(buf.line(0), 10, &mut s).unwrap();
        let got: Vec<(&str, u8)> = cells[..4]
            .iter()
            .map(|c| (c.chars.as_str(), c.width))
            .collect();
        assert_eq!(got, vec![("日", 2), ("", 0), ("本", 2), ("", 0)]);
    }

    #[test]
    fn an_empty_cell_and_a_space_are_the_same() {
        let t = term(10, 3, "a b");
        let buf = TermBuf::all(&t);
        let mut s = ScratchCell::default();
        let cells = buffer_row_cells(buf.line(0), 10, &mut s).unwrap();
        assert_eq!(cells[1].chars, "");
        assert_eq!(cells[1].width, 1);
    }

    #[test]
    fn a_combining_mark_stays_in_its_cell() {
        let t = term(10, 3, "e\u{301}x");
        let buf = TermBuf::all(&t);
        let mut s = ScratchCell::default();
        let cells = buffer_row_cells(buf.line(0), 10, &mut s).unwrap();
        assert_eq!(cells[0].chars, "e\u{301}");
        assert_eq!(cells[1].chars, "x");
    }

    #[test]
    fn a_wrapped_row_continues_the_one_above() {
        let t = term(5, 3, "abcdefgh");
        let buf = TermBuf::all(&t);
        assert!(buf.line(1).unwrap().is_wrapped());
        assert!(!buf.line(0).unwrap().is_wrapped());
        let l = logical_line(&buf, 5, 1);
        assert_eq!(l.first, 0);
        assert!(l.text.starts_with("abcdefgh"));
    }

    #[test]
    fn history_rows_come_first() {
        let t = term(5, 2, "1\r\n2\r\n3\r\n4");
        let hist = t.history_size();
        assert!(hist > 0);
        let buf = TermBuf::all(&t);
        assert_eq!(total_rows(&t), 2 + hist);
        assert_eq!(buf.len(), 2 + hist);
        assert!(row_text(&buf, 0, 5).starts_with('1'));
        assert!(row_text(&buf, buf.len() - 1, 5).starts_with('4'));
    }

    #[test]
    fn viewport_top_follows_the_display_offset() {
        let mut t = term(5, 2, "1\r\n2\r\n3\r\n4");
        let hist = t.history_size();
        assert_eq!(viewport_top(&t), hist);
        t.scroll_display(Scroll::Delta(1));
        assert_eq!(viewport_top(&t), hist - 1);
    }

    #[test]
    fn a_window_extends_over_a_wrapped_line() {
        // Row 2 starts a line of 15 chars: rows 2, 3, 4 (5 columns).
        let t = term(5, 6, "a\r\nb\r\nabcdefghijklmno");
        let total = total_rows(&t);
        let buf = TermBuf::window(&t, 3, 3);
        assert_eq!(buf.len(), total);
        assert!(buf.line(2).is_some());
        assert!(buf.line(3).is_some());
        assert!(buf.line(4).is_some());
        assert!(buf.line(0).is_none());
    }

    #[test]
    fn a_marked_path_the_terminal_wrapped_before_a_wide_character_reads_with_no_space() {
        // Ten columns: `報` would start in the last one, so the terminal leaves it empty and starts the next row.
        let t = term(10, 4, "\u{27E6}D:\\資料\\報告\\a.txt\u{27E7}");
        let buf = TermBuf::all(&t);
        assert!(buf.line(1).unwrap().is_wrapped());
        let m = crate::file_marks::find_marks(&buf, 10, 0, 2);
        assert_eq!(m.len(), 1, "{m:?}");
        assert_eq!(m[0].text, "D:\\資料\\報告\\a.txt");
        assert_eq!(m[0].alts, vec!["D:\\資料\\ 報告\\a.txt"]);
    }

    #[test]
    fn out_of_range_is_none() {
        let t = term(5, 3, "ab");
        let buf = TermBuf::all(&t);
        assert!(buf.line(buf.len()).is_none());
        let mut s = ScratchCell::default();
        assert!(!buf.line(0).unwrap().load_cell(5, &mut s));
    }
}
