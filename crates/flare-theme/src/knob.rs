//! The switch knob colour (Task 11). Source: PT `core/renderer/lib/switchKnob.ts` and its test.

use crate::ansi::contrast_ratio;
use crate::marks::NAME_FLIP;

/// The knob on a dark theme: the near-black the on-accent ink has always been.
pub const KNOB_DARK: &str = "#0b0b0f";
/// The knob on a light theme.
pub const KNOB_LIGHT: &str = "#ffffff";

/// The on switch's knob: the theme's ink (black on a dark ground, white on a light one), flipped only where
/// that ink reads under 2:1 on the ON track (PT `switchKnob`). Decided per colour, never per frame.
pub fn switch_knob(track: &str, dark_ground: bool) -> &'static str {
    let ink = if dark_ground { KNOB_DARK } else { KNOB_LIGHT };
    if contrast_ratio(ink, track) >= NAME_FLIP {
        return ink;
    }
    if dark_ground {
        KNOB_LIGHT
    } else {
        KNOB_DARK
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ansi::contrast_ratio;

    fn grey(n: i32) -> String {
        format!("#{n:02x}{n:02x}{n:02x}")
    }

    #[test]
    fn black_on_dark_white_on_light() {
        assert_eq!(switch_knob("#fe8f34", true), KNOB_DARK);
        assert_eq!(switch_knob("#d8ff26", true), KNOB_DARK);
        assert_eq!(switch_knob("#3a73a0", false), KNOB_LIGHT);
    }

    #[test]
    fn a_very_light_accent_on_a_light_theme_flips_to_black() {
        assert!(contrast_ratio(KNOB_LIGHT, "#cfe4ff") < 2.0);
        assert_eq!(switch_knob("#cfe4ff", false), KNOB_DARK);
    }

    #[test]
    fn a_very_dark_accent_on_a_dark_theme_flips_to_white() {
        assert!(contrast_ratio(KNOB_DARK, "#2a2c30") < 2.0);
        assert_eq!(switch_knob("#2a2c30", true), KNOB_LIGHT);
    }

    #[test]
    fn the_rule_is_under_2_to_1_for_the_knob() {
        let mut g = 0;
        while contrast_ratio(KNOB_DARK, &grey(g)) < 2.0 {
            g += 1;
        }
        assert_eq!(switch_knob(&grey(g), true), KNOB_DARK);
        assert_eq!(switch_knob(&grey(g - 1), true), KNOB_LIGHT);
    }
}
