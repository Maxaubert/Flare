//! The static sample grid: every colour, flag, wide glyph and box character a painter must draw.

use crate::color::Color;
use crate::grid::{CellFlags, Cursor, Grid};

/// Builds the M1 sample grid. Rows at or past `rows` are not written; a grid shorter than
/// 15 rows shows no cursor.
pub fn sample_grid(cols: usize, rows: usize) -> Grid {
    let mut g = Grid::new(cols, rows);
    let rows = g.rows();
    let cols = g.cols();
    let none = CellFlags::empty();
    let fg = Color::DefaultFg;
    let bg = Color::DefaultBg;

    if 0 < rows {
        g.put_str(0, 0, "Flare M1 sample", fg, bg, CellFlags::BOLD);
    }

    if 1 < rows {
        for i in 0..16usize {
            g.put_str(
                i * 3,
                1,
                &format!("{i:>2} "),
                fg,
                Color::Indexed(i as u8),
                none,
            );
        }
    }

    if 2 < rows {
        for i in 0..16usize {
            g.put_str(i * 3, 2, "abc", Color::Indexed(i as u8), bg, none);
        }
    }

    for k in 0..240usize {
        let row = 3 + k / 60;
        if row < rows {
            g.put_str(k % 60, row, " ", fg, Color::Indexed((16 + k) as u8), none);
        }
    }

    if 7 < rows {
        for i in 0..64usize {
            let colour = Color::Rgb((i * 4) as u8, 128, (255 - i * 4) as u8);
            g.put_str(i, 7, " ", fg, colour, none);
        }
    }

    if 8 < rows {
        let mut c = g.put_str(0, 8, "bold", fg, bg, CellFlags::BOLD) + 1;
        c = g.put_str(c, 8, "italic", fg, bg, CellFlags::ITALIC) + 1;
        c = g.put_str(c, 8, "underline", fg, bg, CellFlags::UNDERLINE) + 1;
        let all = CellFlags::BOLD
            .union(CellFlags::ITALIC)
            .union(CellFlags::UNDERLINE);
        c = g.put_str(c, 8, "bold italic underline", fg, bg, all) + 1;
        g.put_str(c, 8, "inverse", fg, bg, CellFlags::INVERSE);
    }

    if 9 < rows {
        g.put_str(0, 9, "CJK: 中文字符 日本語  emoji: 😀 🚀 ✅", fg, bg, none);
    }

    let box_lines = [
        "┌──────┬──────┐",
        "│ box  │ draw │",
        "├──────┼──────┤",
        "└──────┴──────┘",
    ];
    for (k, line) in box_lines.iter().enumerate() {
        let row = 10 + k;
        if row < rows {
            let end = g.put_str(0, row, line, fg, bg, none);
            if row == 10 {
                g.put_str(end, row, "  ▀▄█░▒▓", fg, bg, none);
            }
        }
    }

    if 14 < rows {
        let prompt_end = g.put_str(0, 14, "PS C:\\Users\\flare> ", fg, bg, none);
        g.cursor = Cursor {
            col: prompt_end.min(cols - 1),
            row: 14,
            visible: true,
        };
    } else {
        g.cursor.visible = false;
    }

    g
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_one_has_ansi_backgrounds() {
        let g = sample_grid(80, 16);
        assert_eq!(g.cell(3, 1).bg, Color::Indexed(1));
    }

    #[test]
    fn cube_wraps_at_60() {
        let g = sample_grid(80, 16);
        assert_eq!(g.cell(0, 4).bg, Color::Indexed(76));
    }

    #[test]
    fn truecolour_row() {
        let g = sample_grid(80, 16);
        assert_eq!(g.cell(1, 7).bg, Color::Rgb(4, 128, 251));
    }

    #[test]
    fn cjk_is_wide() {
        let g = sample_grid(80, 16);
        assert!(g.cell(5, 9).flags.contains(CellFlags::WIDE));
        assert_eq!(g.cell(5, 9).ch, '中');
        assert!(g.cell(6, 9).flags.contains(CellFlags::WIDE_SPACER));
    }

    #[test]
    fn cursor_after_prompt() {
        let g = sample_grid(80, 16);
        assert_eq!(g.cursor.col, 19);
        assert_eq!(g.cursor.row, 14);
        assert!(g.cursor.visible);
    }

    #[test]
    fn tiny_grid_does_not_panic() {
        let g = sample_grid(1, 1);
        assert!(!g.cursor.visible);
    }

    #[test]
    fn narrow_grid_truncates() {
        let g = sample_grid(10, 16);
        assert_eq!(g.cell(9, 3).bg, Color::Indexed(25));
    }
}
