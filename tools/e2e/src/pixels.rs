//! Pixel and PNG checks for the snapshots (Tasks 20 and 72).
//!
//! Colours are plain `[u8; 3]` sRGB. The luminance and contrast are the WCAG 2.x formulas, so a
//! contrast figure here means the same thing it means in a design review.

/// One sRGB colour.
pub type Rgb = [u8; 3];

/// A box in device pixels: x, y, width, height.
pub type Bbox = (u32, u32, u32, u32);

/// The relative luminance of `c`, 0.0 for black to 1.0 for white (WCAG 2.x).
pub fn lum(c: Rgb) -> f64 {
    let lin = |v: u8| {
        let s = f64::from(v) / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
}

/// The WCAG contrast ratio of two colours: 1.0 for none, 21.0 for black on white.
pub fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (la, lb) = (lum(a), lum(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// `a` moved toward `b` by `t` (0.0 is `a`, 1.0 is `b`), channel by channel, rounded.
pub fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let ch = |i: usize| {
        let v = f64::from(a[i]) + (f64::from(b[i]) - f64::from(a[i])) * t;
        u8::try_from(v.round() as i64).unwrap_or(0)
    };
    [ch(0), ch(1), ch(2)]
}

/// The box that holds every pixel of a `w` by `h` image whose colour `keep` accepts, read through
/// `at(x, y)`. None when no pixel is accepted.
pub fn bbox_where(
    w: u32,
    h: u32,
    at: impl Fn(u32, u32) -> Rgb,
    keep: impl Fn(Rgb) -> bool,
) -> Option<Bbox> {
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
    let mut any = false;
    for y in 0..h {
        for x in 0..w {
            if keep(at(x, y)) {
                any = true;
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    any.then(|| (x0, y0, x1 - x0 + 1, y1 - y0 + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn black_and_white_have_the_extreme_luminances() {
        assert_eq!(lum([0, 0, 0]), 0.0);
        assert!((lum([255, 255, 255]) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_grey_below_the_linear_knee_is_linear() {
        // 0x0a is below the 0.03928 knee: its luminance is the linear value over 12.92.
        let s = 10.0 / 255.0;
        assert!((lum([10, 10, 10]) - s / 12.92).abs() < 1e-9);
    }

    #[test]
    fn black_on_white_is_twenty_one_to_one_in_either_order() {
        assert!((contrast([0, 0, 0], [255, 255, 255]) - 21.0).abs() < 1e-9);
        assert!((contrast([255, 255, 255], [0, 0, 0]) - 21.0).abs() < 1e-9);
    }

    #[test]
    fn a_colour_against_itself_is_one_to_one() {
        assert!((contrast([40, 90, 200], [40, 90, 200]) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn mix_walks_from_a_to_b_and_clamps_t() {
        assert_eq!(mix([0, 0, 0], [200, 100, 50], 0.0), [0, 0, 0]);
        assert_eq!(mix([0, 0, 0], [200, 100, 50], 1.0), [200, 100, 50]);
        assert_eq!(mix([0, 0, 0], [200, 100, 50], 0.5), [100, 50, 25]);
        assert_eq!(mix([0, 0, 0], [200, 100, 50], 2.0), [200, 100, 50]);
        assert_eq!(mix([0, 0, 0], [200, 100, 50], -1.0), [0, 0, 0]);
    }

    #[test]
    fn bbox_holds_exactly_the_accepted_pixels() {
        // A 6 by 5 image, the accent (red) painted at x 2..=3 and y 1..=3.
        let at = |x: u32, y: u32| {
            if (2..=3).contains(&x) && (1..=3).contains(&y) {
                [254, 0, 0]
            } else {
                [20, 20, 20]
            }
        };
        let keep = |c: Rgb| c[0] > 200;
        assert_eq!(bbox_where(6, 5, at, keep), Some((2, 1, 2, 3)));
    }

    #[test]
    fn bbox_of_one_pixel_is_one_by_one_and_none_is_none() {
        let at = |x: u32, y: u32| {
            if x == 4 && y == 0 {
                [1, 2, 3]
            } else {
                [0, 0, 0]
            }
        };
        assert_eq!(bbox_where(5, 1, at, |c| c == [1, 2, 3]), Some((4, 0, 1, 1)));
        assert_eq!(bbox_where(5, 1, |_, _| [0, 0, 0], |c| c[0] > 0), None);
    }
}
