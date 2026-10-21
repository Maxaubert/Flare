//! Scene: the shared layout step. Turns a grid into rects, glyph draws and overlays that both
//! painters paint, so text is never laid out twice.

use crate::color::{Color, Palette, Rgba, resolve_bg, resolve_fg};
use crate::glyph::GlyphKey;
use crate::grid::{CellFlags, Grid};
use crate::metrics::{CellMetrics, cell_origin};

/// A solid rectangle in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RectFill {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub color: Rgba,
}

/// One cluster to draw: its pen position (left edge, baseline) and the foreground colour.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphDraw {
    pub key: GlyphKey,
    pub pen_x: i32,
    pub baseline_y: i32,
    pub color: Rgba,
}

/// Everything one frame paints, in painting order: clear, rects, glyphs, overlays.
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    pub width: u32,
    pub height: u32,
    pub clear: Rgba,
    pub rects: Vec<RectFill>,
    pub glyphs: Vec<GlyphDraw>,
    pub overlays: Vec<RectFill>,
}

/// Who paints the theme's ground behind the cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GroundPaint {
    /// The grid clears to `palette.bg` and draws every cell that differs from it.
    #[default]
    InGrid,
    /// The UI layer paints the ground: the grid clears to transparent and leaves default-background
    /// cells clear, so glass shows through where the theme's ground is (xterm's panel paints it).
    ByUi,
}

/// Lays out `grid` into a [`Scene`] of `width` x `height` physical pixels, the ground in the grid.
pub fn build_scene(
    grid: &Grid,
    palette: &Palette,
    m: &CellMetrics,
    width: u32,
    height: u32,
) -> Scene {
    build_scene_ground(grid, palette, m, width, height, GroundPaint::InGrid)
}

/// [`build_scene`] with a choice of who paints the ground.
pub fn build_scene_ground(
    grid: &Grid,
    palette: &Palette,
    m: &CellMetrics,
    width: u32,
    height: u32,
    ground: GroundPaint,
) -> Scene {
    let by_ui = ground == GroundPaint::ByUi;
    let mut scene = Scene {
        width,
        height,
        clear: if by_ui {
            Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 0,
            }
        } else {
            palette.bg
        },
        rects: Vec::new(),
        glyphs: Vec::new(),
        overlays: Vec::new(),
    };
    let cursor = grid.cursor;
    for row in 0..grid.rows() {
        for col in 0..grid.cols() {
            let cell = grid.cell(col, row);
            if cell.flags.contains(CellFlags::WIDE_SPACER) {
                continue;
            }
            let wide = cell.flags.contains(CellFlags::WIDE);
            let span: u32 = if wide { 2 } else { 1 };
            let (x, y) = cell_origin(col, row, m);
            let bold = cell.flags.contains(CellFlags::BOLD);
            let italic = cell.flags.contains(CellFlags::ITALIC);

            let mut fg = resolve_fg(palette, cell.fg, bold);
            let mut bg = resolve_bg(palette, cell.bg);
            let inverse = cell.flags.contains(CellFlags::INVERSE);
            if inverse {
                std::mem::swap(&mut fg, &mut bg);
            }
            let under_cursor = cursor.visible
                && cursor.row == row
                && (cursor.col == col || (wide && cursor.col == col + 1));
            if under_cursor {
                bg = palette.cursor;
                fg = palette.cursor_accent;
            }

            let w = span * m.cell_w;
            let draw_bg = if by_ui {
                // An explicit background that equals the ground is still drawn, as xterm does.
                under_cursor || inverse || cell.bg != Color::DefaultBg
            } else {
                under_cursor || bg != palette.bg
            };
            if draw_bg {
                scene.rects.push(RectFill {
                    x,
                    y,
                    w,
                    h: m.cell_h,
                    color: bg,
                });
            }
            if cell.ch != ' ' || cell.combining.is_some() {
                scene.glyphs.push(GlyphDraw {
                    key: GlyphKey {
                        text: cell.cluster(),
                        bold,
                        italic,
                    },
                    pen_x: x,
                    baseline_y: y + m.baseline as i32,
                    color: fg,
                });
            }
            if cell.flags.contains(CellFlags::UNDERLINE) {
                scene.overlays.push(RectFill {
                    x,
                    y: y + m.underline_y as i32,
                    w,
                    h: m.underline_h,
                    color: fg,
                });
            }
        }
    }
    scene
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Cursor;
    use crate::metrics::{FontUnits, cell_metrics};

    fn metrics() -> CellMetrics {
        cell_metrics(
            &FontUnits {
                units_per_em: 2048.0,
                ascent: 1900.0,
                descent: -480.0,
                leading: 0.0,
                advance: 1200.0,
            },
            1.0,
        )
    }

    fn scene(grid: &Grid) -> Scene {
        build_scene(grid, &Palette::pt_default(), &metrics(), 200, 100)
    }

    fn no_cursor(grid: &mut Grid) {
        grid.cursor.visible = false;
    }

    #[test]
    fn blank_grid_has_only_the_cursor() {
        let p = Palette::pt_default();
        let s = scene(&Grid::new(3, 2));
        assert_eq!(
            s.rects,
            vec![RectFill {
                x: 4,
                y: 4,
                w: 8,
                h: 16,
                color: p.cursor
            }]
        );
        assert!(s.glyphs.is_empty());
    }

    #[test]
    fn hidden_cursor_draws_nothing() {
        let p = Palette::pt_default();
        let mut g = Grid::new(3, 2);
        no_cursor(&mut g);
        let s = scene(&g);
        assert!(s.rects.is_empty());
        assert!(s.glyphs.is_empty());
        assert!(s.overlays.is_empty());
        assert_eq!(s.clear, p.bg);
    }

    #[test]
    fn cursor_on_spacer_marks_the_wide_cell() {
        let p = Palette::pt_default();
        let mut g = Grid::new(3, 2);
        g.put_str(
            0,
            0,
            "中",
            Color::DefaultFg,
            Color::DefaultBg,
            CellFlags::empty(),
        );
        g.cursor = Cursor {
            col: 1,
            row: 0,
            visible: true,
        };
        let s = scene(&g);
        assert_eq!(
            s.rects,
            vec![RectFill {
                x: 4,
                y: 4,
                w: 16,
                h: 16,
                color: p.cursor
            }]
        );
    }

    #[test]
    fn coloured_bg_cell_gets_a_rect() {
        let mut g = Grid::new(3, 2);
        no_cursor(&mut g);
        g.cell_mut(1, 0).bg = Color::Indexed(1);
        let s = scene(&g);
        assert_eq!(
            s.rects,
            vec![RectFill {
                x: 12,
                y: 4,
                w: 8,
                h: 16,
                color: Rgba::from_hex("#ff615a").unwrap(),
            }]
        );
    }

    #[test]
    fn wide_cell_spans_two() {
        let mut g = Grid::new(3, 2);
        no_cursor(&mut g);
        g.put_str(
            0,
            0,
            "中",
            Color::DefaultFg,
            Color::DefaultBg,
            CellFlags::empty(),
        );
        g.cell_mut(0, 0).bg = Color::Indexed(1);
        let s = scene(&g);
        assert_eq!(s.glyphs.len(), 1);
        assert_eq!(s.glyphs[0].pen_x, 4);
        assert_eq!(s.glyphs[0].baseline_y, 4 + 12);
        assert_eq!(
            s.rects,
            vec![RectFill {
                x: 4,
                y: 4,
                w: 16,
                h: 16,
                color: Rgba::from_hex("#ff615a").unwrap(),
            }]
        );
    }

    #[test]
    fn inverse_swaps() {
        let p = Palette::pt_default();
        let mut g = Grid::new(3, 2);
        no_cursor(&mut g);
        g.put_str(
            0,
            0,
            "A",
            Color::DefaultFg,
            Color::DefaultBg,
            CellFlags::INVERSE,
        );
        let s = scene(&g);
        assert_eq!(
            s.rects,
            vec![RectFill {
                x: 4,
                y: 4,
                w: 8,
                h: 16,
                color: p.fg
            }]
        );
        assert_eq!(s.glyphs[0].color, p.bg);
    }

    #[test]
    fn cursor_inverts_glyph() {
        let p = Palette::pt_default();
        let mut g = Grid::new(3, 2);
        g.put_str(
            0,
            0,
            "A",
            Color::DefaultFg,
            Color::DefaultBg,
            CellFlags::empty(),
        );
        let s = scene(&g);
        assert_eq!(s.glyphs[0].color, p.bg);
    }

    #[test]
    fn underline_overlay() {
        let p = Palette::pt_default();
        let mut g = Grid::new(3, 2);
        no_cursor(&mut g);
        g.put_str(
            0,
            0,
            "a",
            Color::DefaultFg,
            Color::DefaultBg,
            CellFlags::UNDERLINE,
        );
        let s = scene(&g);
        assert_eq!(
            s.overlays,
            vec![RectFill {
                x: 4,
                y: 4 + 14,
                w: 8,
                h: 1,
                color: p.fg
            }]
        );
    }

    #[test]
    fn bold_key() {
        let mut g = Grid::new(3, 2);
        no_cursor(&mut g);
        g.put_str(
            0,
            0,
            "a",
            Color::Indexed(2),
            Color::DefaultBg,
            CellFlags::BOLD,
        );
        let s = scene(&g);
        assert!(s.glyphs[0].key.bold);
        assert_eq!(s.glyphs[0].color, Rgba::from_hex("#ddf88f").unwrap());
    }

    fn by_ui_scene(grid: &Grid) -> Scene {
        build_scene_ground(
            grid,
            &Palette::pt_default(),
            &metrics(),
            200,
            100,
            GroundPaint::ByUi,
        )
    }

    #[test]
    fn by_ui_clears_to_transparent() {
        let g = Grid::new(3, 2);
        assert_eq!(by_ui_scene(&g).clear.a, 0);
    }

    #[test]
    fn in_grid_clear_is_the_ground() {
        let g = Grid::new(3, 2);
        assert_eq!(scene(&g).clear, Palette::pt_default().bg);
        assert_eq!(GroundPaint::default(), GroundPaint::InGrid);
    }

    #[test]
    fn by_ui_skips_default_background_cells_and_draws_explicit_ones() {
        let p = Palette::pt_default();
        let mut g = Grid::new(4, 2);
        no_cursor(&mut g);
        assert!(by_ui_scene(&g).rects.is_empty());

        g.cell_mut(1, 0).bg = Color::Rgb(p.bg.r, p.bg.g, p.bg.b);
        assert_eq!(by_ui_scene(&g).rects.len(), 1);

        g.cell_mut(2, 0).flags = CellFlags::INVERSE;
        assert_eq!(by_ui_scene(&g).rects.len(), 2);

        g.cursor = Cursor {
            col: 3,
            row: 1,
            visible: true,
        };
        let s = by_ui_scene(&g);
        assert_eq!(s.rects.len(), 3);
        assert_eq!(s.rects[2].color, p.cursor);
    }

    #[test]
    fn in_grid_is_unchanged() {
        let mut g = Grid::new(3, 2);
        no_cursor(&mut g);
        g.cell_mut(1, 0).bg = Color::Rgb(0x17, 0x17, 0x17);
        // The explicit background equals the ground: InGrid still skips it, as M1 did.
        assert!(scene(&g).rects.is_empty());
    }
}
