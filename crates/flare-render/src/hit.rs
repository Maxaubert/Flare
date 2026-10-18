//! Pointer-to-cell hit testing in whole device pixels, filled by Task 2.
//! Column boundaries come from the same origins the grid paints at (`cell_origin_in`), so a pointer
//! lands on the column it is drawn in under either cell-rounding mode.
use crate::metrics::{CELL_ROUNDING, CellMetrics, CellRounding, col_origin_px};

/// Where a pointer is on the grid image. `x` and `y` are PHYSICAL pixels from the image's top-left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub col: usize,   // clamped to 0..cols-1
    pub row: i64,     // NOT clamped: -1 or less above the first row, `rows` or more below the last
    pub right: bool, // the pointer is in the right half of its cell (left half of a clamped column: false)
    pub inside: bool, // inside the cell area (padding excluded)
}

/// The cell under (x, y), under `CELL_ROUNDING`. Left of the padding is column 0 left half; right of
/// the last column is column cols-1 right half. `right` = fraction >= 0.5. `row = floor((y - pad) / cell_h)`
/// as i64 (`floor` for negatives, so y < pad gives -1).
pub fn hit_cell(x: f64, y: f64, m: &CellMetrics, cols: usize, rows: usize) -> Hit {
    hit_cell_in(x, y, m, cols, rows, CELL_ROUNDING)
}

/// [`hit_cell`] under an explicit cell-rounding mode.
pub fn hit_cell_in(
    x: f64,
    y: f64,
    m: &CellMetrics,
    cols: usize,
    rows: usize,
    mode: CellRounding,
) -> Hit {
    let pad = m.padding as f64;
    let ch = m.cell_h as f64;
    let xr = x - pad;
    let origin = |c: usize| col_origin_px(c, m.advance, mode) as f64;
    let right_edge = origin(cols);
    let (col, right) = if xr < 0.0 {
        (0, false)
    } else if xr >= right_edge {
        (cols.saturating_sub(1), true)
    } else {
        let mut c = ((xr / m.advance as f64).floor().max(0.0) as usize).min(cols - 1);
        while c > 0 && origin(c) > xr {
            c -= 1;
        }
        while c + 1 < cols && origin(c + 1) <= xr {
            c += 1;
        }
        let width = origin(c + 1) - origin(c);
        (c, xr - origin(c) >= 0.5 * width)
    };
    let row = ((y - pad) / ch).floor() as i64;
    let inside = x >= pad && x < pad + right_edge && y >= pad && y < pad + rows as f64 * ch;
    Hit {
        col,
        row,
        right,
        inside,
    }
}

/// How far (device pixels) the pointer is beyond the cell area vertically: 0.0 inside, negative above
/// the top of row 0 (`y - pad`), positive below the bottom of the last row (`y - (pad + rows*cell_h)`).
pub fn past_edge_px(y: f64, m: &CellMetrics, rows: usize) -> f64 {
    let top = m.padding as f64;
    let bottom = top + rows as f64 * m.cell_h as f64;
    if y < top {
        y - top
    } else if y > bottom {
        y - bottom
    } else {
        0.0
    }
}

/// A CSS/logical length to device pixels at `scale` (no rounding): `v * scale`.
pub fn to_phys(v: f64, scale: f32) -> f64 {
    v * scale as f64
}

/// The eight scales every geometry test runs at.
pub const SCALES: [f32; 8] = [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 3.0];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glyph::{FakeGlyphs, GlyphSource};
    use crate::metrics::{BASE_FONT_PX, cell_metrics, cell_metrics_in, cell_origin_in};

    fn metrics_at(scale: f32) -> CellMetrics {
        let g = FakeGlyphs::new(8, 16);
        cell_metrics(&g.font_units(), scale)
    }

    fn pt_units() -> crate::metrics::FontUnits {
        crate::metrics::FontUnits {
            units_per_em: 1040.0,
            ascent: 1000.0,
            descent: -160.0,
            leading: 0.0,
            advance: 610.0,
        }
    }

    #[test]
    fn the_quarter_points_of_a_cell_pick_that_cell_and_its_half_at_every_scale() {
        for scale in SCALES {
            let m = metrics_at(scale);
            let pad = m.padding as f64;
            let cw = m.cell_w as f64;
            let ch = m.cell_h as f64;
            for (col, row) in [(0usize, 0usize), (3, 2), (79, 23)] {
                let y = pad + row as f64 * ch + ch / 2.0;
                let left = pad + col as f64 * cw + 0.25 * cw;
                let right = pad + col as f64 * cw + 0.75 * cw;
                let h = hit_cell(left, y, &m, 80, 24);
                assert_eq!(
                    h,
                    Hit {
                        col,
                        row: row as i64,
                        right: false,
                        inside: true
                    },
                    "scale {scale} cell ({col},{row}) left"
                );
                let h = hit_cell(right, y, &m, 80, 24);
                assert_eq!(
                    h,
                    Hit {
                        col,
                        row: row as i64,
                        right: true,
                        inside: true
                    },
                    "scale {scale} cell ({col},{row}) right"
                );
            }
        }
    }

    #[test]
    fn hit_testing_round_trips_in_both_modes() {
        for scale in SCALES {
            for mode in [CellRounding::Whole, CellRounding::Fractional] {
                let m = cell_metrics_in(&pt_units(), BASE_FONT_PX, scale, mode);
                let cols = 80;
                for col in 0..cols {
                    // A pixel one past the column's origin is inside that column.
                    let (x0, y0) = cell_origin_in(col, 1, &m, mode);
                    let h = hit_cell_in(x0 as f64 + 1.0, y0 as f64 + 1.0, &m, cols, 24, mode);
                    assert_eq!(h.col, col, "{mode:?} scale {scale} col {col}");
                    assert!(!h.right, "{mode:?} scale {scale} col {col} left half");
                    assert_eq!(h.row, 1, "{mode:?} scale {scale} col {col}");
                    assert!(h.inside, "{mode:?} scale {scale} col {col}");
                }
            }
        }
    }

    #[test]
    fn left_of_the_padding_is_column_zero_left_half() {
        for scale in SCALES {
            let m = metrics_at(scale);
            let h = hit_cell(0.0, m.padding as f64 + 1.0, &m, 80, 24);
            assert_eq!(h.col, 0, "scale {scale}");
            assert!(!h.right, "scale {scale}");
            assert!(!h.inside, "scale {scale}");
        }
    }

    #[test]
    fn right_of_the_last_column_is_the_last_column_right_half() {
        for scale in SCALES {
            let m = metrics_at(scale);
            let pad = m.padding as f64;
            let x = pad + 80.0 * m.cell_w as f64 + 50.0;
            let h = hit_cell(x, pad + 1.0, &m, 80, 24);
            assert_eq!(h.col, 79, "scale {scale}");
            assert!(h.right, "scale {scale}");
            assert!(!h.inside, "scale {scale}");
        }
    }

    #[test]
    fn rows_are_not_clamped() {
        let m = metrics_at(1.0);
        let pad = m.padding as f64;
        let ch = m.cell_h as f64;
        assert_eq!(hit_cell(pad, 0.0, &m, 80, 24).row, -1);
        assert_eq!(hit_cell(pad, pad + 24.0 * ch + 1.0, &m, 80, 24).row, 24);
        assert_eq!(hit_cell(pad, pad + 3.0 * ch, &m, 80, 24).row, 3);
    }

    #[test]
    fn past_edge_is_signed_distance() {
        let m = metrics_at(2.0);
        let pad = m.padding as f64;
        let ch = m.cell_h as f64;
        assert_eq!(past_edge_px(pad - 10.0, &m, 24), -10.0);
        assert_eq!(past_edge_px(pad + 5.0, &m, 24), 0.0);
        assert_eq!(past_edge_px(pad + 24.0 * ch + 7.0, &m, 24), 7.0);
    }

    #[test]
    fn to_phys_multiplies() {
        assert_eq!(to_phys(100.0, 2.25), 225.0);
    }
}
