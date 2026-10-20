//! The terminal grid model: cells with a base character, combining marks, colours and flags.

use crate::color::Color;
use unicode_width::UnicodeWidthChar;

/// Combining marks one cell keeps. A longer run is output nobody reads (a stack of 16 is already
/// unreadable); unbounded, each new mark made a longer glyph key to shape and cache (audit M1).
pub const MAX_MARKS: usize = 16;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct CellFlags(pub u8);

impl CellFlags {
    pub const BOLD: Self = Self(1);
    pub const ITALIC: Self = Self(2);
    pub const UNDERLINE: Self = Self(4);
    pub const WIDE: Self = Self(8);
    pub const WIDE_SPACER: Self = Self(16);
    pub const INVERSE: Self = Self(32);
    pub const DIM: Self = Self(64);
    pub const STRIKE: Self = Self(128);

    pub const fn empty() -> Self {
        Self(0)
    }

    pub fn contains(self, o: Self) -> bool {
        self.0 & o.0 == o.0
    }

    pub fn insert(&mut self, o: Self) {
        self.0 |= o.0;
    }

    pub fn union(self, o: Self) -> Self {
        Self(self.0 | o.0)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub ch: char,
    pub combining: Option<Box<[char]>>,
    pub fg: Color,
    pub bg: Color,
    pub flags: CellFlags,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            combining: None,
            fg: Color::DefaultFg,
            bg: Color::DefaultBg,
            flags: CellFlags::empty(),
        }
    }
}

impl Cell {
    /// The base character followed by its combining marks.
    pub fn cluster(&self) -> String {
        let mut s = String::new();
        s.push(self.ch);
        if let Some(marks) = &self.combining {
            s.extend(marks.iter().take(MAX_MARKS));
        }
        s
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub col: usize,
    pub row: usize,
    pub visible: bool,
    /// Drawn as an outline, not a filled block: the terminal has lost the keyboard to a popup or a
    /// field (PT's xterm `cursorInactiveStyle` outline). The glyph keeps its own colours.
    pub hollow: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    cols: usize,
    rows: usize,
    cells: Vec<Cell>,
    pub cursor: Cursor,
}

impl Grid {
    /// Bytes the cell storage holds.
    pub fn bytes(&self) -> usize {
        self.cells.capacity() * size_of::<Cell>()
    }

    /// Creates a grid of default cells. `cols` and `rows` are clamped to at least 1.
    pub fn new(cols: usize, rows: usize) -> Self {
        let cols = cols.max(1);
        let rows = rows.max(1);
        Self {
            cols,
            rows,
            cells: vec![Cell::default(); cols * rows],
            cursor: Cursor {
                col: 0,
                row: 0,
                visible: true,
                hollow: false,
            },
        }
    }

    /// Makes the grid `cols` x `rows` (each at least 1), keeping the cell storage when the size is
    /// the same so a caller that refills the grid every frame allocates nothing. The cells keep
    /// whatever they held; the caller overwrites every one.
    pub fn reshape(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if (cols, rows) == (self.cols, self.rows) {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        self.cells.clear();
        self.cells.resize(cols * rows, Cell::default());
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    fn index(&self, col: usize, row: usize) -> usize {
        assert!(
            col < self.cols && row < self.rows,
            "cell ({col},{row}) out of range"
        );
        row * self.cols + col
    }

    /// Panics when `col` or `row` is out of range.
    pub fn cell(&self, col: usize, row: usize) -> &Cell {
        &self.cells[self.index(col, row)]
    }

    /// Panics when `col` or `row` is out of range.
    pub fn cell_mut(&mut self, col: usize, row: usize) -> &mut Cell {
        let i = self.index(col, row);
        &mut self.cells[i]
    }

    /// Writes `text` from (col,row) on one row; returns the column after the last cell written.
    /// Width 0 appends to the previous cell's combining marks, width 1 takes one cell, width 2 takes
    /// a WIDE cell plus a WIDE_SPACER cell after it (stops if it would not fit). Stops at the row end.
    /// Control characters are skipped. A `col` at or past the row end writes nothing.
    pub fn put_str(
        &mut self,
        col: usize,
        row: usize,
        text: &str,
        fg: Color,
        bg: Color,
        flags: CellFlags,
    ) -> usize {
        assert!(row < self.rows, "row {row} out of range");
        if col >= self.cols {
            return col;
        }
        let spacer_flags = CellFlags(flags.0 & !CellFlags::WIDE.0).union(CellFlags::WIDE_SPACER);
        let mut c = col;
        // Column of the last base cell this call wrote; combining marks attach there.
        let mut last: Option<usize> = None;
        for ch in text.chars() {
            match UnicodeWidthChar::width(ch) {
                None => continue,
                Some(0) => {
                    if let Some(lc) = last {
                        let cell = self.cell_mut(lc, row);
                        let mut marks: Vec<char> = cell
                            .combining
                            .take()
                            .map(|b| b.into_vec())
                            .unwrap_or_default();
                        if marks.len() < MAX_MARKS {
                            marks.push(ch);
                        }
                        cell.combining = Some(marks.into_boxed_slice());
                    }
                }
                Some(1) => {
                    if c >= self.cols {
                        break;
                    }
                    *self.cell_mut(c, row) = Cell {
                        ch,
                        combining: None,
                        fg,
                        bg,
                        flags,
                    };
                    last = Some(c);
                    c += 1;
                }
                Some(_) => {
                    if c + 1 >= self.cols {
                        break;
                    }
                    *self.cell_mut(c, row) = Cell {
                        ch,
                        combining: None,
                        fg,
                        bg,
                        flags: flags.union(CellFlags::WIDE),
                    };
                    *self.cell_mut(c + 1, row) = Cell {
                        ch: ' ',
                        combining: None,
                        fg,
                        bg,
                        flags: spacer_flags,
                    };
                    last = Some(c);
                    c += 2;
                }
            }
        }
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(g: &mut Grid, col: usize, row: usize, text: &str) -> usize {
        g.put_str(
            col,
            row,
            text,
            Color::DefaultFg,
            Color::DefaultBg,
            CellFlags::empty(),
        )
    }

    #[test]
    fn new_clamps_to_one() {
        let g = Grid::new(0, 0);
        assert_eq!(g.cols(), 1);
        assert_eq!(g.rows(), 1);
    }

    #[test]
    fn new_cursor_is_visible_at_origin() {
        let g = Grid::new(4, 2);
        assert_eq!(
            g.cursor,
            Cursor {
                col: 0,
                row: 0,
                visible: true,
                hollow: false
            }
        );
        assert_eq!(g.cell(3, 1).ch, ' ');
    }

    #[test]
    fn put_ascii() {
        let mut g = Grid::new(4, 1);
        assert_eq!(put(&mut g, 0, 0, "ab"), 2);
        assert_eq!(g.cell(1, 0).ch, 'b');
    }

    #[test]
    fn put_wide_sets_spacer() {
        let mut g = Grid::new(4, 1);
        assert_eq!(put(&mut g, 0, 0, "中"), 2);
        assert!(g.cell(0, 0).flags.contains(CellFlags::WIDE));
        assert!(g.cell(1, 0).flags.contains(CellFlags::WIDE_SPACER));
        assert!(!g.cell(1, 0).flags.contains(CellFlags::WIDE));
    }

    #[test]
    fn wide_does_not_split_at_row_end() {
        let mut g = Grid::new(3, 1);
        assert_eq!(put(&mut g, 2, 0, "中"), 2);
        assert_eq!(g.cell(2, 0).ch, ' ');
    }

    #[test]
    fn combining_mark_joins_previous_cell() {
        let mut g = Grid::new(4, 1);
        assert_eq!(put(&mut g, 0, 0, "e\u{301}x"), 2);
        assert_eq!(g.cell(0, 0).cluster(), "e\u{301}");
        assert_eq!(g.cell(1, 0).ch, 'x');
    }

    #[test]
    fn marks_beyond_the_cap_are_dropped_in_put_str_and_cluster() {
        let mut g = Grid::new(4, 1);
        let text = format!("e{}", "\u{301}".repeat(MAX_MARKS + 50));
        put(&mut g, 0, 0, &text);
        assert_eq!(
            g.cell(0, 0).combining.as_ref().map(|m| m.len()),
            Some(MAX_MARKS)
        );
        assert_eq!(g.cell(0, 0).cluster().chars().count(), 1 + MAX_MARKS);
        // A cell built elsewhere with too many marks still draws a bounded cluster.
        let wild = Cell {
            ch: 'e',
            combining: Some(vec!['\u{301}'; 5000].into_boxed_slice()),
            ..Cell::default()
        };
        assert_eq!(wild.cluster().chars().count(), 1 + MAX_MARKS);
    }

    #[test]
    fn stops_at_row_end() {
        let mut g = Grid::new(4, 1);
        assert_eq!(put(&mut g, 2, 0, "abcdef"), 4);
    }

    #[test]
    fn start_past_row_end_is_noop() {
        let mut g = Grid::new(4, 1);
        let before = g.clone();
        assert_eq!(put(&mut g, 7, 0, "a"), 7);
        assert_eq!(g, before);
    }

    #[test]
    fn emoji_is_wide() {
        let mut g = Grid::new(4, 1);
        assert_eq!(put(&mut g, 0, 0, "😀"), 2);
    }

    #[test]
    fn mark_after_wide_joins_the_wide_cell() {
        let mut g = Grid::new(4, 1);
        assert_eq!(put(&mut g, 0, 0, "中\u{301}"), 2);
        assert_eq!(g.cell(0, 0).cluster(), "中\u{301}");
        assert_eq!(g.cell(1, 0).combining, None);
    }

    #[test]
    fn leading_mark_is_dropped() {
        let mut g = Grid::new(4, 1);
        assert_eq!(put(&mut g, 0, 0, "\u{301}a"), 1);
        assert_eq!(g.cell(0, 0).cluster(), "a");
    }

    #[test]
    fn control_chars_skipped() {
        let mut g = Grid::new(4, 1);
        assert_eq!(put(&mut g, 0, 0, "a\u{7}b"), 2);
        assert_eq!(g.cell(1, 0).ch, 'b');
    }

    #[test]
    fn flags_bitops() {
        assert!(
            CellFlags::BOLD
                .union(CellFlags::ITALIC)
                .contains(CellFlags::ITALIC)
        );
        assert!(!CellFlags::BOLD.contains(CellFlags::ITALIC));
    }

    #[test]
    fn put_str_applies_colours_and_flags() {
        let mut g = Grid::new(4, 1);
        g.put_str(
            0,
            0,
            "中",
            Color::Indexed(2),
            Color::Indexed(3),
            CellFlags::BOLD,
        );
        let wide = g.cell(0, 0);
        assert_eq!(wide.fg, Color::Indexed(2));
        assert_eq!(wide.bg, Color::Indexed(3));
        assert!(wide.flags.contains(CellFlags::BOLD));
        assert!(g.cell(1, 0).flags.contains(CellFlags::BOLD));
    }
}
