//! Alt+click moves the cursor: xterm's `altClickMovesCursor`, on by default and left on by
//! Prism Terminal (orchestrator answer to owner question 5, 2026-10-17). A cell-for-cell port of xterm.js
//! `src/browser/input/MoveToCell.ts` (`moveToCellSequence`): on the normal screen only left and right
//! arrows, counted across rows; on the alternate screen up and down to the logical row, then left or
//! right by the characters between. The counts are xterm's own, including its quirks.

use crate::cells::CellInfo;

/// Hold time under which an Alt press and release is a click (xterm `ALT_CLICK_MOVE_CURSOR_TIME`).
pub const ALT_CLICK_MS: u64 = 500;

/// One screen row as `MoveToCell` reads it.
#[derive(Clone, Debug, Default)]
pub struct MoveRow {
    /// This row continues the row above (xterm `isWrapped`).
    pub wrapped: bool,
    pub cells: Vec<CellInfo>,
}

/// The screen the move is computed on: rows top to bottom, the cursor in screen coordinates.
pub struct MoveScreen<'a> {
    pub cols: usize,
    pub rows: &'a [MoveRow],
    /// False on the alternate screen (xterm `buffer.hasScrollback`).
    pub has_scrollback: bool,
    pub app_cursor: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dir {
    Up,
    Down,
    Right,
    Left,
}

fn seq(d: Dir, app: bool) -> &'static str {
    match (d, app) {
        (Dir::Up, false) => "\x1b[A",
        (Dir::Down, false) => "\x1b[B",
        (Dir::Right, false) => "\x1b[C",
        (Dir::Left, false) => "\x1b[D",
        (Dir::Up, true) => "\x1bOA",
        (Dir::Down, true) => "\x1bOB",
        (Dir::Right, true) => "\x1bOC",
        (Dir::Left, true) => "\x1bOD",
    }
}

fn repeat(n: i64, s: &str) -> String {
    s.repeat(n.max(0) as usize)
}

impl MoveScreen<'_> {
    fn wrapped(&self, row: i64) -> bool {
        row >= 0 && self.rows.get(row as usize).is_some_and(|r| r.wrapped)
    }

    /// xterm `wrappedRowsForRow`.
    fn wrapped_rows_for_row(&self, mut row: i64) -> i64 {
        let mut n = 0;
        let mut wraps = self.wrapped(row);
        while wraps && row >= 0 && row < self.rows.len() as i64 {
            n += 1;
            row -= 1;
            wraps = self.wrapped(row);
        }
        n
    }

    /// xterm `wrappedRowsCount`.
    fn wrapped_rows_count(&self, sy: i64, ty: i64) -> i64 {
        let start = sy - self.wrapped_rows_for_row(sy);
        let end = ty - self.wrapped_rows_for_row(ty);
        let d = if vertical(sy, ty) == Dir::Up { -1 } else { 1 };
        (0..(start - end).abs())
            .filter(|i| self.wrapped(start + d * i))
            .count() as i64
    }

    /// The UTF-16 length of xterm's `translateBufferLineToString(row, false, start, end)`.
    fn text_len(&self, row: i64, start: i64, end: i64) -> i64 {
        let Some(r) = usize::try_from(row).ok().and_then(|i| self.rows.get(i)) else {
            return 0;
        };
        let end = end.min(self.cols as i64);
        let mut x = start.max(0);
        let mut n = 0;
        while x < end {
            match r.cells.get(x as usize) {
                Some(c) if c.width > 0 => {
                    n += if c.chars.is_empty() {
                        1
                    } else {
                        c.chars.encode_utf16().count() as i64
                    };
                    x += i64::from(c.width);
                }
                _ => {
                    n += 1;
                    x += 1;
                }
            }
        }
        n
    }

    /// xterm `bufferLine(...).length`.
    fn buffer_line_len(
        &self,
        mut start_col: i64,
        start_row: i64,
        end_col: i64,
        end_row: i64,
        fwd: bool,
    ) -> i64 {
        let cols = self.cols as i64;
        let (mut col, mut row) = (start_col, start_row);
        let mut n = 0;
        while (col != end_col || row != end_row) && row >= 0 && row < self.rows.len() as i64 {
            col += if fwd { 1 } else { -1 };
            if fwd && col > cols - 1 {
                n += self.text_len(row, start_col, col);
                col = 0;
                start_col = 0;
                row += 1;
            } else if !fwd && col < 0 {
                n += self.text_len(row, 0, start_col + 1);
                col = cols - 1;
                start_col = col;
                row -= 1;
            }
        }
        n + self.text_len(row, start_col, col)
    }

    fn rows_to_move(&self, sy: i64, ty: i64) -> i64 {
        let start = sy - self.wrapped_rows_for_row(sy);
        let end = ty - self.wrapped_rows_for_row(ty);
        (start - end).abs() - self.wrapped_rows_count(sy, ty)
    }

    fn col_start_row(&self, sy: i64, ty: i64) -> i64 {
        if self.rows_to_move(sy, ty) > 0 {
            ty - self.wrapped_rows_for_row(ty)
        } else {
            sy
        }
    }
}

fn vertical(sy: i64, ty: i64) -> Dir {
    if sy > ty { Dir::Up } else { Dir::Down }
}

/// The arrow keys that move the cursor at `start` (col, row) to `target` (col, row), both in screen
/// coordinates (xterm `moveToCellSequence`).
pub fn move_to_cell_sequence(
    s: &MoveScreen,
    start: (usize, usize),
    target: (usize, usize),
) -> String {
    let (sx, sy) = (start.0 as i64, start.1 as i64);
    let (tx, ty) = (target.0 as i64, target.1 as i64);
    let cols = s.cols as i64;
    let app = s.app_cursor;
    if !s.has_scrollback {
        // resetStartingRow + moveToRequestedRow + moveToRequestedCol.
        let rows = s.rows_to_move(sy, ty);
        let mut out = String::new();
        if rows > 0 {
            let back = s.buffer_line_len(sx, sy, sx, sy - s.wrapped_rows_for_row(sy), false);
            out += &repeat(back, seq(Dir::Left, app));
        }
        out += &repeat(rows, seq(vertical(sy, ty), app));
        let start_row = s.col_start_row(sy, ty);
        let dir = if (sx < tx && start_row <= ty) || (sx >= tx && start_row < ty) {
            Dir::Right
        } else {
            Dir::Left
        };
        let n = s.buffer_line_len(sx, start_row, tx, ty, dir == Dir::Right);
        out += &repeat(n, seq(dir, app));
        return out;
    }
    if sy == ty {
        let dir = if sx > tx { Dir::Left } else { Dir::Right };
        return repeat((sx - tx).abs(), seq(dir, app));
    }
    let dir = if sy > ty { Dir::Left } else { Dir::Right };
    let row_diff = (sy - ty).abs();
    let from_end = cols - if sy > ty { tx } else { sx };
    let from_start = (if sy > ty { sx } else { tx }) - 1;
    repeat(
        from_end + (row_diff - 1) * cols + 1 + from_start,
        seq(dir, app),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(text: &str, cols: usize, wrapped: bool) -> MoveRow {
        let mut cells: Vec<CellInfo> = Vec::new();
        for ch in text.chars() {
            let wide = ch as u32 >= 0x1100 && unicode_wide(ch);
            cells.push(CellInfo {
                chars: ch.to_string(),
                width: if wide { 2 } else { 1 },
            });
            if wide {
                cells.push(CellInfo {
                    chars: String::new(),
                    width: 0,
                });
            }
        }
        while cells.len() < cols {
            cells.push(CellInfo {
                chars: String::new(),
                width: 1,
            });
        }
        MoveRow { wrapped, cells }
    }

    fn unicode_wide(ch: char) -> bool {
        matches!(ch as u32, 0x4E00..=0x9FFF)
    }

    fn normal(rows: &[MoveRow], cols: usize) -> MoveScreen<'_> {
        MoveScreen {
            cols,
            rows,
            has_scrollback: true,
            app_cursor: false,
        }
    }

    #[test]
    fn on_the_normal_screen_the_same_row_is_plain_left_or_right() {
        let rows = vec![row("abcdef", 10, false)];
        let s = normal(&rows, 10);
        assert_eq!(
            move_to_cell_sequence(&s, (5, 0), (2, 0)),
            "\x1b[D".repeat(3)
        );
        assert_eq!(
            move_to_cell_sequence(&s, (2, 0), (6, 0)),
            "\x1b[C".repeat(4)
        );
        assert_eq!(move_to_cell_sequence(&s, (4, 0), (4, 0)), "");
    }

    #[test]
    fn on_the_normal_screen_other_rows_count_cells_across_the_wrap() {
        let rows = vec![
            row("", 80, false),
            row("", 80, false),
            row("", 80, false),
            row("", 80, false),
        ];
        let s = normal(&rows, 80);
        // Up one row: (80 - 3) + 0 + 1 + (5 - 1) = 82 lefts, xterm's count.
        assert_eq!(
            move_to_cell_sequence(&s, (5, 3), (3, 2)),
            "\x1b[D".repeat(82)
        );
        // Down two rows: (80 - 5) + 80 + 1 + (3 - 1) = 158 rights.
        assert_eq!(
            move_to_cell_sequence(&s, (5, 1), (3, 3)),
            "\x1b[C".repeat(158)
        );
    }

    #[test]
    fn application_cursor_mode_sends_ss3_arrows() {
        let rows = vec![row("abc", 10, false)];
        let mut s = normal(&rows, 10);
        s.app_cursor = true;
        assert_eq!(move_to_cell_sequence(&s, (0, 0), (2, 0)), "\x1bOC\x1bOC");
    }

    #[test]
    fn on_the_alternate_screen_it_goes_up_or_down_then_across() {
        let rows = vec![
            row("first line", 20, false),
            row("", 20, false),
            row("third", 20, false),
        ];
        let s = MoveScreen {
            cols: 20,
            rows: &rows,
            has_scrollback: false,
            app_cursor: false,
        };
        // Same row: the characters between.
        assert_eq!(
            move_to_cell_sequence(&s, (1, 0), (4, 0)),
            "\x1b[C".repeat(3)
        );
        // Two rows down, then right from the start column to the target.
        assert_eq!(
            move_to_cell_sequence(&s, (1, 0), (3, 2)),
            format!("{}{}", "\x1b[B".repeat(2), "\x1b[C".repeat(2))
        );
        // xterm's quirk, kept for parity: `bufferLine` going left reads `translateToString(row,
        // start, end)` with start > end, which is "", so a move to the LEFT sends no arrow.
        assert_eq!(move_to_cell_sequence(&s, (4, 0), (1, 0)), "");
        assert_eq!(
            move_to_cell_sequence(&s, (5, 2), (2, 0)),
            "\x1b[A".repeat(2)
        );
    }

    /// xterm `MoveToCell.test.ts`, 'alt buffer': a 5 x 5 screen, the cursor at (3, 3).
    #[test]
    fn xterm_alt_buffer_moves_across_rows() {
        let rows: Vec<MoveRow> = (0..5).map(|_| row("", 5, false)).collect();
        let s = MoveScreen {
            cols: 5,
            rows: &rows,
            has_scrollback: false,
            app_cursor: false,
        };
        assert_eq!(move_to_cell_sequence(&s, (3, 3), (4, 4)), "\x1b[B\x1b[C");
    }

    /// xterm `MoveToCell.test.ts`, 'normal buffer': a 5 x 5 screen, the cursor at (3, 3); the
    /// expected count of arrows for every target, and the application cursor cases.
    #[test]
    fn xterm_normal_buffer_table() {
        let rows: Vec<MoveRow> = (0..5).map(|_| row("", 5, false)).collect();
        let mut s = normal(&rows, 5);
        // (target x, target y, arrows; negative = left).
        let table: [(usize, usize, i32); 24] = [
            (1, 3, -2),
            (2, 3, -1),
            (4, 3, 1),
            (5, 3, 2),
            (1, 1, -12),
            (2, 1, -11),
            (3, 1, -10),
            (4, 1, -9),
            (5, 1, -8),
            (1, 2, -7),
            (2, 2, -6),
            (3, 2, -5),
            (4, 2, -4),
            (5, 2, -3),
            (1, 4, 3),
            (2, 4, 4),
            (3, 4, 5),
            (4, 4, 6),
            (5, 4, 7),
            (1, 5, 8),
            (2, 5, 9),
            (3, 5, 10),
            (4, 5, 11),
            (5, 5, 12),
        ];
        for (x, y, n) in table {
            let want = if n < 0 {
                "\x1b[D".repeat((-n) as usize)
            } else {
                "\x1b[C".repeat(n as usize)
            };
            assert_eq!(
                move_to_cell_sequence(&s, (3, 3), (x, y)),
                want,
                "target ({x}, {y})"
            );
        }
        s.app_cursor = true;
        assert_eq!(
            move_to_cell_sequence(&s, (3, 3), (3, 1)),
            "\x1bOD".repeat(10)
        );
        assert_eq!(
            move_to_cell_sequence(&s, (3, 3), (3, 2)),
            "\x1bOD".repeat(5)
        );
        assert_eq!(move_to_cell_sequence(&s, (3, 3), (2, 3)), "\x1bOD");
        assert_eq!(move_to_cell_sequence(&s, (3, 3), (4, 3)), "\x1bOC");
        assert_eq!(
            move_to_cell_sequence(&s, (3, 3), (3, 4)),
            "\x1bOC".repeat(5)
        );
        assert_eq!(
            move_to_cell_sequence(&s, (3, 3), (3, 5)),
            "\x1bOC".repeat(10)
        );
    }

    #[test]
    fn on_the_alternate_screen_a_wrapped_row_is_not_a_row_to_move() {
        // Row 1 continues row 0: rows 0 and 1 are one logical line, so row 2 is one Down away.
        let rows = vec![
            row("aaaaaaaaaa", 10, false),
            row("bbb", 10, true),
            row("ccc", 10, false),
        ];
        let s = MoveScreen {
            cols: 10,
            rows: &rows,
            has_scrollback: false,
            app_cursor: false,
        };
        let out = move_to_cell_sequence(&s, (2, 0), (2, 2));
        assert_eq!(out, "\x1b[B");
    }

    #[test]
    fn a_wide_character_counts_once_on_the_alternate_screen() {
        // The two cells of a wide character are one character to the shell: one arrow.
        let rows = vec![row("a\u{4E2D}b", 10, false)];
        let s = MoveScreen {
            cols: 10,
            rows: &rows,
            has_scrollback: false,
            app_cursor: false,
        };
        assert_eq!(
            move_to_cell_sequence(&s, (0, 0), (3, 0)),
            "\x1b[C".repeat(2)
        );
    }
}
