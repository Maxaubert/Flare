//! See-through default levels and level to alpha (Task 11b). Source: PT `core/renderer/lib/seeThrough.ts`,
//! `termLook.ts` (`seeThroughLevel`, `termGroundAlpha`, `paintsAlpha`) and their tests.

use crate::ansi::luminance;
use crate::colour::opaque;
use crate::ground::LIGHT_GROUND;
use crate::num::{clamp, js_round};

/// The dark default see-through alpha, 0xb9 (PT `SEE_THROUGH_ALPHA.dark`).
pub const SEE_THROUGH_ALPHA_DARK: f64 = 185.0 / 255.0;
/// The light default see-through alpha, 0xd1.
pub const SEE_THROUGH_ALPHA_LIGHT: f64 = 209.0 / 255.0;
/// The highest alpha a ground may carry while see-through is on, 0xf2 (PT `SEE_THROUGH_MAX`).
pub const SEE_THROUGH_MAX: f64 = 242.0 / 255.0;
/// The top of the see-through slider, in percent.
pub const SEE_THROUGH_TOP: u8 = 70;
/// The 30 percent alpha floor, 0x4d (PT `GROUND_ALPHA_MIN`).
pub const GROUND_ALPHA_MIN: f64 = 77.0 / 255.0;

/// The default see-through for a ground, light or dark MEASURED from its opaque colour; junk reads dark.
pub fn see_through_alpha(ground: &str) -> f64 {
    if luminance(&opaque(ground, "#000000")) > LIGHT_GROUND {
        SEE_THROUGH_ALPHA_LIGHT
    } else {
        SEE_THROUGH_ALPHA_DARK
    }
}

/// A stored slider level: None, empty, NaN or junk reads 0; else clamped to 0..=70 and rounded.
pub fn see_through_level(raw: Option<&str>) -> u8 {
    let n = match raw.map(str::trim) {
        None | Some("") => return 0,
        Some(s) => match s.parse::<f64>() {
            Ok(n) if n.is_finite() => n,
            _ => return 0,
        },
    };
    clamp(js_round(n), 0.0, f64::from(SEE_THROUGH_TOP)) as u8
}

/// The alpha a level paints: `1 - level / 100`.
pub fn ground_alpha(level: u8) -> f64 {
    1.0 - f64::from(level) / 100.0
}

/// PT's old rule, kept for the M7 import: with see-through on, an opaque alpha takes the default for
/// the ground; otherwise the alpha is floored at 30 percent.
pub fn paints_alpha(raw: f64, ground: &str, acrylic_on: bool) -> f64 {
    if acrylic_on && js_round(raw * 255.0) >= 255.0 {
        see_through_alpha(ground)
    } else {
        GROUND_ALPHA_MIN.max(raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_levels_are_the_references_two_as_they_paint() {
        assert_eq!(js_round(SEE_THROUGH_ALPHA_DARK * 255.0), 185.0);
        assert_eq!(js_round(SEE_THROUGH_ALPHA_LIGHT * 255.0), 209.0);
    }

    #[test]
    fn light_is_measured_above_luminance_0_4() {
        assert_eq!(see_through_alpha("#0b0b0f"), SEE_THROUGH_ALPHA_DARK);
        assert_eq!(see_through_alpha("#ffffff"), SEE_THROUGH_ALPHA_LIGHT);
        assert_eq!(see_through_alpha("#aaaaaa"), SEE_THROUGH_ALPHA_LIGHT);
        assert_eq!(see_through_alpha("#a9a9a9"), SEE_THROUGH_ALPHA_DARK);
    }

    #[test]
    fn a_see_through_colour_reads_by_its_own_opaque_colour() {
        assert_eq!(see_through_alpha("#ffffff20"), SEE_THROUGH_ALPHA_LIGHT);
        assert_eq!(see_through_alpha("#00000020"), SEE_THROUGH_ALPHA_DARK);
    }

    #[test]
    fn an_unparseable_ground_reads_as_dark() {
        assert_eq!(see_through_alpha("soup"), SEE_THROUGH_ALPHA_DARK);
    }

    #[test]
    fn the_cap_sits_below_opaque_and_above_both_defaults() {
        assert_eq!(js_round(SEE_THROUGH_MAX * 255.0), 242.0);
        const { assert!(SEE_THROUGH_MAX > SEE_THROUGH_ALPHA_LIGHT && SEE_THROUGH_MAX < 1.0) };
    }

    #[test]
    fn the_slider_level_is_clamped_and_junk_reads_zero() {
        assert_eq!(see_through_level(None), 0);
        assert_eq!(see_through_level(Some("")), 0);
        assert_eq!(see_through_level(Some("soup")), 0);
        assert_eq!(see_through_level(Some("NaN")), 0);
        assert_eq!(see_through_level(Some("35")), 35);
        assert_eq!(see_through_level(Some("500")), 70);
        assert_eq!(see_through_level(Some("-4")), 0);
        assert_eq!(see_through_level(Some("12.6")), 13);
    }

    #[test]
    fn ground_alpha_is_one_minus_the_level() {
        assert!((ground_alpha(0) - 1.0).abs() < 1e-12);
        assert!((ground_alpha(70) - 0.3).abs() < 1e-12);
        assert!((ground_alpha(30) - 0.7).abs() < 1e-12);
    }

    #[test]
    fn paints_alpha_is_the_old_rule_the_slider_move_reads() {
        assert_eq!(paints_alpha(1.0, "#000000", false), 1.0);
        assert_eq!(
            paints_alpha(1.0, "#000000", true),
            f64::from(0xb9u8) / 255.0
        );
        assert_eq!(
            paints_alpha(1.0, "#ffffff", true),
            f64::from(0xd1u8) / 255.0
        );
        assert_eq!(
            paints_alpha(f64::from(0x80u8) / 255.0, "#ffffff", true),
            f64::from(0x80u8) / 255.0
        );
        assert_eq!(
            paints_alpha(f64::from(0x05u8) / 255.0, "#000000", true),
            f64::from(0x4du8) / 255.0
        );
    }
}
