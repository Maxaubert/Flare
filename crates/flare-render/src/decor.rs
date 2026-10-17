//! The decoration layer: selection, find matches and link ink painted into the same [`Scene`]
//! both painters draw. Every colour is opaque (the painters do not blend rects) and every edge is
//! a whole device pixel.

use crate::blend::blend_mask;
use crate::color::{Color, Palette, Rgba};
use crate::grid::Grid;
use crate::metrics::{CellMetrics, cell_origin};
use crate::scene::{RectFill, Scene, build_scene};

/// Cells on one VIEWPORT row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowSpan {
    pub row: usize,
    pub col: usize,
    pub width: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecorInk {
    /// WITH alpha: pt-default `#bbbbbb55` (the cursor colour at 0x55).
    pub selection: Rgba,
    pub find_bg: Rgba,
    pub find_border: Rgba,
    /// The accent-hi, opaque; pt-default `#f6a25b`.
    pub active_bg: Rgba,
    pub active_border: Rgba,
    /// `#79a7d8`, moved to 4.5:1 by the theme (M4 `link_color`).
    pub link: Rgba,
    pub link_underline_alpha: u8,
}

impl DecorInk {
    /// Prism Terminal's values; M4's theme code builds the others through this seam.
    pub fn pt_default() -> Self {
        Self {
            selection: Rgba {
                r: 187,
                g: 187,
                b: 187,
                a: 0x55,
            },
            find_bg: Rgba::rgb(0x4a, 0x51, 0x60),
            find_border: Rgba::rgb(0x6b, 0x74, 0x82),
            active_bg: Rgba::rgb(0xf6, 0xa2, 0x5b),
            active_border: Rgba::rgb(0xf6, 0xa2, 0x5b),
            link: Rgba::rgb(0x79, 0xa7, 0xd8),
            link_underline_alpha: 0x8c,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Decor {
    pub selection: Vec<RowSpan>,
    /// Every visible match, one span per row piece.
    pub find: Vec<RowSpan>,
    /// The active match's pieces.
    pub find_active: Vec<RowSpan>,
    /// Link ink and underline.
    pub links: Vec<RowSpan>,
    /// The hovered link: underline at full ink.
    pub hover: Vec<RowSpan>,
    /// `None`: [`DecorInk::pt_default`].
    pub ink: Option<DecorInk>,
}

impl Decor {
    pub fn is_empty(&self) -> bool {
        self.selection.is_empty()
            && self.find.is_empty()
            && self.find_active.is_empty()
            && self.links.is_empty()
            && self.hover.is_empty()
    }
}

/// `max(1, floor(scale))` device pixels: the Chromium rule for a CSS `1px` border.
pub fn hair_px(scale: f32) -> u32 {
    scale.floor().max(1.0) as u32
}

fn span_rect(span: &RowSpan, grid: &Grid, m: &CellMetrics) -> Option<RectFill> {
    if span.row >= grid.rows() || span.col >= grid.cols() || span.width == 0 {
        return None;
    }
    let width = span.width.min(grid.cols() - span.col);
    let (x, y) = cell_origin(span.col, span.row, m);
    Some(RectFill {
        x,
        y,
        w: width as u32 * m.cell_w,
        h: m.cell_h,
        color: Rgba::default(),
    })
}

/// Clips `r` to the scene; `None` when nothing is left.
fn clip(r: RectFill, width: u32, height: u32) -> Option<RectFill> {
    let x0 = i64::from(r.x).max(0);
    let y0 = i64::from(r.y).max(0);
    let x1 = (i64::from(r.x) + i64::from(r.w)).min(i64::from(width));
    let y1 = (i64::from(r.y) + i64::from(r.h)).min(i64::from(height));
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some(RectFill {
        x: x0 as i32,
        y: y0 as i32,
        w: (x1 - x0) as u32,
        h: (y1 - y0) as u32,
        color: r.color,
    })
}

fn push_clipped(out: &mut Vec<RectFill>, r: RectFill, width: u32, height: u32) {
    if let Some(c) = clip(r, width, height) {
        out.push(c);
    }
}

fn opaque(px: [u8; 4]) -> Rgba {
    Rgba::rgb(px[0], px[1], px[2])
}

/// Four rects of `hair` px drawn OUTSIDE `b`: top, bottom, left, right.
fn outline(out: &mut Vec<RectFill>, b: &RectFill, hair: u32, color: Rgba, w: u32, h: u32) {
    let hi = hair as i32;
    let full = b.w + 2 * hair;
    let edges = [
        (b.x - hi, b.y - hi, full, hair),
        (b.x - hi, b.y + b.h as i32, full, hair),
        (b.x - hi, b.y, hair, b.h),
        (b.x + b.w as i32, b.y, hair, b.h),
    ];
    for (x, y, ew, eh) in edges {
        let rect = RectFill {
            x,
            y,
            w: ew,
            h: eh,
            color,
        };
        push_clipped(out, rect, w, h);
    }
}

fn fill_spans(
    grid: &Grid,
    m: &CellMetrics,
    spans: &[RowSpan],
    color: Rgba,
    out: &mut Vec<RectFill>,
    (width, height): (u32, u32),
) {
    for s in spans {
        if let Some(mut r) = span_rect(s, grid, m) {
            r.color = color;
            push_clipped(out, r, width, height);
        }
    }
}

fn underline_spans(
    grid: &Grid,
    m: &CellMetrics,
    spans: &[RowSpan],
    color: Rgba,
    hair: u32,
    out: &mut Vec<RectFill>,
    (width, height): (u32, u32),
) {
    for s in spans {
        if let Some(r) = span_rect(s, grid, m) {
            let h = hair.min(r.h);
            let line = RectFill {
                y: r.y + (r.h - h) as i32,
                h,
                color,
                ..r
            };
            push_clipped(out, line, width, height);
        }
    }
}

/// [`build_scene`] plus the decoration (all colours opaque). Link cells take the link ink on the
/// grid first; then rects (find, selection, active find, the cursor again), then overlays after
/// the glyphs (link underline, hover underline, find outlines, active outlines).
pub fn build_scene_decor(
    grid: &mut Grid,
    palette: &Palette,
    m: &CellMetrics,
    width: u32,
    height: u32,
    decor: &Decor,
    hair: u32,
) -> Scene {
    let ink = decor.ink.unwrap_or_else(DecorInk::pt_default);
    for span in &decor.links {
        if span.row >= grid.rows() {
            continue;
        }
        let end = (span.col + span.width).min(grid.cols());
        for col in span.col..end {
            grid.cell_mut(col, span.row).fg = Color::Rgb(ink.link.r, ink.link.g, ink.link.b);
        }
    }
    let mut scene = build_scene(grid, palette, m, width, height);
    if decor.is_empty() {
        return scene;
    }
    let size = (width, height);
    let ground = [palette.bg.r, palette.bg.g, palette.bg.b, 255];
    let sel_cov = if ink.selection.a == 255 {
        77
    } else {
        ink.selection.a
    };
    let sel_color = opaque(blend_mask(ground, ink.selection, sel_cov));

    // Cell rects were pushed by `build_scene`; remember the cursor's before adding more.
    let cursor = grid.cursor;
    let cursor_rect = if cursor.visible && cursor.row < grid.rows() && cursor.col < grid.cols() {
        let (cx, cy) = cell_origin(cursor.col, cursor.row, m);
        scene
            .rects
            .iter()
            .rev()
            .find(|r| {
                r.color == palette.cursor
                    && r.y == cy
                    && r.x <= cx
                    && cx < r.x + r.w as i32
                    && r.h == m.cell_h
            })
            .copied()
    } else {
        None
    };

    fill_spans(grid, m, &decor.find, ink.find_bg, &mut scene.rects, size);
    fill_spans(grid, m, &decor.selection, sel_color, &mut scene.rects, size);
    fill_spans(
        grid,
        m,
        &decor.find_active,
        ink.active_bg,
        &mut scene.rects,
        size,
    );
    if let Some(r) = cursor_rect {
        scene.rects.push(r);
    }

    let hair = hair.max(1);
    let faint = opaque(blend_mask(ground, ink.link, ink.link_underline_alpha));
    let full = Rgba::rgb(ink.link.r, ink.link.g, ink.link.b);
    underline_spans(
        grid,
        m,
        &decor.links,
        faint,
        hair,
        &mut scene.overlays,
        size,
    );
    underline_spans(grid, m, &decor.hover, full, hair, &mut scene.overlays, size);

    for s in &decor.find {
        if let Some(r) = span_rect(s, grid, m) {
            outline(
                &mut scene.overlays,
                &r,
                hair,
                ink.find_border,
                width,
                height,
            );
        }
    }
    for s in &decor.find_active {
        if let Some(r) = span_rect(s, grid, m) {
            outline(
                &mut scene.overlays,
                &r,
                hair,
                ink.active_border,
                width,
                height,
            );
        }
    }
    scene
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::{RgbaImage, paint_scene};
    use crate::glyph::FakeGlyphs;
    use crate::grid::Cursor;
    use crate::metrics::{FontUnits, cell_metrics};

    const SCALES: [f32; 8] = [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 3.0];

    fn units() -> FontUnits {
        FontUnits {
            units_per_em: 2048.0,
            ascent: 1900.0,
            descent: -480.0,
            leading: 0.0,
            advance: 1200.0,
        }
    }

    fn setup(scale: f32) -> (Grid, Palette, CellMetrics, u32, u32) {
        let m = cell_metrics(&units(), scale);
        let mut g = Grid::new(20, 6);
        g.cursor.visible = false;
        let w = 2 * m.padding + 20 * m.cell_w;
        let h = 2 * m.padding + 6 * m.cell_h;
        (g, Palette::pt_default(), m, w, h)
    }

    fn paint(scene: &Scene, m: &CellMetrics) -> RgbaImage {
        let mut fake = FakeGlyphs::new(m.cell_w, m.cell_h);
        let mut img = RgbaImage::new(scene.width, scene.height);
        paint_scene(scene, &mut fake, &mut img);
        img
    }

    fn centre(img: &RgbaImage, m: &CellMetrics, col: usize, row: usize) -> [u8; 4] {
        let (x, y) = cell_origin(col, row, m);
        img.pixel(x as u32 + m.cell_w / 2, y as u32 + m.cell_h / 2)
    }

    fn span(row: usize, col: usize, width: usize) -> RowSpan {
        RowSpan { row, col, width }
    }

    fn rgb(c: Rgba) -> [u8; 4] {
        [c.r, c.g, c.b, 255]
    }

    #[test]
    fn an_empty_decor_changes_nothing() {
        for scale in [1.0, 1.5, 2.25] {
            let (mut g, p, m, w, h) = setup(scale);
            g.put_str(
                0,
                0,
                "hi",
                Color::DefaultFg,
                Color::DefaultBg,
                Default::default(),
            );
            g.cursor.visible = true;
            let want = build_scene(&g, &p, &m, w, h);
            let got = build_scene_decor(&mut g, &p, &m, w, h, &Decor::default(), 1);
            assert_eq!(got, want);
        }
    }

    #[test]
    fn pt_default_ink_values() {
        let i = DecorInk::pt_default();
        assert_eq!(
            i.selection,
            Rgba {
                r: 187,
                g: 187,
                b: 187,
                a: 0x55
            }
        );
        assert_eq!(i.find_bg, Rgba::from_hex("#4a5160").unwrap());
        assert_eq!(i.find_border, Rgba::from_hex("#6b7482").unwrap());
        assert_eq!(i.link, Rgba::from_hex("#79a7d8").unwrap());
        assert_eq!(i.link_underline_alpha, 0x8c);
    }

    fn selected(scale: f32, ink: Option<DecorInk>) -> (RgbaImage, CellMetrics) {
        let (mut g, p, m, w, h) = setup(scale);
        let d = Decor {
            selection: vec![span(1, 1, 3)],
            ink,
            ..Default::default()
        };
        let s = build_scene_decor(&mut g, &p, &m, w, h, &d, hair_px(scale));
        (paint(&s, &m), m)
    }

    #[test]
    fn the_selection_is_the_opaque_blend_over_the_ground() {
        let (img, m) = selected(1.0, None);
        assert_eq!(centre(&img, &m, 2, 1), [78, 78, 78, 255]);
    }

    #[test]
    fn an_opaque_selection_colour_blends_at_thirty_percent() {
        let mut ink = DecorInk::pt_default();
        ink.selection = Rgba::from_hex("#ff0000ff").unwrap();
        let p = Palette::pt_default();
        let ground = [0u8, 0, 0, 255];
        let want = blend_mask(ground, ink.selection, 77);
        assert_eq!(want, [77, 0, 0, 255]);
        let (mut g, mut p2, m, w, h) = setup(1.0);
        p2.bg = Rgba::rgb(0, 0, 0);
        let _ = p;
        let d = Decor {
            selection: vec![span(0, 0, 2)],
            ink: Some(ink),
            ..Default::default()
        };
        let s = build_scene_decor(&mut g, &p2, &m, w, h, &d, 1);
        assert_eq!(centre(&paint(&s, &m), &m, 1, 0), [77, 0, 0, 255]);
    }

    #[test]
    fn selection_rects_tile_cells_exactly_at_every_scale() {
        for scale in SCALES {
            let (mut g, p, m, w, h) = setup(scale);
            let d = Decor {
                selection: vec![span(0, 2, 5), span(1, 0, 10), span(2, 0, 3)],
                ..Default::default()
            };
            let s = build_scene_decor(&mut g, &p, &m, w, h, &d, hair_px(scale));
            let img = paint(&s, &m);
            let sel = [78u8, 78, 78, 255];
            let ground = rgb(p.bg);
            for (row, c0, c1) in [(0usize, 2usize, 7usize), (1, 0, 10), (2, 0, 3)] {
                for col in 0..20 {
                    let want = if (c0..c1).contains(&col) { sel } else { ground };
                    assert_eq!(centre(&img, &m, col, row), want, "{scale} {col},{row}");
                }
                let (x0, y) = cell_origin(c0, row, &m);
                assert_eq!(x0 as u32, m.padding + c0 as u32 * m.cell_w);
                assert_eq!(img.pixel(x0 as u32, y as u32 + 1), sel);
                if x0 > 0 {
                    let left = img.pixel(x0 as u32 - 1, y as u32 + 1);
                    assert_ne!(left, sel, "{scale} left of span");
                }
            }
            assert_eq!(centre(&img, &m, 0, 3), ground);
        }
    }

    #[test]
    fn selected_text_keeps_its_colour() {
        let (mut g, p, m, w, h) = setup(1.0);
        g.put_str(
            0,
            0,
            "abc",
            Color::Indexed(2),
            Color::DefaultBg,
            Default::default(),
        );
        let plain = build_scene_decor(&mut g.clone(), &p, &m, w, h, &Decor::default(), 1);
        let d = Decor {
            selection: vec![span(0, 0, 3)],
            ..Default::default()
        };
        let sel = build_scene_decor(&mut g, &p, &m, w, h, &d, 1);
        assert_eq!(plain.glyphs.len(), sel.glyphs.len());
        for (a, b) in plain.glyphs.iter().zip(&sel.glyphs) {
            assert_eq!(a.color, b.color);
        }
    }

    #[test]
    fn precedence_is_find_then_selection_then_active() {
        let (_, p, m, w, h) = setup(1.0);
        let ink = DecorInk::pt_default();
        let run = |sel: bool, active: bool| {
            let (mut g, ..) = setup(1.0);
            let d = Decor {
                find: vec![span(0, 1, 1)],
                selection: if sel { vec![span(0, 1, 1)] } else { vec![] },
                find_active: if active { vec![span(0, 1, 1)] } else { vec![] },
                ..Default::default()
            };
            let s = build_scene_decor(&mut g, &p, &m, w, h, &d, 1);
            centre(&paint(&s, &m), &m, 1, 0)
        };
        assert_eq!(run(true, true), rgb(ink.active_bg));
        assert_eq!(run(true, false), [78, 78, 78, 255]);
        assert_eq!(run(false, false), rgb(ink.find_bg));
    }

    #[test]
    fn the_cursor_stays_on_top_of_a_selection() {
        let (mut g, p, m, w, h) = setup(1.0);
        g.cursor = Cursor {
            col: 2,
            row: 1,
            visible: true,
        };
        let d = Decor {
            selection: vec![span(1, 0, 5)],
            ..Default::default()
        };
        let s = build_scene_decor(&mut g, &p, &m, w, h, &d, 1);
        let img = paint(&s, &m);
        assert_eq!(centre(&img, &m, 2, 1), rgb(p.cursor));
        assert_eq!(centre(&img, &m, 3, 1), [78, 78, 78, 255]);
    }

    #[test]
    fn link_cells_wear_the_link_ink() {
        let (mut g, p, m, w, h) = setup(1.0);
        g.put_str(
            0,
            0,
            "ab",
            Color::DefaultFg,
            Color::DefaultBg,
            Default::default(),
        );
        let d = Decor {
            links: vec![span(0, 0, 2)],
            ..Default::default()
        };
        let s = build_scene_decor(&mut g, &p, &m, w, h, &d, 1);
        let link = DecorInk::pt_default().link;
        assert_eq!(s.glyphs.len(), 2);
        assert!(s.glyphs.iter().all(|gl| gl.color == link));
    }

    #[test]
    fn the_link_underline_is_hair_thick_at_the_cell_bottom() {
        for (scale, want) in [(1.0f32, 1u32), (1.5, 1), (2.25, 2)] {
            let (mut g, p, m, w, h) = setup(scale);
            let hair = hair_px(scale);
            assert_eq!(hair, want);
            let d = Decor {
                links: vec![span(2, 3, 4)],
                ..Default::default()
            };
            let s = build_scene_decor(&mut g, &p, &m, w, h, &d, hair);
            let img = paint(&s, &m);
            let (x, y) = cell_origin(3, 2, &m);
            let x = x as u32 + 1;
            let y = y as u32;
            let line = blend_mask(rgb(p.bg), DecorInk::pt_default().link, 0x8c);
            for k in 0..hair {
                assert_eq!(img.pixel(x, y + m.cell_h - 1 - k), line, "{scale}");
            }
            assert_ne!(img.pixel(x, y + m.cell_h - 1 - hair), line);
        }
    }

    #[test]
    fn the_hovered_link_is_underlined_at_full_ink() {
        let (mut g, p, m, w, h) = setup(1.0);
        let d = Decor {
            hover: vec![span(0, 0, 3)],
            ..Default::default()
        };
        let s = build_scene_decor(&mut g, &p, &m, w, h, &d, 1);
        let img = paint(&s, &m);
        let (x, y) = cell_origin(1, 0, &m);
        assert_eq!(
            img.pixel(x as u32, y as u32 + m.cell_h - 1),
            rgb(DecorInk::pt_default().link)
        );
    }

    #[test]
    fn a_find_outline_is_hair_thick_and_outside_the_box() {
        let scale = 2.25;
        let (mut g, p, m, w, h) = setup(scale);
        let hair = hair_px(scale);
        assert_eq!(hair, 2);
        let ink = DecorInk::pt_default();
        let d = Decor {
            find: vec![span(2, 2, 3)],
            find_active: vec![span(4, 5, 2)],
            ..Default::default()
        };
        let s = build_scene_decor(&mut g, &p, &m, w, h, &d, hair);
        let img = paint(&s, &m);
        let (x, y) = cell_origin(2, 2, &m);
        let my = y as u32 + m.cell_h / 2;
        assert_eq!(img.pixel(x as u32 - 1, my), rgb(ink.find_border));
        assert_eq!(img.pixel(x as u32 - 2, my), rgb(ink.find_border));
        assert_eq!(img.pixel(x as u32, my), rgb(ink.find_bg));
        let (ax, ay) = cell_origin(5, 4, &m);
        let amy = ay as u32 + m.cell_h / 2;
        assert_eq!(img.pixel(ax as u32 - 1, amy), rgb(ink.active_border));
        assert_eq!(img.pixel(ax as u32, amy), rgb(ink.active_bg));
    }

    #[test]
    fn outlines_are_clipped_at_the_scene_edge() {
        let (mut g, p, m, w, h) = setup(2.0);
        let d = Decor {
            find: vec![span(0, 0, 1)],
            find_active: vec![span(0, 0, 1)],
            ..Default::default()
        };
        // Offset the view so the box touches the scene edge.
        let mut m0 = m;
        m0.padding = 0;
        let s = build_scene_decor(&mut g, &p, &m0, w, h, &d, 2);
        for r in s.rects.iter().chain(&s.overlays) {
            assert!(r.x >= 0 && r.y >= 0 && r.w > 0 && r.h > 0);
            assert!(i64::from(r.x) + i64::from(r.w) <= i64::from(w));
            assert!(i64::from(r.y) + i64::from(r.h) <= i64::from(h));
        }
    }

    #[test]
    fn hair_px_is_max_one_floor() {
        let cases = [
            (1.0, 1),
            (1.25, 1),
            (1.5, 1),
            (1.75, 1),
            (2.0, 2),
            (2.25, 2),
            (2.5, 2),
            (3.0, 3),
            (0.5, 1),
        ];
        for (s, want) in cases {
            assert_eq!(hair_px(s), want, "{s}");
        }
    }

    #[test]
    fn spans_past_the_grid_are_skipped() {
        let (mut g, p, m, w, h) = setup(1.0);
        let d = Decor {
            links: vec![span(99, 0, 3), span(0, 18, 9)],
            selection: vec![span(99, 0, 3), span(0, 50, 3)],
            ..Default::default()
        };
        let s = build_scene_decor(&mut g, &p, &m, w, h, &d, 1);
        assert!(s.rects.iter().all(|r| i64::from(r.x) < i64::from(w)));
    }
}
