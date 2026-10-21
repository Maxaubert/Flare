//! The link colour (Task 11b). Source: PT `core/renderer/lib/termLinks.ts` (`linkColor`) and its tests.

use crate::ansi::{contrast_ratio, ensure_contrast, mix_hex, normalize_color};

/// The link blue, PT's `LINK_BLUE`.
pub const LINK_BLUE: &str = "#79a7d8";

/// WCAG floor for link text.
const FLOOR: f64 = 4.5;

/// The colour a link is drawn in on this ground: the link blue where it reads, moved to clear the floor,
/// and tinted toward the blue when the tint still reads better against the foreground.
pub fn link_colour(background: &str, foreground: &str) -> String {
    let bg = normalize_color(background, "#000000");
    let fg = normalize_color(foreground, "#ffffff");
    let moved = ensure_contrast(LINK_BLUE, &bg, FLOOR);
    if moved == LINK_BLUE || contrast_ratio(&moved, &fg) >= 1.25 {
        return moved;
    }
    let tinted = mix_hex(&moved, LINK_BLUE, 0.35);
    if contrast_ratio(&tinted, &bg) >= FLOOR {
        tinted
    } else {
        moved
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ansi::contrast_ratio;
    use crate::preset::presets;
    use crate::resolve::resolve_preset;
    use crate::testutil::fixture;

    fn channels(hex: &str) -> (u32, u32, u32) {
        let n = u32::from_str_radix(&hex[1..], 16).unwrap();
        ((n >> 16) & 255, (n >> 8) & 255, n & 255)
    }

    #[test]
    fn the_link_blue_where_that_blue_reads() {
        assert_eq!(link_colour("#000000", "#e6e6e6"), LINK_BLUE);
    }

    #[test]
    fn it_moves_on_a_light_ground_and_stays_a_blue() {
        let c = link_colour("#f4f4f4", "#24292e");
        assert_ne!(c, LINK_BLUE);
        assert!(contrast_ratio(&c, "#f4f4f4") >= 4.5);
        let (r, _, b) = channels(&c);
        assert!(b > r, "{c} should have more blue than red");
    }

    #[test]
    fn it_clears_the_floor_on_a_ground_that_is_itself_blue() {
        for bg in ["#142033", "#1e3a8a", "#3b82f6", "#60a5fa"] {
            assert!(
                contrast_ratio(&link_colour(bg, "#ffffff"), bg) >= 4.5,
                "{bg}"
            );
        }
    }

    #[test]
    fn it_survives_a_ground_that_is_not_a_plain_hex() {
        assert!(contrast_ratio(&link_colour("rgba(0,0,0,0.5)", "#fff"), "#000000") >= 4.5);
    }

    #[test]
    fn a_link_reads_on_every_themes_own_ground() {
        for p in presets() {
            let t = resolve_preset(p);
            let c = link_colour(&t.background, &t.foreground);
            assert!(contrast_ratio(&c, &t.background) >= 4.5, "{}: {c}", p.id);
        }
    }

    #[test]
    fn all_39_links_equal_the_oracle() {
        let want = fixture("link_colours.json");
        let want = want.as_object().unwrap();
        assert_eq!(want.len(), 39);
        for p in presets() {
            let t = resolve_preset(p);
            let got = link_colour(&t.background, &t.foreground);
            assert_eq!(want[p.id].as_str().unwrap(), got, "{}", p.id);
        }
    }
}
