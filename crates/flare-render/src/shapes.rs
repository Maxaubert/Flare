//! Shapes Flare draws itself, edge to edge in the cell, instead of asking a font (owner bug
//! 2026-10-21: Claude Code's status line showed a tofu box and a star where Powerline's chevron and
//! round caps belong, because the font stack's first face has no Powerline glyphs and a symbol
//! font answered the private-use codepoints with unrelated drawings).
//!
//! The set is the Powerline symbols U+E0B0 to U+E0BF: the solid and thin arrows, the half circles
//! (solid and outline) and the eight diagonal triangles and lines. Each is a coverage mask the
//! size of the cell, anti-aliased by sampling, so a cap meets the next cell's background with no
//! seam at any scale. The painters tint it with the cell's foreground exactly like a font glyph.

use crate::glyph::{GlyphBitmap, GlyphKind};

/// Samples per pixel side. 8 x 8 = 64 levels of edge coverage.
const SS: usize = 8;

/// Whether `ch` has a built-in shape.
pub fn has_shape(ch: char) -> bool {
    matches!(ch as u32, 0xE0B0..=0xE0BF)
}

/// The mask for `ch` in a cell of `w` x `h` device pixels, `stroke` px wide for thin shapes. The
/// bitmap sits at left 0 and top `baseline` so it fills the cell from its top-left corner.
pub fn shape_bitmap(ch: char, w: u32, h: u32, baseline: u32, stroke: f32) -> Option<GlyphBitmap> {
    if !has_shape(ch) || w == 0 || h == 0 {
        return None;
    }
    let (wf, hf) = (w as f32, h as f32);
    let s = stroke.max(1.0);
    let inside = shape_fn(ch as u32, wf, hf, s);
    let mut data = vec![0u8; (w * h) as usize];
    for py in 0..h as usize {
        for px in 0..w as usize {
            let mut hit = 0u32;
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = px as f32 + (sx as f32 + 0.5) / SS as f32;
                    let y = py as f32 + (sy as f32 + 0.5) / SS as f32;
                    if inside(x, y) {
                        hit += 1;
                    }
                }
            }
            data[py * w as usize + px] =
                ((hit * 255 + (SS * SS) as u32 / 2) / (SS * SS) as u32) as u8;
        }
    }
    Some(GlyphBitmap {
        left: 0,
        top: baseline as i32,
        width: w,
        height: h,
        kind: GlyphKind::Mask,
        data,
    })
}

type Inside = Box<dyn Fn(f32, f32) -> bool>;

fn shape_fn(code: u32, w: f32, h: f32, s: f32) -> Inside {
    let mid = h / 2.0;
    match code {
        // Solid arrows: a triangle whose base is the cell edge and whose tip is the opposite edge.
        0xE0B0 => Box::new(move |x, y| (y - mid).abs() <= mid * (1.0 - x / w)),
        0xE0B2 => Box::new(move |x, y| (y - mid).abs() <= mid * (x / w)),
        // Thin arrows: a stroked chevron from the corners to the tip on the far edge.
        0xE0B1 => Box::new(move |x, y| chevron(x, y, w, h, s, false)),
        0xE0B3 => Box::new(move |x, y| chevron(x, y, w, h, s, true)),
        // Half circles (an ellipse the size of the cell, cut in half): solid, then outline.
        0xE0B4 => Box::new(move |x, y| ellipse_in(x, y, 0.0, mid, w, mid)),
        0xE0B6 => Box::new(move |x, y| ellipse_in(x, y, w, mid, w, mid)),
        0xE0B5 => Box::new(move |x, y| ring(x, y, 0.0, w, h, s)),
        0xE0B7 => Box::new(move |x, y| ring(x, y, w, w, h, s)),
        // The corner triangles and the diagonals between them.
        0xE0B8 => Box::new(move |x, y| y >= h * x / w),
        0xE0BA => Box::new(move |x, y| y >= h * (1.0 - x / w)),
        0xE0BC => Box::new(move |x, y| y <= h * (1.0 - x / w)),
        0xE0BE => Box::new(move |x, y| y <= h * x / w),
        0xE0B9 | 0xE0BF => Box::new(move |x, y| near_line(x, y, (0.0, 0.0), (w, h), s)),
        _ => Box::new(move |x, y| near_line(x, y, (w, 0.0), (0.0, h), s)),
    }
}

/// Inside the ellipse centred at (cx, cy) with radii (rx, ry).
fn ellipse_in(x: f32, y: f32, cx: f32, cy: f32, rx: f32, ry: f32) -> bool {
    let (nx, ny) = ((x - cx) / rx, (y - cy) / ry);
    nx * nx + ny * ny <= 1.0
}

/// The outline of the half circle whose flat side is the cell edge at `cx` (0 or w); the stroke
/// stays inside the cell, so its outer edge touches the far edge and the top and bottom.
fn ring(x: f32, y: f32, cx: f32, w: f32, h: f32, s: f32) -> bool {
    let (rx, ry) = ((w - s / 2.0).max(0.5), (h / 2.0 - s / 2.0).max(0.5));
    let (dx, dy) = (x - cx, y - h / 2.0);
    let f = (dx / rx).powi(2) + (dy / ry).powi(2) - 1.0;
    let gx = 2.0 * dx / (rx * rx);
    let gy = 2.0 * dy / (ry * ry);
    let g = (gx * gx + gy * gy).sqrt().max(1e-6);
    (f / g).abs() <= s / 2.0
}

/// The thin arrow: two strokes meeting at the middle of the far edge. `left` mirrors it.
fn chevron(x: f32, y: f32, w: f32, h: f32, s: f32, left: bool) -> bool {
    let x = if left { w - x } else { x };
    let tip = (w, h / 2.0);
    near_line(x, y, (0.0, 0.0), tip, s) || near_line(x, y, (0.0, h), tip, s)
}

/// Within half a stroke of the segment a-b, the segment extended past both ends so the cell edge
/// clips it square.
fn near_line(x: f32, y: f32, a: (f32, f32), b: (f32, f32), s: f32) -> bool {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = (dx * dx + dy * dy).sqrt();
    let (ux, uy) = (dx / len, dy / len);
    let (px, py) = (x - a.0, y - a.1);
    let along = (px * ux + py * uy).clamp(-s, len + s);
    let (qx, qy) = (a.0 + ux * along, a.1 + uy * along);
    let (ex, ey) = (x - qx, y - qy);
    (ex * ex + ey * ey).sqrt() <= s / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bm(ch: char, w: u32, h: u32, stroke: f32) -> GlyphBitmap {
        shape_bitmap(ch, w, h, h * 4 / 5, stroke).expect("a shape")
    }

    fn px(b: &GlyphBitmap, x: u32, y: u32) -> u8 {
        b.data[(y * b.width + x) as usize]
    }

    /// Cell sizes of 100, 150, 200, 225 and 300 percent of an 8 x 16 cell.
    const CELLS: [(u32, u32, f32); 5] = [
        (8, 16, 1.0),
        (12, 24, 1.5),
        (16, 32, 2.0),
        (18, 36, 2.25),
        (24, 48, 3.0),
    ];

    #[test]
    fn only_the_powerline_range_has_a_shape() {
        assert!(has_shape('\u{E0B0}') && has_shape('\u{E0BF}'));
        assert!(!has_shape('\u{E0AF}') && !has_shape('\u{E0C0}') && !has_shape('A'));
        assert!(shape_bitmap('A', 8, 16, 12, 1.0).is_none());
    }

    #[test]
    fn every_shape_fills_the_cell_box_and_has_ink() {
        for (w, h, s) in CELLS {
            for code in 0xE0B0..=0xE0BFu32 {
                let b = bm(char::from_u32(code).unwrap(), w, h, s);
                assert_eq!((b.width, b.height, b.left), (w, h, 0));
                assert_eq!(b.top, (h * 4 / 5) as i32);
                assert!(
                    b.data.iter().any(|&v| v > 0),
                    "U+{code:04X} is empty at {w}x{h}"
                );
                assert!(
                    b.data.contains(&0),
                    "U+{code:04X} is a solid box at {w}x{h}"
                );
            }
        }
    }

    #[test]
    fn a_solid_cap_meets_its_neighbour_without_a_seam() {
        // The flat side of a cap is the cell edge it shares with the cell beside it: that whole
        // column must be solid (to the sampling's edge at the rounded ends).
        for (w, h, s) in CELLS {
            let right = bm('\u{E0B4}', w, h, s);
            let left = bm('\u{E0B6}', w, h, s);
            for y in 0..h {
                assert!(
                    px(&right, 0, y) >= 250,
                    "right cap column 0 row {y} at {w}x{h}"
                );
                assert!(
                    px(&left, w - 1, y) >= 250,
                    "left cap column {} row {y} at {w}x{h}",
                    w - 1
                );
            }
            // Mirror images of each other.
            for y in 0..h {
                for x in 0..w {
                    assert_eq!(px(&right, x, y), px(&left, w - 1 - x, y));
                }
            }
        }
    }

    #[test]
    fn the_cap_reaches_the_far_edge_and_is_round() {
        for (w, h, s) in CELLS {
            let b = bm('\u{E0B4}', w, h, s);
            assert!(px(&b, w - 1, h / 2) >= 128, "tip at {w}x{h}");
            assert!(
                px(&b, w - 1, 0) == 0 && px(&b, w - 1, h - 1) == 0,
                "far corners empty"
            );
            // Wider in the middle than near the ends.
            let row = |y: u32| (0..w).filter(|&x| px(&b, x, y) >= 128).count();
            assert!(row(h / 2) > row(1) && row(h / 2) > row(h - 2));
        }
    }

    #[test]
    fn a_solid_arrow_is_a_full_edge_and_a_tip() {
        for (w, h, s) in CELLS {
            let r = bm('\u{E0B0}', w, h, s);
            let l = bm('\u{E0B2}', w, h, s);
            for y in 2..h - 2 {
                assert!(px(&r, 0, y) >= 128, "arrow base row {y} at {w}x{h}");
                assert!(px(&l, w - 1, y) >= 128);
            }
            assert!(px(&r, w - 1, h / 2) > 0 && px(&r, w - 1, 0) == 0);
            assert!(px(&l, 0, h / 2) > 0 && px(&l, 0, 0) == 0);
        }
    }

    #[test]
    fn the_thin_chevron_is_a_stroke_not_a_fill() {
        for (w, h, s) in CELLS {
            let b = bm('\u{E0B1}', w, h, s);
            // Ink at the two corners of the open side and at the tip, none in the middle of the
            // open side.
            assert!(
                px(&b, 0, 0) > 0 && px(&b, 0, h - 1) > 0,
                "corners at {w}x{h}"
            );
            assert!(
                px(&b, w - 1, h / 2) > 0 || px(&b, w - 1, h / 2 - 1) > 0,
                "tip at {w}x{h}"
            );
            assert_eq!(px(&b, 0, h / 2), 0, "open side at {w}x{h}");
            let total: u32 = b.data.iter().map(|&v| u32::from(v)).sum();
            assert!(
                total < u32::from(255u8) * w * h / 3,
                "too much ink for a stroke at {w}x{h}"
            );
            // The left arrow is its mirror.
            let m = bm('\u{E0B3}', w, h, s);
            for y in 0..h {
                for x in 0..w {
                    assert_eq!(px(&b, x, y), px(&m, w - 1 - x, y));
                }
            }
        }
    }

    #[test]
    fn the_outline_cap_is_a_ring_inside_the_cell() {
        for (w, h, s) in CELLS {
            let b = bm('\u{E0B5}', w, h, s);
            assert!(
                px(&b, w - 1, h / 2) > 0 || px(&b, w - 1, h / 2 - 1) > 0,
                "far point at {w}x{h}"
            );
            assert_eq!(px(&b, 0, h / 2), 0, "the inside is empty at {w}x{h}");
            assert!(
                px(&b, 0, 0) > 0 || px(&b, 1, 0) > 0,
                "top edge reached at {w}x{h}"
            );
        }
    }

    #[test]
    fn a_corner_triangle_has_solid_flat_edges() {
        for (w, h, s) in CELLS {
            let b = bm('\u{E0B8}', w, h, s);
            for i in 4..h {
                assert!(px(&b, 0, i) >= 250, "left edge row {i}");
            }
            for i in 0..w - 1 {
                assert!(px(&b, i, h - 1) >= 128, "bottom edge col {i} at {w}x{h}");
            }
            assert_eq!(px(&b, w - 1, 0), 0);
        }
    }

    #[test]
    fn the_shapes_are_the_same_at_every_call() {
        let a = bm('\u{E0B6}', 17, 33, 1.3);
        let b = bm('\u{E0B6}', 17, 33, 1.3);
        assert_eq!(a, b);
    }
}
