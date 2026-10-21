//! snapshot_grid: alacritty Grid to flare_render Grid.
//!
//! The viewport (the rows on screen, shifted by the display offset so a scrolled-back view shows
//! history) is copied into a `flare_render::grid::Grid` of the same size, so both renderers paint
//! the live tab from one model. Text positions are grid columns; wide characters keep their
//! spacer cell (rule 21).

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::{Cell as TCell, Flags};
use alacritty_terminal::term::{Term, TermMode, point_to_viewport};
use alacritty_terminal::vte::ansi::{Color as AColor, NamedColor};
use flare_render::color::Color as FColor;
use flare_render::grid::{Cell as FCell, CellFlags, Cursor as FCursor, Grid};

/// The viewport as a `flare_render` grid of the same size, with the cursor placed in it.
pub fn snapshot_grid<T: EventListener>(term: &Term<T>) -> Grid {
    let cols = term.columns();
    let rows = term.screen_lines();
    let mut out = Grid::new(cols, rows);
    let grid = term.grid();
    let offset = grid.display_offset();
    for row in 0..rows {
        // Viewport row 0 is the top of the view; a scrolled-back view reads older lines.
        let line = Line(row as i32 - offset as i32);
        for col in 0..cols {
            *out.cell_mut(col, row) = map_cell(&grid[Point::new(line, Column(col))]);
        }
    }
    let point = grid.cursor.point;
    let shown = term.mode().contains(TermMode::SHOW_CURSOR);
    out.cursor = match point_to_viewport(offset, point) {
        Some(p) if p.line < rows => FCursor {
            col: p.column.0,
            row: p.line,
            visible: shown,
        },
        _ => FCursor {
            col: point.column.0,
            row: 0,
            visible: false,
        },
    };
    out
}

fn map_cell(src: &TCell) -> FCell {
    let ch = if src
        .flags
        .intersects(Flags::HIDDEN | Flags::LEADING_WIDE_CHAR_SPACER)
    {
        ' '
    } else {
        src.c
    };
    let (fg, fg_dim) = map_color(src.fg);
    let (bg, _) = map_color(src.bg);
    let mut flags = map_flags(src.flags);
    if fg_dim {
        flags.insert(CellFlags::DIM);
    }
    FCell {
        ch,
        combining: src.zerowidth().map(|m| m.to_vec().into_boxed_slice()),
        fg,
        bg,
        flags,
    }
}

fn map_flags(f: Flags) -> CellFlags {
    let mut out = CellFlags::empty();
    let pairs = [
        (Flags::BOLD, CellFlags::BOLD),
        (Flags::ITALIC, CellFlags::ITALIC),
        (Flags::INVERSE, CellFlags::INVERSE),
        (Flags::WIDE_CHAR, CellFlags::WIDE),
        (Flags::WIDE_CHAR_SPACER, CellFlags::WIDE_SPACER),
        (Flags::DIM, CellFlags::DIM),
        (Flags::STRIKEOUT, CellFlags::STRIKE),
    ];
    for (from, to) in pairs {
        if f.contains(from) {
            out.insert(to);
        }
    }
    if f.intersects(Flags::ALL_UNDERLINES) {
        out.insert(CellFlags::UNDERLINE);
    }
    out
}

/// Maps an alacritty colour to a renderer colour; the bool is true for a dim colour.
pub fn map_color(c: AColor) -> (FColor, bool) {
    match c {
        AColor::Spec(rgb) => (FColor::Rgb(rgb.r, rgb.g, rgb.b), false),
        AColor::Indexed(i) => (FColor::Indexed(i), false),
        AColor::Named(n) => map_named(n),
    }
}

fn map_named(n: NamedColor) -> (FColor, bool) {
    let v = n as usize;
    match n {
        NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::Cursor => {
            (FColor::DefaultFg, false)
        }
        NamedColor::DimForeground => (FColor::DefaultFg, true),
        NamedColor::Background => (FColor::DefaultBg, false),
        // Black..White are 0..=7 and BrightBlack..BrightWhite are 8..=15.
        _ if v <= 15 => (FColor::Indexed(v as u8), false),
        // DimBlack..DimWhite: the normal palette entry, drawn dim.
        _ if (259..=266).contains(&v) => (FColor::Indexed((v - 259) as u8), true),
        _ => (FColor::DefaultFg, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::grid::Scroll;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::{Processor, Rgb, StdSyncHandler};

    struct Dims {
        cols: usize,
        rows: usize,
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

    fn term(cols: usize, rows: usize, bytes: &[u8]) -> Term<VoidListener> {
        let mut t = Term::new(Config::default(), &Dims { cols, rows }, VoidListener);
        Processor::<StdSyncHandler>::new().advance(&mut t, bytes);
        t
    }

    #[test]
    fn plain_ascii_row() {
        let t = term(10, 3, b"ab");
        let g = snapshot_grid(&t);
        assert_eq!(g.cell(0, 0).ch, 'a');
        assert_eq!(g.cell(1, 0).ch, 'b');
        assert_eq!(g.cell(2, 0).ch, ' ');
        assert_eq!(
            g.cursor,
            flare_render::grid::Cursor {
                col: 2,
                row: 0,
                visible: true
            }
        );
    }

    #[test]
    fn named_indexed_and_truecolour() {
        let t = term(
            10,
            1,
            b"\x1b[31mR\x1b[0m\x1b[38;5;196mI\x1b[0m\x1b[38;2;1;2;3mT\x1b[0m\x1b[44mB",
        );
        let g = snapshot_grid(&t);
        assert_eq!(g.cell(0, 0).fg, FColor::Indexed(1));
        assert_eq!(g.cell(1, 0).fg, FColor::Indexed(196));
        assert_eq!(g.cell(2, 0).fg, FColor::Rgb(1, 2, 3));
        assert_eq!(g.cell(3, 0).bg, FColor::Indexed(4));
        assert_eq!(g.cell(9, 0).fg, FColor::DefaultFg);
        assert_eq!(g.cell(9, 0).bg, FColor::DefaultBg);
    }

    #[test]
    fn bright_named_colours() {
        let t = term(4, 1, b"\x1b[91mX\x1b[97mY");
        let g = snapshot_grid(&t);
        assert_eq!(g.cell(0, 0).fg, FColor::Indexed(9));
        assert_eq!(g.cell(1, 0).fg, FColor::Indexed(15));
    }

    #[test]
    fn flags_bold_italic_underline_inverse_strike_dim() {
        let t = term(
            10,
            1,
            b"\x1b[1mB\x1b[0m\x1b[3mI\x1b[0m\x1b[4mU\x1b[0m\x1b[7mV\x1b[0m\x1b[9mS\x1b[0m\x1b[2mD",
        );
        let g = snapshot_grid(&t);
        let expect = [
            CellFlags::BOLD,
            CellFlags::ITALIC,
            CellFlags::UNDERLINE,
            CellFlags::INVERSE,
            CellFlags::STRIKE,
            CellFlags::DIM,
        ];
        let all = [
            CellFlags::BOLD,
            CellFlags::ITALIC,
            CellFlags::UNDERLINE,
            CellFlags::INVERSE,
            CellFlags::STRIKE,
            CellFlags::DIM,
        ];
        for (i, want) in expect.iter().enumerate() {
            let got = g.cell(i, 0).flags;
            assert!(got.contains(*want), "cell {i} lacks its flag");
            for other in all.iter().filter(|o| o.0 != want.0) {
                assert!(!got.contains(*other), "cell {i} has an extra flag");
            }
        }
    }

    #[test]
    fn double_underline_counts_as_underline() {
        let t = term(4, 1, b"\x1b[4:2mX");
        let g = snapshot_grid(&t);
        assert!(g.cell(0, 0).flags.contains(CellFlags::UNDERLINE));
    }

    #[test]
    fn sgr_21_is_cancel_bold_not_double_underline() {
        // vte 0.15 reads SGR 21 as "bold off" (ECMA-48), so it never reaches the grid as an underline.
        let t = term(4, 1, b"\x1b[1m\x1b[21mX");
        let g = snapshot_grid(&t);
        assert!(!g.cell(0, 0).flags.contains(CellFlags::BOLD));
        assert!(!g.cell(0, 0).flags.contains(CellFlags::UNDERLINE));
    }

    #[test]
    fn wide_char_and_spacer() {
        let t = term(4, 1, "中".as_bytes());
        let g = snapshot_grid(&t);
        assert_eq!(g.cell(0, 0).ch, '中');
        assert!(g.cell(0, 0).flags.contains(CellFlags::WIDE));
        assert!(g.cell(1, 0).flags.contains(CellFlags::WIDE_SPACER));
        assert!(!g.cell(1, 0).flags.contains(CellFlags::WIDE));
    }

    #[test]
    fn combining_marks_are_kept() {
        let t = term(4, 1, "e\u{301}x".as_bytes());
        let g = snapshot_grid(&t);
        assert_eq!(g.cell(0, 0).cluster(), "e\u{301}");
        assert_eq!(g.cell(1, 0).ch, 'x');
    }

    #[test]
    fn hidden_cell_is_blank() {
        let t = term(4, 1, b"\x1b[8mX");
        let g = snapshot_grid(&t);
        assert_eq!(g.cell(0, 0).ch, ' ');
    }

    #[test]
    fn cursor_hidden_by_dectcem() {
        let t = term(4, 1, b"\x1b[?25l");
        assert!(!snapshot_grid(&t).cursor.visible);
        let t = term(4, 1, b"\x1b[?25l\x1b[?25h");
        assert!(snapshot_grid(&t).cursor.visible);
    }

    #[test]
    fn scrolled_back_view_shows_history() {
        let mut t = term(5, 2, b"1\r\n2\r\n3\r\n4");
        t.scroll_display(Scroll::Delta(1));
        let g = snapshot_grid(&t);
        assert_eq!(g.cell(0, 0).ch, '2');
        assert_eq!(g.cell(0, 1).ch, '3');
        assert!(!g.cursor.visible);
        t.scroll_display(Scroll::Bottom);
        let g = snapshot_grid(&t);
        assert_eq!(g.cell(0, 0).ch, '3');
        assert_eq!(g.cell(0, 1).ch, '4');
        assert_eq!(
            g.cursor,
            flare_render::grid::Cursor {
                col: 1,
                row: 1,
                visible: true
            }
        );
    }

    #[test]
    fn map_color_table() {
        assert_eq!(
            map_color(AColor::Named(NamedColor::DimRed)),
            (FColor::Indexed(1), true)
        );
        assert_eq!(
            map_color(AColor::Named(NamedColor::Foreground)),
            (FColor::DefaultFg, false)
        );
        assert_eq!(
            map_color(AColor::Named(NamedColor::Background)),
            (FColor::DefaultBg, false)
        );
        assert_eq!(
            map_color(AColor::Named(NamedColor::BrightForeground)).0,
            FColor::DefaultFg
        );
        assert_eq!(
            map_color(AColor::Spec(Rgb { r: 9, g: 8, b: 7 })),
            (FColor::Rgb(9, 8, 7), false)
        );
    }

    #[test]
    fn grid_size_matches_term() {
        let t = term(7, 3, b"");
        let g = snapshot_grid(&t);
        assert_eq!((g.cols(), g.rows()), (7, 3));
    }
}
