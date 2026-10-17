//! OKLab conversions and the lightness lift (Task 7). Port of PT `core/renderer/lib/oklab.ts`.

use crate::ansi::{contrast_ratio, hex_to_rgb, luminance};
use crate::num::{clamp, hex_byte};

/// OKLab (L, a, b). L is 0 black .. 1 white.
pub type Lab = [f64; 3];

/// sRGB channel (0..1) to linear light (PT `lin`).
fn lin(v: f64) -> f64 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear light to sRGB channel (PT `unlin`).
fn unlin(v: f64) -> f64 {
    if v <= 0.0031308 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// A 6-digit hex to OKLab (PT `toLab`).
pub fn to_lab(hex: &str) -> Lab {
    let [r, g, b] = hex_to_rgb(hex).map(|c| lin(c / 255.0));
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    [
        0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
    ]
}

/// OKLab back to a lower-case `#rrggbb`, each channel clipped to the screen (PT `fromLab`).
pub fn from_lab([lightness, a, b]: Lab) -> String {
    let l = (lightness + 0.3963377774 * a + 0.2158037573 * b).powf(3.0);
    let m = (lightness - 0.1055613458 * a - 0.0638541728 * b).powf(3.0);
    let s = (lightness - 0.0894841775 * a - 1.291485548 * b).powf(3.0);
    let rgb = [
        4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
    ];
    let channel = |v: f64| hex_byte(clamp(unlin(v.max(0.0)), 0.0, 1.0) * 255.0);
    format!("#{}{}{}", channel(rgb[0]), channel(rgb[1]), channel(rgb[2]))
}

/// OKLab lightness, 0 black to 1 white.
pub fn ok_lightness(hex: &str) -> f64 {
    to_lab(hex)[0]
}

/// A colour at another lightness, its chroma scaled with the lightness (PT `atLightness`).
pub fn at_lightness(hex: &str, l: f64) -> String {
    let [l0, a, b] = to_lab(hex);
    let k = if l0 > 0.001 { l / l0 } else { 0.0 };
    from_lab([l, a * k, b * k])
}

/// A colour moved by lightness alone until it reads at `floor` on every ground (PT `liftToFloor`).
/// `None` when no lightness clears the floor.
pub fn lift_to_floor(hex: &str, grounds: &[&str], floor: f64) -> Option<String> {
    let reads = |c: &str| grounds.iter().all(|g| contrast_ratio(c, g) >= floor);
    if reads(hex) {
        return Some(hex.to_string());
    }
    let up = luminance(grounds[0]) < 0.18;
    let l0 = ok_lightness(hex);
    for i in 1..=100 {
        let t = f64::from(i) / 100.0;
        let l = if up {
            l0 + (1.0 - l0) * t
        } else {
            l0 * (1.0 - t)
        };
        let c = at_lightness(hex, l);
        if reads(&c) {
            return Some(c);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ansi::{contrast_ratio, hex_to_rgb};
    use crate::testutil::fixture;

    #[test]
    fn oracle_oklab_vectors() {
        let v = fixture("oklab_vectors.json");
        let rows = v.as_array().expect("an array");
        assert_eq!(rows.len(), 8);
        for row in rows {
            let hex = row["hex"].as_str().expect("hex");
            let want = row["lab"].as_array().expect("lab");
            let got = to_lab(hex);
            for (i, w) in want.iter().enumerate() {
                let w = w.as_f64().expect("a number");
                assert!(
                    (got[i] - w).abs() <= 1e-9,
                    "{hex} lab[{i}]: {} vs {w}",
                    got[i]
                );
            }
            assert_eq!(
                from_lab(to_lab(hex)),
                row["back"].as_str().expect("back"),
                "{hex} back"
            );
            assert_eq!(
                lift_to_floor(hex, &["#121212"], 3.0).as_deref(),
                row["lift"].as_str(),
                "{hex} lift"
            );
            assert_eq!(
                lift_to_floor(hex, &["#f6f4ee"], 3.0).as_deref(),
                row["liftLight"].as_str(),
                "{hex} liftLight"
            );
        }
    }

    #[test]
    fn an_accent_too_dark_to_see_is_lifted_to_a_red_not_a_grey() {
        let c = lift_to_floor("#140707", &["#121212"], 3.0).expect("a lift");
        assert!(contrast_ratio(&c, "#121212") >= 3.0, "{c}");
        let [r, g, b] = hex_to_rgb(&c);
        assert!(r - g > 50.0 && r - b > 50.0, "{c} is not a red");
    }

    #[test]
    fn a_pick_that_already_reads_is_left_alone() {
        assert_eq!(
            lift_to_floor("#ff0000", &["#121212"], 3.0).as_deref(),
            Some("#ff0000")
        );
    }

    #[test]
    fn a_pale_pick_on_a_light_ground_is_darkened() {
        let c = lift_to_floor("#ffd0d0", &["#f6f4ee"], 3.0).expect("a lift");
        assert!(contrast_ratio(&c, "#f6f4ee") >= 3.0, "{c}");
        let [r, g, _] = hex_to_rgb(&c);
        assert!(r - g > 20.0, "{c} is not a pink");
    }
}
