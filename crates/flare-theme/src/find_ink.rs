//! The find highlights, drawn from the worn theme (review #52). The cells' own foreground is drawn over
//! these fills, so each fill is held to the body floor against that foreground: a fixed dark grey under a
//! light theme's dark text read about 1.4:1.

use crate::ansi::{contrast_ratio, mix_hex, normalize_color};

/// WCAG body floor: the theme foreground on a match fill.
const FLOOR: f64 = 4.5;
/// Where a fill starts, as a share of the way from the ground to the foreground.
const MATCH_MIX: f64 = 0.26;
/// The line round a match.
const BORDER_MIX: f64 = 0.5;
const STEP: f64 = 0.1;

/// The four colours of a find (`#rrggbb`): every match, its line, the active match and its line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindInk {
    pub bg: String,
    pub border: String,
    pub active_bg: String,
    pub active_border: String,
}

/// `fill` moved toward the ground until the foreground clears the floor on it. The ground clears it by the
/// theme's own rule, so the walk always ends; the ground itself is the last resort.
fn held(fill: &str, fg: &str, bg: &str) -> String {
    let mut c = fill.to_string();
    for _ in 0..30 {
        if contrast_ratio(fg, &c) >= FLOOR {
            return c;
        }
        c = mix_hex(&c, bg, STEP);
    }
    if contrast_ratio(fg, &c) >= FLOOR {
        c
    } else {
        bg.to_string()
    }
}

/// The find colours for a ground, its foreground and its accent (the chrome's solid accent).
pub fn find_ink(background: &str, foreground: &str, accent: &str) -> FindInk {
    let bg = normalize_color(background, "#000000");
    let fg = normalize_color(foreground, "#ffffff");
    let accent = normalize_color(accent, "#5b5bd6");
    FindInk {
        bg: held(&mix_hex(&bg, &fg, MATCH_MIX), &fg, &bg),
        border: mix_hex(&bg, &fg, BORDER_MIX),
        active_bg: held(&accent, &fg, &bg),
        active_border: accent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chrome::chrome_tokens;
    use crate::edges::WindowEdges;
    use crate::preset::presets;
    use crate::resolve::resolve_preset;

    #[test]
    fn every_preset_reads_its_foreground_on_a_match_and_on_the_active_match() {
        for p in presets() {
            let t = resolve_preset(p);
            let chrome = chrome_tokens(&t, 1.0, p.accent, WindowEdges::Hairline, None, None);
            let accent = &chrome.vars["--p-accent-solid"];
            let ink = find_ink(&t.background, &t.foreground, accent);
            let fg = normalize_color(&t.foreground, "#ffffff");
            assert!(
                contrast_ratio(&fg, &ink.bg) >= FLOOR,
                "{} match {} on {}",
                p.id,
                fg,
                ink.bg
            );
            assert!(
                contrast_ratio(&fg, &ink.active_bg) >= FLOOR,
                "{} active {} on {}",
                p.id,
                fg,
                ink.active_bg
            );
        }
    }

    #[test]
    fn a_light_theme_no_longer_wears_pt_defaults_dark_grey() {
        let ink = find_ink("#fafafa", "#383a42", "#4078f2");
        assert_ne!(ink.bg, "#4a5160");
        assert!(contrast_ratio("#383a42", &ink.bg) >= FLOOR);
        // The match still stands off the ground.
        assert!(contrast_ratio(&ink.bg, "#fafafa") > 1.1);
    }
}
