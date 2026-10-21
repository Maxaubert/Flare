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
}

pub fn logical_line(b: &dyn BufferLike, cols: usize, y: usize) -> LogicalLine {
    let mut first = y;
    while first > 0 && b.line(first).is_some_and(LineLike::is_wrapped) {
        first -= 1;
    }
    let mut last = y;
    while last + 1 < b.len() && b.line(last + 1).is_some_and(LineLike::is_wrapped) {
        last += 1;
    }
    let mut scratch = ScratchCell::default();
    let mut cells = Vec::new();
    for i in first..=last {
        cells.extend(buffer_row_cells(b.line(i), cols, &mut scratch).unwrap_or_default());
    }
    let widths = cells.iter().map(|c| c.width).collect();
    let CellText {
        text,
        index,
        text_end,
    } = cell_text(&cells);
    LogicalLine {
        first,
        last,
        widths,
        text,
        index,
        text_end,
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
