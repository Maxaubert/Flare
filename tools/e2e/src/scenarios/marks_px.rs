//! The pixel maths of the indicator scenarios (M10 D21): where a mark sits on a tab, what colour its
//! pixels are, and how far a tab's rule or line runs. Pure functions over colours, so each rule is
//! held by a unit test; the scenarios feed them the snapshot's pixels.

use crate::pixels::Rgb;

/// The sum of the channel differences of two colours.
pub fn dist(a: Rgb, b: Rgb) -> i32 {
    (0..3)
        .map(|i| (i32::from(a[i]) - i32::from(b[i])).abs())
        .sum()
}

/// `#rrggbb` or `#rrggbbaa` as its three colour bytes; mid grey for anything else.
pub fn hex_rgb(hex: &str) -> Rgb {
    let h = hex.trim_start_matches('#');
    let byte = |i: usize| {
        h.get(i..i + 2)
            .and_then(|s| u8::from_str_radix(s, 16).ok())
            .unwrap_or(0x80)
    };
    if h.len() < 6 {
        return [0x80; 3];
    }
    [byte(0), byte(2), byte(4)]
}

/// Where `p` lies on the line from `ground` to `ink` (0 at the ground, 1 at the ink) and how far off
/// that line it sits (the sum of the channel gaps). A pixel of an antialiased ink-coloured stroke is
/// on the line at any coverage; a pixel of another colour is off it. `None` when ink and ground are
/// the same colour, so there is no line.
pub fn along(ground: Rgb, ink: Rgb, p: Rgb) -> Option<(f32, f32)> {
    let d: [f32; 3] = std::array::from_fn(|i| f32::from(ink[i]) - f32::from(ground[i]));
    let len2: f32 = d.iter().map(|v| v * v).sum();
    if len2 < 1.0 {
        return None;
    }
    let v: [f32; 3] = std::array::from_fn(|i| f32::from(p[i]) - f32::from(ground[i]));
    let t = v.iter().zip(&d).map(|(a, b)| a * b).sum::<f32>() / len2;
    let off: f32 = (0..3).map(|i| (v[i] - t * d[i]).abs()).sum();
    Some((t, off))
}

/// The best pixel of `pixels` for the `ink` over `ground`: the greatest `t` among those within `tol`
/// of the line, or `None` when none is.
pub fn best_ink(
    pixels: impl IntoIterator<Item = Rgb>,
    ground: Rgb,
    ink: Rgb,
    tol: f32,
) -> Option<f32> {
    pixels
        .into_iter()
        .filter_map(|p| along(ground, ink, p))
        .filter(|(_, off)| *off <= tol)
        .map(|(t, _)| t)
        .max_by(f32::total_cmp)
}

/// How many pixels from the start of `run` are within `tol` of `colour`: a rule's thickness when
/// `run` is the column of pixels down from a tab's top edge.
pub fn leading_run(run: impl IntoIterator<Item = Rgb>, colour: Rgb, tol: i32) -> usize {
    run.into_iter()
        .take_while(|p| dist(*p, colour) <= tol)
        .count()
}

/// The share of `row` (0..1) that differs from `ground` by more than `tol`: how much of a tab's
/// foot a line covers.
pub fn covered(row: &[Rgb], ground: Rgb, tol: i32) -> f32 {
    if row.is_empty() {
        return 0.0;
    }
    row.iter().filter(|p| dist(**p, ground) > tol).count() as f32 / row.len() as f32
}

/// The device rows a mark `css` px thick fills at scale `k`: whole device pixels, never less than one.
pub fn thick_rows(css: f32, k: f32) -> usize {
    ((css * k).round() as usize).max(1)
}

/// How many pixels up from the foot of `column` (its first pixel is the tab's last row) differ from
/// `ground` by more than `tol`: the thickness of a line along the tab's foot.
pub fn foot_thickness(column: impl IntoIterator<Item = Rgb>, ground: Rgb, tol: i32) -> usize {
    column
        .into_iter()
        .take_while(|p| dist(*p, ground) > tol)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GROUND: Rgb = [0x17, 0x17, 0x17];
    const INK: Rgb = [0xde, 0xda, 0xcf];
    const ACCENT: Rgb = [0xf6, 0xa2, 0x5b];

    #[test]
    fn a_hex_reads_its_three_bytes_and_ignores_the_alpha() {
        assert_eq!(hex_rgb("#f6a25bff"), ACCENT);
        assert_eq!(hex_rgb("#171717"), GROUND);
        assert_eq!(hex_rgb("nope"), [0x80; 3]);
    }

    #[test]
    fn a_half_covered_ink_pixel_is_on_the_line_half_way() {
        let half = [
            ((u16::from(GROUND[0]) + u16::from(INK[0])) / 2) as u8,
            ((u16::from(GROUND[1]) + u16::from(INK[1])) / 2) as u8,
            ((u16::from(GROUND[2]) + u16::from(INK[2])) / 2) as u8,
        ];
        let (t, off) = along(GROUND, INK, half).unwrap();
        assert!((t - 0.5).abs() < 0.02, "{t}");
        assert!(off < 4.0, "{off}");
    }

    #[test]
    fn the_accent_is_not_the_ink_even_where_it_is_as_far_from_the_ground() {
        assert_eq!(best_ink([ACCENT], GROUND, INK, 30.0), None);
        assert!(best_ink([GROUND, INK], GROUND, INK, 30.0).is_some_and(|t| t > 0.9));
        assert_eq!(best_ink([GROUND, GROUND], GROUND, INK, 30.0), Some(0.0));
    }

    #[test]
    fn the_best_ink_pixel_is_the_one_furthest_along_the_line() {
        let half = [0x7a, 0x78, 0x73];
        let best = best_ink([GROUND, half, INK], GROUND, INK, 30.0).unwrap();
        assert!((best - 1.0).abs() < 0.01, "{best}");
        assert_eq!(best_ink([ACCENT], GROUND, INK, 30.0), None);
    }

    #[test]
    fn an_ink_the_colour_of_the_ground_has_no_line_to_wear() {
        assert_eq!(along(GROUND, GROUND, INK), None);
        assert_eq!(best_ink([INK], GROUND, GROUND, 99.0), None);
    }

    #[test]
    fn a_rule_is_the_run_of_its_colour_from_the_top() {
        let col = [ACCENT, ACCENT, GROUND, ACCENT];
        assert_eq!(leading_run(col, ACCENT, 6), 2);
        assert_eq!(leading_run([GROUND, ACCENT], ACCENT, 6), 0);
    }

    #[test]
    fn a_foot_is_covered_by_the_share_of_pixels_off_the_ground() {
        let row = [GROUND, ACCENT, ACCENT, GROUND];
        assert!((covered(&row, GROUND, 30) - 0.5).abs() < 1e-6);
        assert_eq!(covered(&[], GROUND, 30), 0.0);
    }

    #[test]
    fn a_mark_is_its_css_thickness_in_whole_device_pixels() {
        assert_eq!(thick_rows(2.0, 1.0), 2);
        assert_eq!(thick_rows(2.0, 2.25), 5);
        assert_eq!(thick_rows(3.0, 3.0), 9);
        assert_eq!(thick_rows(2.0, 0.1), 1);
    }

    #[test]
    fn a_foot_line_is_the_run_off_the_ground_up_from_the_bottom() {
        let col = [ACCENT, ACCENT, ACCENT, GROUND, ACCENT];
        assert_eq!(foot_thickness(col, GROUND, 30), 3);
        assert_eq!(foot_thickness([GROUND], GROUND, 30), 0);
    }
}
