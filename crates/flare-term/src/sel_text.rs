//! The text of a selection, cell by cell (rule 21: one cell-to-text function).

use crate::cells::{buffer_row_cells, BufferLike, CellInfo, LineLike, ScratchCell};

/// The text of a selection. `start` and `end` are boundaries `(row, b)` with `start <= end` in reading order
/// (the caller orders them); a boundary at `b == cols` means the row end. Normal: first row from `start.b`
/// (to `end.b` when it is the same row), middle rows whole, last row `0..end.b`; each row trimmed on the right
/// (empty cells), a row that continues the one above (`is_wrapped`) is appended to it without a break, U+00A0
/// becomes a space, rows are joined with "\r\n". A wide character whose spacer is the first or last included
/// cell is included whole. Block: for each row the columns `min(start.b, end.b) .. max(..)`, "" when they are equal.
pub fn selection_text(
    b: &dyn BufferLike,
    cols: usize,
    start: (usize, usize),
    end: (usize, usize),
    block: bool,
) -> String {
    let mut scratch = ScratchCell::default();
    let mut out = String::new();
    if block {
        let from = start.1.min(end.1);
        let to = start.1.max(end.1);
        if from == to {
            return out;
        }
        for y in start.0..=end.0 {
            if y > start.0 {
                out.push_str("\r\n");
            }
            let cells = buffer_row_cells(b.line(y), cols, &mut scratch).unwrap_or_default();
            out.push_str(&span_text(&cells, from, to));
        }
        return out;
    }
    if start >= end {
        return out;
    }
    for y in start.0..=end.0 {
        let from = if y == start.0 { start.1 } else { 0 };
        let to = if y == end.0 { end.1 } else { cols };
        if y > start.0 && !b.line(y).is_some_and(LineLike::is_wrapped) {
            out.push_str("\r\n");
        }
        let cells = buffer_row_cells(b.line(y), cols, &mut scratch).unwrap_or_default();
        out.push_str(&span_text(&cells, from, to));
    }
    out
}

/// The text of the cells `from..to` of one row: a wide character cut at either edge is taken whole, empty
/// cells between text are spaces, trailing empty cells are dropped, U+00A0 becomes a space.
fn span_text(cells: &[CellInfo], from: usize, to: usize) -> String {
    let to = to.min(cells.len());
    if from >= to {
        return String::new();
    }
    let mut from = from;
    if cells[from].width == 0 && from > 0 {
        from -= 1;
    }
    let mut to = to;
    if cells[to - 1].width == 2 && cells.get(to).is_some_and(|c| c.width == 0) {
        to += 1;
    }
    let mut out = String::new();
    let mut keep = 0;
    for c in &cells[from..to] {
        if c.width == 0 {
            continue;
        }
        if c.chars.is_empty() {
            out.push(' ');
        } else {
            for ch in c.chars.chars() {
                out.push(if ch == '\u{a0}' { ' ' } else { ch });
            }
            keep = out.len();
        }
    }
    out.truncate(keep);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row of cells and its wrap flag; a missing cell reads as nothing.
    struct Row {
        cells: Vec<CellInfo>,
        wrapped: bool,
    }

    impl LineLike for Row {
        fn is_wrapped(&self) -> bool {
            self.wrapped
        }
        fn load_cell(&self, x: usize, into: &mut ScratchCell) -> bool {
            match self.cells.get(x) {
                Some(c) => {
                    into.chars = c.chars.clone();
                    into.width = c.width;
                    true
                }
                None => false,
            }
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

    fn cell(chars: &str, width: u8) -> CellInfo {
        CellInfo {
            chars: chars.to_string(),
            width,
        }
    }

    /// A row of one width-1 cell per char, padded with empty cells to `cols`.
    fn row(s: &str, cols: usize, wrapped: bool) -> Row {
        let mut cells: Vec<CellInfo> = s.chars().map(|c| cell(&c.to_string(), 1)).collect();
        while cells.len() < cols {
            cells.push(cell("", 1));
        }
        Row { cells, wrapped }
    }

    fn buf(rows: Vec<Row>) -> Buf {
        Buf { rows }
    }

    #[test]
    fn one_row_part_of_a_line() {
        let b = buf(vec![row("hello world", 11, false)]);
        assert_eq!(selection_text(&b, 11, (0, 2), (0, 7), false), "llo w");
    }

    #[test]
    fn trailing_blanks_are_trimmed() {
        let b = buf(vec![row("ab", 10, false)]);
        assert_eq!(selection_text(&b, 10, (0, 0), (0, 10), false), "ab");
    }

    #[test]
    fn rows_are_joined_with_crlf() {
        let b = buf(vec![
            row("one", 5, false),
            row("two", 5, false),
            row("three", 5, false),
        ]);
        assert_eq!(
            selection_text(&b, 5, (0, 1), (2, 3), false),
            "ne\r\ntwo\r\nthr"
        );
    }

    #[test]
    fn a_wrapped_row_is_joined_without_a_break() {
        let b = buf(vec![row("abcde", 5, false), row("fgh", 5, true)]);
        assert_eq!(selection_text(&b, 5, (0, 0), (1, 3), false), "abcdefgh");
    }

    #[test]
    fn a_selection_ending_at_the_row_end_includes_the_whole_row() {
        let b = buf(vec![row("abc", 5, false)]);
        assert_eq!(selection_text(&b, 5, (0, 0), (0, 5), false), "abc");
    }

    #[test]
    fn non_breaking_spaces_become_spaces() {
        let b = buf(vec![row("a\u{a0}b", 3, false)]);
        assert_eq!(selection_text(&b, 3, (0, 0), (0, 3), false), "a b");
    }

    #[test]
    fn an_empty_range_is_empty() {
        let b = buf(vec![row("abcd", 4, false)]);
        assert_eq!(selection_text(&b, 4, (0, 2), (0, 2), false), "");
    }

    #[test]
    fn a_block_takes_the_same_columns_of_every_row() {
        let b = buf(vec![row("abcdef", 6, false), row("ghijkl", 6, false)]);
        assert_eq!(selection_text(&b, 6, (0, 2), (1, 4), true), "cd\r\nij");
    }

    #[test]
    fn a_block_of_zero_width_is_empty() {
        let b = buf(vec![row("abcdef", 6, false), row("ghijkl", 6, false)]);
        assert_eq!(selection_text(&b, 6, (0, 2), (1, 2), true), "");
    }

    #[test]
    fn a_wide_character_is_included_whole() {
        // 日 (lead, spacer), 本 (lead, spacer), x
        let b = buf(vec![Row {
            cells: vec![
                cell("日", 2),
                cell("", 0),
                cell("本", 2),
                cell("", 0),
                cell("x", 1),
            ],
            wrapped: false,
        }]);
        assert_eq!(selection_text(&b, 5, (0, 1), (0, 3), false), "日本");
    }

    #[test]
    fn emoji_and_combining_text_survive() {
        let s = "e\u{301}\u{1F600}x";
        let b = buf(vec![row(s, 4, false)]);
        assert_eq!(selection_text(&b, 4, (0, 0), (0, 4), false), s);
    }
}
