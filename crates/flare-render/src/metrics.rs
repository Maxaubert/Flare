//! Cell metrics: how a font's units become whole-pixel cells at a given scale, and where cells sit.

/// Base font size in logical pixels (Prism Terminal's `TERM_BASE_FONT_PX`).
pub const BASE_FONT_PX: f32 = 13.0;
/// Padding around the grid in logical pixels (Prism Terminal's `p-1` on the terminal panel).
pub const TERM_PADDING_LOGICAL: f32 = 4.0;

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

/// Whole-pixel cell geometry at one scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellMetrics {
    pub cell_w: u32,
    pub cell_h: u32,
    pub baseline: u32,
    pub underline_y: u32,
    pub underline_h: u32,
    pub padding: u32,
}

/// Font size in physical pixels for a base size in logical pixels at `scale`.
pub fn font_px_for(base_px: f32, scale: f32) -> f32 {
    base_px * scale
}

/// Font size in physical pixels at `scale`, at the default base size.
pub fn font_px(scale: f32) -> f32 {
    font_px_for(BASE_FONT_PX, scale)
}

/// Derives the cell geometry from font units at `scale` (1.0 = 100%, 2.25 = 225%), default base.
pub fn cell_metrics(u: &FontUnits, scale: f32) -> CellMetrics {
    cell_metrics_at(u, BASE_FONT_PX, scale)
}

/// Derives the cell geometry from font units for a base size (logical px) at `scale`.
pub fn cell_metrics_at(u: &FontUnits, base_px: f32, scale: f32) -> CellMetrics {
    let px = font_px_for(base_px, scale);
    let k = px / u.units_per_em;
    let cell_w = (u.advance * k).round().max(1.0) as u32;
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
    }
}

/// How many whole cells fit in a `width_px` by `height_px` area, as `(cols, rows)`, each at least 1.
pub fn grid_dims(width_px: u32, height_px: u32, m: &CellMetrics) -> (usize, usize) {
    let inset = 2 * m.padding;
    let cols = (width_px.saturating_sub(inset) / m.cell_w).max(1) as usize;
    let rows = (height_px.saturating_sub(inset) / m.cell_h).max(1) as usize;
    (cols, rows)
}

/// Top-left pixel of the cell at `col`, `row`, including the padding.
pub fn cell_origin(col: usize, row: usize, m: &CellMetrics) -> (i32, i32) {
    let x = m.padding as i32 + col as i32 * m.cell_w as i32;
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
                m,
                CellMetrics {
                    cell_w,
                    cell_h,
                    baseline,
                    underline_y,
                    underline_h,
                    padding,
                },
                "scale {scale}"
            );
        }
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
    fn font_px_scales_base() {
        assert_eq!(font_px(1.0), 13.0);
        assert_eq!(font_px(2.0), 26.0);
        assert_eq!(font_px_for(16.0, 1.5), 24.0);
    }
}
