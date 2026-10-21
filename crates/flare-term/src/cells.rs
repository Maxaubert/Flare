//! Cells are not characters (Prism Terminal's termCells.ts, rule 21). The screen counts
//! cells: a wide character is two (the second a spacer of width 0), an emoji is one
//! character, a combining sequence is one cell holding several. Every text position here
//! is a char index (Unicode scalar value), and every caller converts through `index`.

/// What one screen cell holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellInfo {
    /// What the cell holds; "" for an empty cell and for a wide character's spacer.
    pub chars: String,
    /// 1, 2 for a wide character, 0 for its spacer.
    pub width: u8,
}

/// The cells as one string, with the char index where each cell's text starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellText {
    /// Empty cells as spaces, spacers as nothing.
    pub text: String,
    /// `index[i]` is where cell i's text starts (a spacer shares its character's);
    /// `index[cells.len()]` is the text length in chars.
    pub index: Vec<usize>,
    /// The cell just past the last one holding anything.
    pub text_end: usize,
}

/// Builds the text of a row of cells, empty cells as spaces.
pub fn cell_text(cells: &[CellInfo]) -> CellText {
    let mut text = String::new();
    let mut text_len = 0usize;
    let mut text_end = 0usize;
    let mut index = Vec::with_capacity(cells.len() + 1);
    for (i, cell) in cells.iter().enumerate() {
        if cell.width == 0 {
            index.push(if i > 0 { index[i - 1] } else { text_len });
            continue;
        }
        index.push(text_len);
        let piece = if cell.chars.is_empty() {
            " "
        } else {
            cell.chars.as_str()
        };
        text.push_str(piece);
        text_len += piece.chars().count();
        if !cell.chars.is_empty() {
            text_end = i + usize::from(cell.width.max(1));
        }
    }
    index.push(text_len);
    CellText {
        text,
        index,
        text_end,
    }
}

/// Where a cursor `back` cells before (or, negative, after) the position (`row`, `col`)
/// lands on a screen `cols` wide, following the wrap. Rows are never walked above 0.
pub fn cell_from(row: i32, col: i32, back: i32, cols: i32) -> (i32, i32) {
    let mut r = row;
    let mut c = col - back;
    while c < 0 && r > 0 {
        c += cols;
        r -= 1;
    }
    while c >= cols {
        c -= cols;
        r += 1;
    }
    (r, c.max(0))
}

/// One cell read into the reusable scratch: its chars and its width.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScratchCell {
    pub chars: String,
    pub width: u8,
}

impl Default for ScratchCell {
    fn default() -> Self {
        Self {
            chars: String::new(),
            width: 1,
        }
    }
}

/// One line of a buffer, as the cell reader sees it.
pub trait LineLike {
    fn is_wrapped(&self) -> bool;
    /// Loads cell `x` into `into`; false when the line has no such cell.
    fn load_cell(&self, x: usize, into: &mut ScratchCell) -> bool;
}

/// A buffer of lines.
pub trait BufferLike {
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn line(&self, y: usize) -> Option<&dyn LineLike>;
}

/// The cells of one row. None for a missing line; otherwise `cols` cells, every read going
/// through the ONE `scratch`, each value copied out. A missing cell reads as ("", 1).
pub fn buffer_row_cells(
    line: Option<&dyn LineLike>,
    cols: usize,
    scratch: &mut ScratchCell,
) -> Option<Vec<CellInfo>> {
    let line = line?;
    let mut out = Vec::with_capacity(cols);
    for x in 0..cols {
        *scratch = ScratchCell::default();
        line.load_cell(x, scratch);
        out.push(CellInfo {
            chars: scratch.chars.clone(),
            width: scratch.width,
        });
    }
    Some(out)
}

/// The logical line holding buffer row `y`: its first and last rows across wraps, every
/// cell's width, the joined text, its index and the cell just past the last printed one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogicalLine {
    pub first: usize,
    pub last: usize,
    pub widths: Vec<u8>,
    pub text: String,
    pub index: Vec<usize>,
    pub text_end: usize,
    /// Where in `text` (char index) a row broken BY HAND starts (`by_hand`); empty unless asked
    /// for (`logical_line_in` with `hand`).
    pub joins: Vec<usize>,
}

/// What a path's piece can be made of: one character that is not a break of `paths.rs`
/// (white space, a quote, `<>|*?`) and not a box-drawing character (a table's or a frame's
/// edge, U+2500-U+257F). PT termCells.ts `PATHISH`.
fn pathish(s: &str) -> bool {
    let mut it = s.chars();
    let (Some(c), None) = (it.next(), it.next()) else {
        return false;
    };
    !(c.is_whitespace()
        || matches!(c, '"' | '\'' | '`' | '<' | '>' | '|' | '*' | '?')
        || ('\u{2500}'..='\u{257F}').contains(&c))
}

/// A ROW THE PROGRAM BROKE BY HAND (owner, 2026-10-21, PT #250: a path Claude printed was not a
/// link). Claude Code breaks its own lines at the window's width and writes each row on its own,
/// so the second row is never marked wrapped, and a long path became two pieces that each named
/// nothing. Row `y` carries on row `y - 1` when that row's last cell and this row's first both
/// hold a piece of a path, and the piece across the edge has a separator in it. Only PATHS are
/// read across it: a path that exists lights up, and a sentence that ends at the edge stays.
pub fn by_hand(b: &dyn BufferLike, cols: usize, y: usize, scratch: &mut ScratchCell) -> bool {
    if y == 0 || cols < 2 {
        return false;
    }
    let (Some(row), Some(above)) = (b.line(y), b.line(y - 1)) else {
        return false;
    };
    by_hand_rows(above, row, cols, scratch)
}

/// `by_hand` over the two rows themselves: `row` carries on `above`.
pub fn by_hand_rows(
    above: &dyn LineLike,
    row: &dyn LineLike,
    cols: usize,
    scratch: &mut ScratchCell,
) -> bool {
    if cols < 2 || row.is_wrapped() {
        return false;
    }
    // The cell's chars, or None for a wide character's spacer (skipped, as PT's width 0 "").
    let mut at = |line: &dyn LineLike, x: usize| -> Option<String> {
        *scratch = ScratchCell::default();
        line.load_cell(x, scratch);
        (!(scratch.chars.is_empty() && scratch.width == 0)).then(|| scratch.chars.clone())
    };
    let edge_ok = |c: Option<String>| c.is_some_and(|c| pathish(&c));
    if !edge_ok(at(above, cols - 1)) || !edge_ok(at(row, 0)) {
        return false;
    }
    let mut sep = false;
    for x in (0..cols).rev() {
        let Some(c) = at(above, x) else { continue };
        if !pathish(&c) {
            break;
        }
        sep |= c.contains(['\\', '/']);
    }
    for x in 0..cols {
        let Some(c) = at(row, x) else { continue };
        if !pathish(&c) {
            break;
        }
        sep |= c.contains(['\\', '/']);
    }
    sep
}

pub fn logical_line(b: &dyn BufferLike, cols: usize, y: usize) -> LogicalLine {
    logical_line_in(b, cols, y, false)
}

/// `logical_line`, and with `hand` across rows broken `by_hand` too, with `joins` saying where
/// those rows start in the text (PT termCells.ts `logicalLine(..., hand)`).
pub fn logical_line_in(b: &dyn BufferLike, cols: usize, y: usize, hand: bool) -> LogicalLine {
    let mut scratch = ScratchCell::default();
    let mut goes_on = |r: usize| {
        b.line(r).is_some_and(LineLike::is_wrapped) || (hand && by_hand(b, cols, r, &mut scratch))
    };
    let mut first = y;
    while first > 0 && goes_on(first) {
        first -= 1;
    }
    let mut last = y;
    while last + 1 < b.len() && goes_on(last + 1) {
        last += 1;
    }
    let mut cells = Vec::new();
    let mut hand_rows = Vec::new();
    for i in first..=last {
        if i > first && !b.line(i).is_some_and(LineLike::is_wrapped) {
            hand_rows.push(i - first);
        }
        cells.extend(buffer_row_cells(b.line(i), cols, &mut scratch).unwrap_or_default());
    }
    let widths = cells.iter().map(|c| c.width).collect();
    let CellText {
        text,
        index,
        text_end,
    } = cell_text(&cells);
    let joins = hand_rows
        .iter()
        .filter_map(|k| index.get(k * cols).copied())
        .collect();
    LogicalLine {
        first,
        last,
        widths,
        text,
        index,
        text_end,
        joins,
    }
}

/// The char index of the cell at buffer row `row`, column `col`, clamped to the line.
pub fn char_at_cell(line: &LogicalLine, cols: usize, row: i32, col: i32) -> usize {
    let cell = (row - line.first as i32) as i64 * cols as i64 + i64::from(col);
    let last = line.index.len() - 1;
    line.index[cell.clamp(0, last as i64) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::links::find_links;

    /// `T("ab")`: one width-1 cell per char. `W("日")`: a wide cell and its spacer.
    enum Part {
        T(&'static str),
        W(&'static str),
    }

    fn cells(parts: &[Part], pad: usize) -> Vec<CellInfo> {
        let mut out = Vec::new();
        for part in parts {
            match part {
                Part::T(s) => {
                    for ch in s.chars() {
                        out.push(CellInfo {
                            chars: ch.to_string(),
                            width: 1,
                        });
                    }
                }
                Part::W(s) => {
                    out.push(CellInfo {
                        chars: (*s).to_string(),
                        width: 2,
                    });
                    out.push(CellInfo {
                        chars: String::new(),
                        width: 0,
                    });
                }
            }
        }
        out.extend((0..pad).map(|_| CellInfo {
            chars: String::new(),
            width: 1,
        }));
        out
    }

    #[test]
    fn cell_text_trailing_blanks_are_spaces() {
        let t = cell_text(&cells(&[Part::T("PS C:\\> ")], 3));
        assert_eq!(t.text, "PS C:\\>    ");
        assert_eq!(t.text_end, 8);
        assert_eq!(t.index[8], 8);
    }

    #[test]
    fn cell_text_wide_is_two_cells_one_char() {
        let t = cell_text(&cells(&[Part::T("a"), Part::W("日"), Part::T("b")], 0));
        assert_eq!(t.text, "a日b");
        assert_eq!(t.index, vec![0, 1, 1, 2, 3]);
        assert_eq!(t.text_end, 4);
    }

    #[test]
    fn cell_text_emoji_keeps_link_edges() {
        let t = cell_text(&cells(&[Part::W("😀"), Part::T(" https://a.io/x more")], 0));
        let link = find_links(&t.text)[0];
        let first_h = 3usize;
        let after_x = 3 + "https://a.io/x".chars().count();
        let on = |cell: usize| t.index[cell] >= link.start && t.index[cell] < link.end;
        assert!(on(first_h));
        assert!(on(after_x - 1));
        assert!(!on(after_x));
    }

    #[test]
    fn cell_from_walks_back_over_wraps() {
        assert_eq!(cell_from(5, 2, 4, 10), (4, 8));
    }

    #[test]
    fn cell_from_moves_right_wrapping() {
        assert_eq!(cell_from(5, 9, -1, 10), (6, 0));
        assert_eq!(cell_from(5, 7, -1, 10), (5, 8));
    }

    #[test]
    fn cell_from_never_above_row_zero() {
        assert_eq!(cell_from(0, 1, 5, 10), (0, 0));
    }

    struct Row {
        text: &'static str,
        wrapped: bool,
    }
    impl LineLike for Row {
        fn is_wrapped(&self) -> bool {
            self.wrapped
        }
        fn load_cell(&self, x: usize, into: &mut ScratchCell) -> bool {
            into.chars = self
                .text
                .chars()
                .nth(x)
                .map(|c| c.to_string())
                .unwrap_or_default();
            into.width = 1;
            true
        }
    }
    struct Buf {
        rows: Vec<Row>,
    }
    impl BufferLike for Buf {
        fn len(&self) -> usize {
            self.rows.len()
        }
        fn line(&self, y: usize) -> Option<&dyn LineLike> {
            self.rows.get(y).map(|r| r as &dyn LineLike)
        }
    }
    fn buf(rows: &[(&'static str, bool)]) -> Buf {
        Buf {
            rows: rows
                .iter()
                .map(|&(text, wrapped)| Row { text, wrapped })
                .collect(),
        }
    }

    #[test]
    fn logical_line_joins_from_any_row() {
        let b = buf(&[
            ("$ ls", false),
            ("abcd", false),
            ("efgh", true),
            ("ij", true),
            ("next", false),
        ]);
        for y in [1, 2, 3] {
            let l = logical_line(&b, 4, y);
            assert_eq!((l.first, l.last), (1, 3));
            assert_eq!(l.text, "abcdefghij  ");
            assert_eq!(l.text_end, 10);
        }
    }

    // Owner, 2026-10-21 (PT #250): Claude Code breaks its own lines at the window's width and
    // writes each row on its own, so the next row is never marked wrapped.
    #[test]
    fn a_path_cut_at_the_edge_carries_on_into_the_next_row() {
        let b = buf(&[("at C:\\t\\scra", false), ("tch\\a.exe", false)]);
        assert!(by_hand(&b, 12, 1, &mut ScratchCell::default()));
        let l = logical_line_in(&b, 12, 1, true);
        assert_eq!((l.first, l.last), (0, 1));
        assert_eq!(l.text.trim_end(), "at C:\\t\\scratch\\a.exe");
        assert_eq!(l.joins, vec![12]);
        // Not asked for: the line is the row, as before.
        let l = logical_line(&b, 12, 1);
        assert_eq!((l.first, l.joins.len()), (1, 0));
        // A wrapped row is no join.
        let w = buf(&[("at C:\\t\\scra", false), ("tch\\a.exe", true)]);
        assert!(logical_line_in(&w, 12, 1, true).joins.is_empty());
    }

    #[test]
    fn no_hand_break_when_short_blank_or_no_separator() {
        let no = |rows: &[(&'static str, bool)], cols: usize| {
            !by_hand(&buf(rows), cols, 1, &mut ScratchCell::default())
        };
        assert!(no(&[("at C:\\t\\sc", false), ("ratch\\a.exe", false)], 12));
        assert!(no(&[("at C:\\t\\scra", false), (" tch\\a.exe", false)], 12));
        assert!(no(&[("a sentence e", false), ("nds here", false)], 12));
        // A frame's edge is no path.
        assert!(no(
            &[
                ("\u{2502} C:\\t\\a \u{2502}", false),
                ("\u{2502} b \u{2502}", false)
            ],
            10
        ));
        // A quote is a break.
        assert!(no(&[("x \"C:\\t\\ab\"", false), ("\"cd", false)], 11));
    }

    #[test]
    fn char_at_cell_clamps() {
        let b = buf(&[("abcd", false), ("ef", true)]);
        let l = logical_line(&b, 4, 0);
        assert_eq!(char_at_cell(&l, 4, 1, 1), 5);
        assert_eq!(char_at_cell(&l, 4, 0, -3), 0);
        assert_eq!(char_at_cell(&l, 4, 9, 9), l.text.chars().count());
    }

    #[test]
    fn buffer_row_cells_reuses_one_scratch() {
        use std::cell::{Cell, RefCell};
        struct Counting {
            calls: Cell<usize>,
            addrs: RefCell<Vec<usize>>,
            text: &'static str,
        }
        impl LineLike for Counting {
            fn is_wrapped(&self) -> bool {
                false
            }
            fn load_cell(&self, x: usize, into: &mut ScratchCell) -> bool {
                self.calls.set(self.calls.get() + 1);
                self.addrs.borrow_mut().push(into as *const _ as usize);
                into.chars = self
                    .text
                    .chars()
                    .nth(x)
                    .map(|c| c.to_string())
                    .unwrap_or_default();
                true
            }
        }
        let line = Counting {
            calls: Cell::new(0),
            addrs: RefCell::new(Vec::new()),
            text: "ab",
        };
        let mut scratch = ScratchCell::default();
        let got = buffer_row_cells(Some(&line), 3, &mut scratch).unwrap();
        let expect = vec![
            CellInfo {
                chars: "a".into(),
                width: 1,
            },
            CellInfo {
                chars: "b".into(),
                width: 1,
            },
            CellInfo {
                chars: String::new(),
                width: 1,
            },
        ];
        assert_eq!(got, expect);
        assert_eq!(line.calls.get(), 3);
        let addrs = line.addrs.borrow();
        assert!(addrs.iter().all(|a| *a == addrs[0]));
    }
}
