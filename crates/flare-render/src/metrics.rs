//! Cell metrics: how a font's units become cells at a given scale, and where cells sit.
//!
//! The cell-size policy is chosen in exactly one place: [`CELL_ROUNDING`]. The public functions read it;
//! the `*_in` variants take the mode explicitly so both policies stay under test.

/// Base font size in logical pixels (Prism Terminal's `TERM_BASE_FONT_PX`).
pub const BASE_FONT_PX: f32 = 13.0;
/// Padding around the grid in logical pixels (Prism Terminal's `p-1` on the terminal panel).
pub const TERM_PADDING_LOGICAL: f32 = 4.0;

/// How a cell advance is turned into pixels. `Whole` rounds every advance to whole device pixels;
/// `Fractional` keeps the exact advance (Prism Terminal's 7.625 x 15 at 100%).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellRounding {
    Whole,
    Fractional,
}

/// THE switch point for the cell-size policy (owner question Q3, default taken: whole pixels).
/// Changing it changes every cell position, the hit test and the grid dimensions.
pub const CELL_ROUNDING: CellRounding = CellRounding::Whole;

/// The font's vertical and horizontal metrics, in font units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontUnits {
    pub units_per_em: f32,
    pub ascent: f32,
    /// Negative in font units, as in OpenType.
    pub descent: f32,
    pub leading: f32,
    pub advance: f32,
}

/// Cell geometry at one scale. `advance` is the advance the grid uses (under `Whole` it equals
/// `cell_w as f32`; under `Fractional` it is the exact advance and `cell_w` is its ceiling, the painted width).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellMetrics {
    pub cell_w: u32,
    pub cell_h: u32,
    pub baseline: u32,
    pub underline_y: u32,
    pub underline_h: u32,
    pub padding: u32,
    pub advance: f32,
}

/// Font size in physical pixels for a base size in logical pixels at `scale`.
pub fn font_px_for(base_px: f32, scale: f32) -> f32 {
    base_px * scale
}

/// Font size in physical pixels at `scale`, at the default base size.
pub fn font_px(scale: f32) -> f32 {
    font_px_for(BASE_FONT_PX, scale)
}

/// The cell advance in device pixels for a base size (logical px) at `scale`, in `mode`.
/// `Whole` rounds to a whole pixel; `Fractional` returns the exact advance.
pub fn cell_advance_px(u: &FontUnits, base_px: f32, scale: f32, mode: CellRounding) -> f32 {
    let exact = u.advance * font_px_for(base_px, scale) / u.units_per_em;
    match mode {
        CellRounding::Whole => exact.round().max(1.0),
        CellRounding::Fractional => exact.max(1.0),
    }
}

/// The pixel offset (from the grid's left padding edge) of column `col`, for an advance in `mode`.
/// `Whole`: `col * round(advance)`. `Fractional`: `round(col * advance)`.
pub fn col_origin_px(col: usize, advance: f32, mode: CellRounding) -> i32 {
    match mode {
        CellRounding::Whole => col as i32 * advance.round() as i32,
        CellRounding::Fractional => (col as f32 * advance).round() as i32,
    }
}

/// Derives the cell geometry from font units at `scale` (1.0 = 100%, 2.25 = 225%), default base.
pub fn cell_metrics(u: &FontUnits, scale: f32) -> CellMetrics {
    cell_metrics_at(u, BASE_FONT_PX, scale)
}

/// Derives the cell geometry from font units for a base size (logical px) at `scale`, under `CELL_ROUNDING`.
pub fn cell_metrics_at(u: &FontUnits, base_px: f32, scale: f32) -> CellMetrics {
    cell_metrics_in(u, base_px, scale, CELL_ROUNDING)
}

/// [`cell_metrics_at`] under an explicit cell-rounding mode.
pub fn cell_metrics_in(u: &FontUnits, base_px: f32, scale: f32, mode: CellRounding) -> CellMetrics {
    let px = font_px_for(base_px, scale);
    let k = px / u.units_per_em;
    let advance = cell_advance_px(u, base_px, scale, mode);
    let cell_w = advance.ceil().max(1.0) as u32;
    let asc = u.ascent * k;
    let desc = -u.descent * k;
    let lead = u.leading * k;
    let cell_h = (asc + desc + lead).ceil().max(1.0) as u32;
    let baseline = (lead / 2.0 + asc).round() as u32;
    let underline_h = scale.round().max(1.0) as u32;
    let underline_y = baseline + ((cell_h as f32 - baseline as f32) * 0.4).round().max(1.0) as u32;
    let padding = (TERM_PADDING_LOGICAL * scale).round() as u32;
    CellMetrics {
        cell_w,
        cell_h,
        baseline,
        underline_y,
        underline_h,
        padding,
        advance,
    }
}

/// How many whole cells fit in a `width_px` by `height_px` area, as `(cols, rows)`, each at least 1,
/// under `CELL_ROUNDING`.
pub fn grid_dims(width_px: u32, height_px: u32, m: &CellMetrics) -> (usize, usize) {
    grid_dims_in(width_px, height_px, m, CELL_ROUNDING)
}

/// [`grid_dims`] under an explicit mode. A column count `n` fits when the pixel origin of column `n`
/// is inside the padded width, so `Whole` gives `floor(width / cell_w)` as before.
pub fn grid_dims_in(
    width_px: u32,
    height_px: u32,
    m: &CellMetrics,
    mode: CellRounding,
) -> (usize, usize) {
    let inset = 2 * m.padding;
    let avail = width_px.saturating_sub(inset) as i32;
    let mut cols = (avail as f32 / m.advance) as usize;
    while cols > 0 && col_origin_px(cols, m.advance, mode) > avail {
        cols -= 1;
    }
    while col_origin_px(cols + 1, m.advance, mode) <= avail {
        cols += 1;
    }
    let cols = cols.max(1);
    let rows = (height_px.saturating_sub(inset) / m.cell_h).max(1) as usize;
    (cols, rows)
}

/// Top-left pixel of the cell at `col`, `row`, including the padding, under `CELL_ROUNDING`.
pub fn cell_origin(col: usize, row: usize, m: &CellMetrics) -> (i32, i32) {
    cell_origin_in(col, row, m, CELL_ROUNDING)
}

/// [`cell_origin`] under an explicit mode.
pub fn cell_origin_in(col: usize, row: usize, m: &CellMetrics, mode: CellRounding) -> (i32, i32) {
    let x = m.padding as i32 + col_origin_px(col, m.advance, mode);
    let y = m.padding as i32 + row as i32 * m.cell_h as i32;
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cascadia() -> FontUnits {
        FontUnits {
            units_per_em: 2048.0,
            ascent: 1900.0,
            descent: -480.0,
            leading: 0.0,
            advance: 1200.0,
        }
    }

    /// Prism Terminal's measured cell at 100%: advance 7.625, height 15 (the fixture's units give exactly 7.625).
    fn pt_measured() -> FontUnits {
        FontUnits {
            units_per_em: 1040.0,
            ascent: 1000.0,
            descent: -160.0,
            leading: 0.0,
            advance: 610.0,
        }
    }

    #[test]
    fn whole_is_the_default_and_gives_8_by_16_at_100_percent() {
        assert_eq!(CELL_ROUNDING, CellRounding::Whole);
        let m = cell_metrics(&cascadia(), 1.0);
        assert_eq!((m.cell_w, m.cell_h), (8, 16));
        assert_eq!(m.advance, 8.0);
    }

    #[test]
    fn fractional_gives_7_625_at_100_percent_with_origins_0_8_15_23() {
        let u = pt_measured();
        let adv = cell_advance_px(&u, BASE_FONT_PX, 1.0, CellRounding::Fractional);
        assert_eq!(adv, 7.625);
        let m = cell_metrics_in(&u, BASE_FONT_PX, 1.0, CellRounding::Fractional);
        assert_eq!(
            (m.cell_w, m.cell_h),
            (8, 15),
            "painted width is ceil(7.625)"
        );
        let origins: Vec<i32> = (0..4)
            .map(|c| col_origin_px(c, adv, CellRounding::Fractional))
            .collect();
        assert_eq!(origins, vec![0, 8, 15, 23]);
        let (x, _) = cell_origin_in(3, 0, &m, CellRounding::Fractional);
        assert_eq!(x, m.padding as i32 + 23);
    }

    #[test]
    fn whole_origins_are_whole_multiples_of_the_rounded_advance() {
        let adv = cell_advance_px(&cascadia(), BASE_FONT_PX, 1.0, CellRounding::Whole);
        let origins: Vec<i32> = (0..4)
            .map(|c| col_origin_px(c, adv, CellRounding::Whole))
            .collect();
        assert_eq!(origins, vec![0, 8, 16, 24]);
    }

    #[test]
    fn grid_dims_for_1200x800() {
        let m1 = cell_metrics(&cascadia(), 1.0);
        assert_eq!(grid_dims(1200, 800, &m1), (149, 49));
        let m225 = cell_metrics(&cascadia(), 2.25);
        assert_eq!(grid_dims(1200, 800, &m225), (69, 23));
    }

    #[test]
    fn grid_dims_never_zero() {
        let m = cell_metrics(&cascadia(), 1.0);
        assert_eq!(grid_dims(0, 0, &m), (1, 1));
    }

    #[test]
    fn cell_origin_includes_padding() {
        let m = cell_metrics(&cascadia(), 1.0);
        assert_eq!(cell_origin(2, 3, &m), (4 + 16, 4 + 48));
    }

    #[test]
    fn grid_dims_match_in_both_modes_at_the_8_scales() {
        // Both modes describe the same area, so their column counts differ by at most one, and
        // each count is the largest n whose column origin fits in the padded width.
        for scale in crate::hit::SCALES {
            for mode in [CellRounding::Whole, CellRounding::Fractional] {
                let u = pt_measured();
                let m = cell_metrics_in(&u, BASE_FONT_PX, scale, mode);
                let (w, h) = (1000u32, 640u32);
                let (cols, rows) = grid_dims_in(w, h, &m, mode);
                let avail = (w - 2 * m.padding) as i32;
                assert!(
                    col_origin_px(cols, m.advance, mode) <= avail,
                    "{mode:?} {scale}"
                );
                assert!(
                    col_origin_px(cols + 1, m.advance, mode) > avail,
                    "{mode:?} {scale}"
                );
                assert_eq!(
                    rows,
                    ((h - 2 * m.padding) / m.cell_h) as usize,
                    "{mode:?} {scale}"
                );
            }
        }
    }

    #[test]
    fn grid_dims_at_100_percent_differ_by_the_rounding_the_switch_chooses() {
        // 1000 px wide, 8 px padding each side: 992 px for columns. Whole cells are 8 px (124 columns),
        // fractional cells are 7.625 px (130 columns, the 5% that Prism Terminal's grid has over Whole).
        let u = pt_measured();
        let whole = cell_metrics_in(&u, BASE_FONT_PX, 1.0, CellRounding::Whole);
        let frac = cell_metrics_in(&u, BASE_FONT_PX, 1.0, CellRounding::Fractional);
        assert_eq!(grid_dims_in(1000, 640, &whole, CellRounding::Whole).0, 124);
        assert_eq!(
            grid_dims_in(1000, 640, &frac, CellRounding::Fractional).0,
            130
        );
    }

    #[test]
    fn cascadia_metrics_at_each_scale() {
        // scale -> cell_w, cell_h, baseline, underline_y, underline_h, padding
        let table: [(f32, u32, u32, u32, u32, u32, u32); 6] = [
            (1.0, 8, 16, 12, 14, 1, 4),
            (1.25, 10, 19, 15, 17, 1, 5),
            (1.5, 11, 23, 18, 20, 2, 6),
            (1.75, 13, 27, 21, 23, 2, 7),
            (2.0, 15, 31, 24, 27, 2, 8),
            (2.25, 17, 34, 27, 30, 2, 9),
        ];
        for (scale, cell_w, cell_h, baseline, underline_y, underline_h, padding) in table {
            let m = cell_metrics(&cascadia(), scale);
            assert_eq!(
                (
                    m.cell_w,
                    m.cell_h,
                    m.baseline,
                    m.underline_y,
                    m.underline_h,
                    m.padding
                ),
                (cell_w, cell_h, baseline, underline_y, underline_h, padding),
                "scale {scale}"
            );
            assert_eq!(m.advance, cell_w as f32, "scale {scale}");
        }
    }

    #[test]
    fn font_px_scales_base() {
        assert_eq!(font_px(1.0), 13.0);
        assert_eq!(font_px(2.0), 26.0);
        assert_eq!(font_px_for(16.0, 1.5), 24.0);
    }
}
