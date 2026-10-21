//! The look: settings in, what the window wears out (Task 14).
//! Source: PT `src/renderer/src/lib/paintChrome.ts`, `core/renderer/components/TerminalPanel.tsx`
//! (`groundedTheme`, `currentLinkColor`) and `core/renderer/settings/theme/palette.ts` (`cardAnsi`).
//! Pure: no runtime dependencies.

use crate::chrome::{chrome_tokens, ChromeTokens};
use crate::edges::WindowEdges;
use crate::links::link_colour;
use crate::marks::Mode;
use crate::resolve::{on_ground, preset_accent, resolve_theme, CustomPalette, TermTheme};
use crate::retired::live_theme_id;
use crate::see_through::{ground_alpha, SEE_THROUGH_TOP};

/// Everything the settings say about the look.
pub struct LookInput<'a> {
    /// As stored; retired ids are allowed.
    pub theme_id: &'a str,
    pub custom: Option<&'a CustomPalette>,
    /// The colour editor's draft, worn while it is open.
    pub draft: Option<&'a CustomPalette>,
    /// Settings picks; `None` follows the theme.
    pub background: Option<&'a str>,
    pub accent: Option<&'a str>,
    pub secondary: Option<&'a str>,
    pub edges: WindowEdges,
    /// 0 to 70.
    pub see_through: u8,
    /// The GPU renderer is effective and the OS has the backdrop.
    pub glass_possible: bool,
}

/// What the window wears.
#[derive(Clone, Debug, PartialEq)]
pub struct AppliedLook {
    /// The live id.
    pub theme_id: String,
    /// What the grid wears: `on_ground(resolve(id, ground), ground)`.
    pub term: TermTheme,
    /// `link_colour(term.background, term.foreground)`.
    pub link_colour: String,
    pub chrome: ChromeTokens,
    /// The chrome's `--p-bg-solid`.
    pub solid_ground: String,
    /// 1.0 unless glass.
    pub ground_alpha: f64,
    /// See-through is on and possible.
    pub glass: bool,
    /// The chrome is in light mode.
    pub light: bool,
    pub edges: WindowEdges,
    pub see_through: u8,
}

/// Settings in, everything the window wears out (PT `paintChrome` plus `groundedTheme`).
pub fn apply_look(i: &LookInput) -> AppliedLook {
    let id = live_theme_id(i.theme_id);
    let see_through = i.see_through.min(SEE_THROUGH_TOP);
    let glass = see_through > 0 && i.glass_possible;
    let alpha = if glass {
        ground_alpha(see_through)
    } else {
        1.0
    };
    let background = i.background.filter(|b| !b.is_empty());

    // The chrome never takes the ground from the grid: a picked background replaces the theme's.
    let mut chrome_theme = resolve_theme(id, id, i.custom, i.draft, None);
    if let Some(b) = background {
        chrome_theme.background = b.to_string();
    }
    let wanted = preset_accent(id, id, i.custom, i.draft);
    let chrome = chrome_tokens(
        &chrome_theme,
        alpha,
        wanted.as_deref(),
        i.edges,
        i.accent,
        i.secondary,
    );

    let term = on_ground(
        &resolve_theme(id, id, i.custom, i.draft, background),
        background,
    );
    let solid_ground = chrome
        .vars
        .get("--p-bg-solid")
        .cloned()
        .unwrap_or_else(|| term.background.clone());
    AppliedLook {
        theme_id: id.to_string(),
        link_colour: link_colour(&term.background, &term.foreground),
        light: chrome.mode == Mode::Light,
        term,
        chrome,
        solid_ground,
        ground_alpha: alpha,
        glass,
        edges: i.edges,
        see_through,
    }
}

/// The five colours a theme card's miniature session reads.
#[derive(Clone, Debug, PartialEq)]
pub struct CardAnsi {
    pub green: String,
    pub yellow: String,
    pub blue: String,
    pub cyan: String,
    pub red: String,
}

/// The card's five colours, with the fallbacks a theme missing one of them gets.
pub fn card_ansi(theme: &TermTheme) -> CardAnsi {
    let pick = |c: Option<&str>, fallback: &str| c.unwrap_or(fallback).to_string();
    CardAnsi {
        green: pick(theme.green(), "#8cc265"),
        yellow: pick(theme.ansi_at(crate::ansi::idx::YELLOW), "#d1a54b"),
        blue: pick(theme.blue(), "#4aa5f0"),
        cyan: pick(theme.ansi_at(crate::ansi::idx::CYAN), "#42b3c2"),
        red: pick(theme.red(), "#e05561"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ansi::{contrast_ratio, idx, PartialAnsi};
    use crate::preset::{presets, Preset};
    use crate::resolve::resolve_preset;
    use crate::testutil::fixture;
    use std::collections::BTreeMap;

    fn input(theme_id: &str) -> LookInput<'_> {
        LookInput {
            theme_id,
            custom: None,
            draft: None,
            background: None,
            accent: None,
            secondary: None,
            edges: WindowEdges::Hairline,
            see_through: 0,
            glass_possible: true,
        }
    }

    fn vars_of(c: &ChromeTokens) -> BTreeMap<String, String> {
        c.vars
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect()
    }

    fn want_of(v: &serde_json::Value) -> BTreeMap<String, String> {
        v.as_object()
            .expect("tokens")
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().expect("string").to_string()))
            .collect()
    }

    fn v<'a>(c: &'a ChromeTokens, k: &str) -> &'a str {
        c.vars.get(k).unwrap_or_else(|| panic!("token {k}"))
    }

    fn custom_pt_default() -> CustomPalette {
        CustomPalette {
            bg: "#171717".into(),
            fg: "#dedacf".into(),
            cursor: "#bbbbbb".into(),
            selection: None,
            ansi: PartialAnsi::default(),
            accent: Some("#fe8f34".into()),
        }
    }

    #[test]
    fn unset_colours_follow_the_theme_exactly() {
        let all = fixture("chrome_presets.json");
        let ps: &[Preset] = presets();
        assert_eq!(ps.len(), 39);
        for p in ps {
            let a = apply_look(&input(p.id));
            assert_eq!(vars_of(&a.chrome), want_of(&all[p.id]["theme"]), "{}", p.id);
            assert_eq!(a.term, resolve_preset(p), "{}", p.id);
            assert_eq!(a.theme_id, p.id);
        }
    }

    #[test]
    fn a_picked_background_replaces_the_themes_before_the_chrome_is_measured() {
        let mut i = input("pt-default");
        i.background = Some("#f4f1ea");
        let a = apply_look(&i);
        assert_eq!(a.chrome.mode, Mode::Light);
        assert_eq!(v(&a.chrome, "--p-bg-solid"), "#f4f1ea");
        assert!(a.light);
        assert!(contrast_ratio(v(&a.chrome, "--p-text"), "#f4f1ea") >= 4.5);
        let picks = fixture("chrome_picks.json");
        let row = picks
            .as_array()
            .unwrap()
            .iter()
            .find(|r| {
                r["accent"].is_null()
                    && r["secondary"].is_null()
                    && r["alpha"].as_f64() == Some(1.0)
                    && r["bg"].as_str() == Some("#f4f1ea")
            })
            .expect("fixture row");
        assert_eq!(vars_of(&a.chrome), want_of(&row["vars"]));
    }

    #[test]
    fn the_grid_theme_is_floored_on_the_picked_ground() {
        let mut i = input("pt-default");
        i.background = Some("#f4f1ea");
        let a = apply_look(&i);
        assert_eq!(a.term.background, "#f4f1ea");
        assert!(contrast_ratio(&a.term.foreground, "#f4f1ea") >= 4.5);
        assert!(contrast_ratio(&a.term.cursor, "#f4f1ea") >= 3.0);
        for k in 0..16 {
            let c = a.term.ansi_at(k).expect("sixteen");
            assert!(contrast_ratio(c, "#f4f1ea") >= 3.0, "ansi {k} {c}");
        }
    }

    #[test]
    fn a_picked_accent_and_secondary_reach_the_chrome() {
        let mut i = input("pt-default");
        i.accent = Some("#e07a2f80");
        i.secondary = Some("#3a3a3a");
        let a = apply_look(&i);
        let picks = fixture("chrome_picks.json");
        let row = picks
            .as_array()
            .unwrap()
            .iter()
            .find(|r| {
                r["accent"].as_str() == Some("#e07a2f80")
                    && r["secondary"].as_str() == Some("#3a3a3a")
                    && r["alpha"].as_f64() == Some(1.0)
                    && r["bg"].is_null()
            })
            .expect("fixture row");
        assert_eq!(vars_of(&a.chrome), want_of(&row["vars"]));
        assert_eq!(v(&a.chrome, "--p-secondary"), "#3a3a3a");

        // The accent alone, no alpha, against the four-variant preset fixture.
        let mut j = input("pt-default");
        j.accent = Some("#e07a2f");
        let b = apply_look(&j);
        let all = fixture("chrome_presets.json");
        assert_eq!(vars_of(&b.chrome), want_of(&all["pt-default"]["picked"]));
    }

    #[test]
    fn the_slider_level_sets_the_ground_alpha_once() {
        let levels = fixture("chrome_levels.json");
        for id in ["pt-default", "paper", "volt", "dracula"] {
            for level in [0u8, 1, 10, 25, 30, 50, 69, 70] {
                let mut i = input(id);
                i.see_through = level;
                let a = apply_look(&i);
                assert_eq!(
                    vars_of(&a.chrome),
                    want_of(&levels[id][level.to_string()]),
                    "{id} {level}"
                );
                assert_eq!(a.glass, level > 0, "{id} {level}");
                assert_eq!(a.ground_alpha, 1.0 - f64::from(level) / 100.0);
            }
        }
    }

    #[test]
    fn a_level_without_glass_leaves_the_window_solid() {
        let mut i = input("pt-default");
        i.see_through = 40;
        i.glass_possible = false;
        let a = apply_look(&i);
        assert!(!a.glass);
        assert_eq!(a.ground_alpha, 1.0);
        assert_eq!(v(&a.chrome, "--p-bg").len(), 7);
        let solid = apply_look(&input("pt-default"));
        assert_eq!(vars_of(&a.chrome), vars_of(&solid.chrome));
    }

    #[test]
    fn a_draft_is_worn_while_it_is_open() {
        let saved = custom_pt_default();
        let mut draft = custom_pt_default();
        draft.cursor = "#ff00aa".into();
        draft.accent = Some("#e07a2f".into());
        let mut i = input("custom");
        i.custom = Some(&saved);
        let without = apply_look(&i);
        i.draft = Some(&draft);
        let with = apply_look(&i);
        assert_ne!(with.term.cursor, without.term.cursor);
        assert_eq!(with.term.cursor, "#ff00aa");
        assert_eq!(v(&with.chrome, "--p-accent"), "#e07a2f");
        assert_eq!(v(&without.chrome, "--p-accent"), "#fe8f34");
    }

    #[test]
    fn a_retired_theme_wears_its_replacement() {
        let a = apply_look(&input("github"));
        assert_eq!(a.theme_id, "paper");
        assert_eq!(a.chrome.mode, Mode::Light);
    }

    #[test]
    fn a_custom_made_from_pt_default_keeps_the_orange() {
        let mut c = custom_pt_default();
        c.cursor = "#ff00aa".into();
        let mut i = input("custom");
        i.custom = Some(&c);
        let a = apply_look(&i);
        assert_eq!(v(&a.chrome, "--p-accent"), "#fe8f34");
    }

    #[test]
    fn card_ansi_falls_back_per_colour() {
        let mut ansi = PartialAnsi::default();
        ansi[idx::RED] = Some("#aa0000".into());
        let t = TermTheme {
            background: "#000000".into(),
            foreground: "#ffffff".into(),
            cursor: "#ffffff".into(),
            selection_background: "#ffffff55".into(),
            ansi,
        };
        let c = card_ansi(&t);
        assert_eq!(c.red, "#aa0000");
        assert_eq!(c.green, "#8cc265");
        assert_eq!(c.yellow, "#d1a54b");
        assert_eq!(c.blue, "#4aa5f0");
        assert_eq!(c.cyan, "#42b3c2");
    }

    #[cfg(not(debug_assertions))]
    #[test]
    fn applying_all_39_themes_is_fast_enough_to_do_on_a_click() {
        let t = std::time::Instant::now();
        for p in presets() {
            std::hint::black_box(apply_look(&input(p.id)));
        }
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        println!("39 apply_look calls: {ms:.2} ms");
        assert!(ms < 100.0, "{ms} ms");
    }
}
