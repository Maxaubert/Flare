//! Agent colours that follow the theme (Task 13). Source: PT `src/renderer/src/lib/agentColors.ts` and its test,
//! and `core/renderer/lib/agentColors.ts` (the two constants). Marks are drawn in M6; this module owns the
//! theme-derived colours.

use crate::ansi::{contrast_ratio, ensure_contrast, hex_to_rgb, normalize_color};
use crate::chrome::chrome_tokens;
use crate::colour::{parse_colour, rgb_to_hsl};
use crate::edges::WindowEdges;
use crate::resolve::TermTheme;

/// The question mark's blue (PT `QUESTION_BLUE`).
pub const QUESTION_BLUE: &str = "#3b82f6";
/// The Failed mark's red where the theme names none (PT `FAILED_RED`).
pub const FAILED_RED: &str = "#ef4444";
/// The fallback green for Finished (PT `FALLBACK_DONE`).
const FALLBACK_DONE: &str = "#22c55e";
/// The last resort for Failed, a magenta, for a palette whose reds are all taken by its accent.
const FALLBACK_FAIL_ALT: &str = "#d946ef";
/// Every mark moves to at least 3:1 on the ground (PT `FLOOR`).
const FLOOR: f64 = 3.0;

#[derive(Clone, Debug, PartialEq)]
pub struct AgentColours {
    pub working: String,
    pub finished: String,
    pub question: String,
    pub failed: String,
}

/// `window_background` / `window_accent` are Settings' picks; `None` (or an empty string) follows the theme.
pub fn theme_agent_colours(
    theme: &TermTheme,
    preset_accent: Option<&str>,
    window_background: Option<&str>,
    window_accent: Option<&str>,
) -> AgentColours {
    // What the chrome REALLY wears: the picked background replaces the theme's own, as PT's `paintChrome` does.
    let mut theme = theme.clone();
    if let Some(bg) = window_background.filter(|b| !b.is_empty()) {
        theme.background = bg.to_string();
    }
    // The accent as a LINE, never see-through: the indicator that follows the theme is opaque.
    let vars = chrome_tokens(
        &theme,
        1.0,
        preset_accent,
        WindowEdges::Hairline,
        window_accent,
        None,
    )
    .vars;
    let bg = vars.get("--p-bg-solid").cloned().unwrap_or_default();
    let working = vars.get("--p-accent-solid").cloned().unwrap_or_default();

    let green = ensure_contrast(
        &normalize_color(theme.green().unwrap_or(""), FALLBACK_DONE),
        &bg,
        FLOOR,
    );
    // Near-identical colours measure about 1:1 against each other.
    let finished = if contrast_ratio(&green, &working) < 1.15 {
        ensure_contrast(FALLBACK_DONE, &bg, FLOOR)
    } else {
        green
    };
    let question = ensure_contrast(QUESTION_BLUE, &bg, FLOOR);

    // Failed is the theme's red, then the plain red, the theme's magenta, the last resort. Told apart by hue
    // or distance, never by contrast alone (which only sees lightness).
    let tries: Vec<String> = [
        theme.red(),
        Some(FAILED_RED),
        theme.magenta(),
        Some(FALLBACK_FAIL_ALT),
    ]
    .into_iter()
    .map(|c| ensure_contrast(&normalize_color(c.unwrap_or(""), FAILED_RED), &bg, FLOOR))
    .collect();
    let failed = tries
        .iter()
        .find(|c| marks_apart(c, &working) && marks_apart(c, &finished))
        .cloned()
        .unwrap_or_else(|| tries[0].clone());

    AgentColours {
        working,
        finished,
        question,
        failed,
    }
}

/// Hue (degrees) and saturation (0 to 1) of a colour; grey for an unparseable one (PT `hueSat`).
fn hue_sat(hex: &str) -> (f64, f64) {
    match parse_colour(hex) {
        Some(c) => {
            let hsl = rgb_to_hsl(&c);
            (hsl.h, hsl.s / 100.0)
        }
        None => (0.0, 0.0),
    }
}

/// Two marks that read as two: hues 20 degrees apart (both coloured), or an RGB distance of 80 or more on the
/// 0 to 255 scale (PT `marksApart`).
pub fn marks_apart(a: &str, b: &str) -> bool {
    let (ha, sa) = hue_sat(a);
    let (hb, sb) = hue_sat(b);
    let turn = (ha - hb).abs();
    if sa > 0.25 && sb > 0.25 && turn.min(360.0 - turn) >= 20.0 {
        return true;
    }
    let [ar, ag, ab] = hex_to_rgb(a);
    let [br, bg, bb] = hex_to_rgb(b);
    ((ar - br).powi(2) + (ag - bg).powi(2) + (ab - bb).powi(2)).sqrt() >= 80.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ansi::contrast_ratio;
    use crate::chrome::chrome_tokens;
    use crate::edges::WindowEdges;
    use crate::preset::{find_preset, presets};
    use crate::resolve::resolve_preset;
    use crate::testutil::fixture;

    fn preset(id: &str) -> &'static crate::preset::Preset {
        find_preset(id).unwrap_or_else(|| panic!("no preset {id}"))
    }

    /// The opaque flat ground the indicators are measured on (`--p-bg-solid`).
    fn bg_solid(id: &str) -> String {
        let p = preset(id);
        chrome_tokens(
            &resolve_preset(p),
            1.0,
            p.accent,
            WindowEdges::Hairline,
            None,
            None,
        )
        .vars
        .get("--p-bg-solid")
        .cloned()
        .unwrap_or_else(|| panic!("no --p-bg-solid for {id}"))
    }

    fn colours(id: &str) -> AgentColours {
        let p = preset(id);
        theme_agent_colours(&resolve_preset(p), p.accent, None, None)
    }

    #[test]
    fn working_is_the_chrome_accent_of_the_theme() {
        for id in ["pt-default", "dracula", "paper"] {
            let p = preset(id);
            let tokens = chrome_tokens(
                &resolve_preset(p),
                1.0,
                p.accent,
                WindowEdges::Hairline,
                None,
                None,
            );
            let want = tokens.vars.get("--p-accent").cloned().unwrap_or_default();
            assert_eq!(colours(id).working, want, "{id}");
        }
    }

    #[test]
    fn working_wears_the_indigo_on_the_prism_theme() {
        assert_eq!(colours("prism").working, "#5b5bd6");
    }

    #[test]
    fn both_states_show_on_the_ground_and_differ_from_each_other() {
        for p in presets() {
            let c = colours(p.id);
            let bg = bg_solid(p.id);
            assert!(contrast_ratio(&c.working, &bg) >= 3.0, "{} working", p.id);
            assert!(contrast_ratio(&c.finished, &bg) >= 3.0, "{} finished", p.id);
            assert_ne!(
                c.working.to_lowercase(),
                c.finished.to_lowercase(),
                "{}",
                p.id
            );
        }
    }

    #[test]
    fn failed_shows_on_the_ground_and_is_neither_of_the_other_two() {
        for p in presets() {
            let c = colours(p.id);
            let bg = bg_solid(p.id);
            assert!(contrast_ratio(&c.failed, &bg) >= 3.0, "{} failed", p.id);
            assert_ne!(c.failed, c.working, "{}", p.id);
            assert_ne!(c.failed, c.finished, "{}", p.id);
            assert!(marks_apart(&c.failed, &c.working), "{} vs working", p.id);
            assert!(marks_apart(&c.failed, &c.finished), "{} vs finished", p.id);
        }
    }

    #[test]
    fn failed_is_the_themes_own_red_where_it_reads_apart_from_the_accent() {
        assert_eq!(colours("pt-default").failed, "#ff615a");
    }

    #[test]
    fn marks_apart_tells_a_red_from_an_orange_not_a_red_from_a_red() {
        assert!(marks_apart("#ff615a", "#fe8f34"));
        assert!(!marks_apart("#ef4444", "#e5484d"));
        assert!(marks_apart("#22c55e", "#ef4444"));
        assert!(!marks_apart("#777777", "#7a7a7a"));
    }

    #[test]
    fn a_see_through_picked_accent_leaves_working_opaque() {
        let p = preset("pt-default");
        let c = theme_agent_colours(&resolve_preset(p), p.accent, None, Some("#e07a2f80"));
        assert_eq!(c.working, "#e07a2f");
    }

    #[test]
    fn oracle_agent_colours() {
        let want = fixture("agent_colours.json");
        let want = want.as_object().expect("object");
        assert_eq!(want.len(), presets().len() * 3);
        for p in presets() {
            let theme = resolve_preset(p);
            let cases = [
                (
                    p.id.to_string(),
                    theme_agent_colours(&theme, p.accent, None, None),
                ),
                (
                    format!("{}@bg", p.id),
                    theme_agent_colours(&theme, p.accent, Some("#f4f1ea"), None),
                ),
                (
                    format!("{}@accent", p.id),
                    theme_agent_colours(&theme, p.accent, None, Some("#e07a2f80")),
                ),
            ];
            for (key, got) in cases {
                let row = want
                    .get(&key)
                    .unwrap_or_else(|| panic!("no fixture row {key}"));
                let field = |name: &str| row[name].as_str().unwrap_or_default().to_string();
                assert_eq!(got.working, field("working"), "{key} working");
                assert_eq!(got.finished, field("finished"), "{key} finished");
                assert_eq!(got.question, field("question"), "{key} question");
                assert_eq!(got.failed, field("failed"), "{key} failed");
            }
        }
    }

    #[test]
    fn oracle_marks_apart() {
        let rows = fixture("marks_apart.json");
        let rows = rows.as_array().expect("array");
        assert_eq!(rows.len(), 4);
        for row in rows {
            let a = row["a"].as_str().unwrap_or_default();
            let b = row["b"].as_str().unwrap_or_default();
            assert_eq!(
                marks_apart(a, b),
                row["apart"].as_bool() == Some(true),
                "{a} {b}"
            );
        }
    }
}
