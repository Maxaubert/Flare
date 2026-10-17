//! The 31 chrome colour tokens derived from the theme (Task 12). Source: PT
//! `src/renderer/src/lib/chromeTheme.ts`, whole; every comment there is an owner decision.

use std::collections::BTreeMap;

use crate::ansi::{contrast_ratio, ensure_contrast, luminance, mix_hex, normalize_color};
use crate::colour::{alpha_hex, alpha_of};
use crate::colour_contrast::{composite, selection_for};
use crate::edges::{edge_alpha, WindowEdges};
use crate::ground::{DEFAULT_GROUND, LIGHT_GROUND, ON_DARK_INK};
use crate::knob::switch_knob;
use crate::marks::Mode;
use crate::num::clamp;
use crate::oklab::{at_lightness, lift_to_floor, ok_lightness};
use crate::resolve::TermTheme;
use crate::sidebar::sidebar_ground;

const INDIGO: &str = "#5b5bd6";
const FALLBACK_BG: &str = DEFAULT_GROUND;
const FALLBACK_FG: &str = "#e7e7ee";

/// Body text clears WCAG's 4.5.
const TEXT_FLOOR: f64 = 4.5;
/// The dim inks, icons and the accent clear 3, the large-text and non-text line.
const QUIET_FLOOR: f64 = 3.0;
/// White text on the accent is tried from this ratio up.
const WHITE_ON_ACCENT: f64 = 3.0;
/// The sidebar tab names: AAA.
const SIDE_TEXT_FLOOR: f64 = 7.0;

/// Every COLOUR token the components read (PT `CHROME_COLOUR_TOKENS`).
pub const CHROME_COLOUR_TOKENS: [&str; 31] = [
    "--p-bg",
    "--p-bg-solid",
    "--p-side",
    "--p-side-flat",
    "--p-secondary",
    "--p-title",
    "--p-tabs",
    "--p-tab-active",
    "--p-text",
    "--p-text-soft",
    "--p-dim",
    "--p-dim2",
    "--p-accent",
    "--p-accent-solid",
    "--p-accent-hi",
    "--p-on-accent",
    "--p-icon",
    "--p-hover",
    "--p-hover-hi",
    "--p-divider",
    "--p-line",
    "--p-track",
    "--p-control",
    "--p-preview",
    "--p-sel-bg",
    "--p-tree-folder",
    "--p-switch-knob",
    "--p-pressed",
    "--p-pressed-ink",
    "--p-side-text",
    "--p-side-dim",
];

/// The mode the chrome wears and its tokens; every value is `#rrggbb` or `#rrggbbaa`, lower-case.
#[derive(Clone, Debug, PartialEq)]
pub struct ChromeTokens {
    pub mode: Mode,
    pub vars: BTreeMap<&'static str, String>,
}

/// A percentage of the mode's ink as the two hex digits of an alpha (PT `pctHex`).
fn pct_hex(pct: f64) -> String {
    alpha_hex(clamp(pct, 0.0, 100.0) / 100.0)
}

/// Pure: a terminal theme in, the chrome's tokens out (PT `chromeTokens`). `ground_alpha` below 1 makes the
/// window's surfaces translucent. `--p-bg-solid` and `--p-side-flat` are always flat, which is what everything
/// here is measured against and what a menu or a dialog paints. Every value is `#rrggbb` or `#rrggbbaa`.
///
/// `edges` is applied here, to the two tokens every edge reads (`--p-divider`, `--p-line`). `chosen_accent` is
/// the accent the user chose (it may carry an alpha); `chosen_secondary` the sidebar and title bar colour they
/// chose. `None` or an empty string for either follows the theme.
pub fn chrome_tokens(
    theme: &TermTheme,
    ground_alpha: f64,
    wanted_accent: Option<&str>,
    edges: WindowEdges,
    chosen_accent: Option<&str>,
    chosen_secondary: Option<&str>,
) -> ChromeTokens {
    // Flattened before any maths: a theme may publish rgba() or #rrggbbaa, which is fine to paint and
    // wrong to measure.
    let bg = normalize_color(&theme.background, FALLBACK_BG);
    let raw_fg = normalize_color(&theme.foreground, FALLBACK_FG);
    // Measured, never read off a name.
    let mode = if luminance(&bg) > LIGHT_GROUND {
        Mode::Light
    } else {
        Mode::Dark
    };
    let light = mode == Mode::Light;
    // The flat panel (menus, dialogs, the find bar) is the ground lifted a touch toward the text.
    let flat = mix_hex(&bg, &ensure_contrast(&raw_fg, &bg, TEXT_FLOOR), 0.04);
    // The secondary colour: the sidebar and the title bar. Unpicked, one even step darker than the ground.
    let own_secondary = sidebar_ground(&bg);
    let secondary = match chosen_secondary.filter(|c| !c.is_empty()) {
        Some(c) => normalize_color(c, &own_secondary),
        None => own_secondary,
    };
    // The form control's fill, a step off the ground.
    let control_fill = mix_hex(&bg, &raw_fg, if light { 0.09 } else { 0.035 });
    let floor_on = |c: &str, floor: f64| -> String {
        let a = ensure_contrast(c, &flat, floor);
        let b = ensure_contrast(&a, &secondary, floor);
        let d = ensure_contrast(&b, if light { &control_fill } else { &bg }, floor);
        ensure_contrast(&d, &bg, floor)
    };
    let fg = floor_on(&raw_fg, TEXT_FLOOR);
    // The theme's own blue first, its cursor second, the indigo last; the first that is visible wins.
    let candidates: Vec<String> = [
        wanted_accent,
        theme.blue(),
        Some(theme.cursor.as_str()),
        Some(INDIGO),
    ]
    .into_iter()
    .flatten()
    .filter(|c| !c.is_empty())
    .map(|c| normalize_color(c, INDIGO))
    .collect();
    let visible = |c: &str| -> bool {
        contrast_ratio(c, &bg) >= QUIET_FLOOR && contrast_ratio(c, &flat) >= QUIET_FLOOR
    };
    // A CHOSEN accent is kept when visible, else only moved as far as the floor.
    let picked = chosen_accent.filter(|c| !c.is_empty());
    let chosen = picked.map(|c| normalize_color(c, INDIGO));
    // The accent as a LINE: never see-through; a picked alpha belongs to the fills below.
    let mut solid = match &chosen {
        Some(c) => {
            if visible(c) {
                c.clone()
            } else {
                let lifted =
                    lift_to_floor(c, &[&bg, &flat], QUIET_FLOOR).unwrap_or_else(|| c.clone());
                floor_on(&lifted, QUIET_FLOOR)
            }
        }
        None => candidates
            .iter()
            .find(|c| visible(c))
            .cloned()
            .unwrap_or_else(|| floor_on(&candidates[0], QUIET_FLOOR)),
    };
    let glass = ground_alpha < 1.0;
    let mut accent = solid.clone();
    let on_accent: String;
    let mut sel_bg: String;
    // A see-through accent is a FILL painted with its alpha; its text is chosen on what the eye sees.
    let fill_alpha = picked.map_or(1.0, alpha_of);
    if fill_alpha < 1.0 {
        let a = alpha_hex(fill_alpha);
        let (fill, ink) = selection_for(&format!("{solid}{a}"), &[&bg, &flat]);
        on_accent = ink.to_string();
        accent = format!("{fill}{a}");
        sel_bg = accent.clone();
    } else {
        // White or near-black, whichever reads better.
        let on_white = contrast_ratio("#ffffff", &accent);
        let on_dark = contrast_ratio(ON_DARK_INK, &accent);
        let mut ink = if on_white >= on_dark {
            "#ffffff"
        } else {
            ON_DARK_INK
        };
        // White on a strong mid colour: take the accent a shade deeper, by lightness alone, until white
        // clears the body floor, as far as the ground still shows it at the accent floor.
        if ink != "#ffffff" && on_white >= WHITE_ON_ACCENT && on_white * 1.5 >= on_dark {
            let mut deeper = accent.clone();
            for _ in 0..40 {
                if contrast_ratio("#ffffff", &deeper) >= TEXT_FLOOR {
                    break;
                }
                let next = at_lightness(&deeper, ok_lightness(&deeper) - 0.005);
                if !visible(&next) {
                    break;
                }
                deeper = next;
            }
            if contrast_ratio("#ffffff", &deeper) >= TEXT_FLOOR {
                accent = deeper.clone();
                solid = deeper;
                ink = "#ffffff";
            }
        }
        on_accent = ink.to_string();
        // A selected row carries text, so its fill is the accent moved until that ink clears the floor.
        sel_bg = accent.clone();
        let away = if ink == "#ffffff" {
            "#000000"
        } else {
            "#ffffff"
        };
        for _ in 0..15 {
            if contrast_ratio(ink, &sel_bg) >= TEXT_FLOOR {
                break;
            }
            sel_bg = mix_hex(&sel_bg, away, 0.04);
        }
    }
    // Fills under glass: a see-through fill that carries text is flattened over the solid ground it was
    // chosen on. An opaque accent composites to itself.
    if glass && fill_alpha < 1.0 {
        accent = composite(&accent, &bg);
        sel_bg = composite(&sel_bg, &bg);
    }
    let accent_hi = floor_on(&mix_hex(&solid, &fg, 0.25), QUIET_FLOOR);
    // The sidebar inks: on its ground and on the accent pill of the tab in front.
    let side_solid = composite(&secondary, &bg);
    let pill = mix_hex(&side_solid, &accent_hi, 0.18);
    let side_held = |c: &str, floor: f64| -> String {
        ensure_contrast(&ensure_contrast(c, &side_solid, floor), &pill, floor)
    };
    let side_text = side_held(&fg, SIDE_TEXT_FLOOR);
    let side_dim = side_held(
        &mix_hex(&fg, &side_solid, if light { 0.42 } else { 0.55 }),
        TEXT_FLOOR,
    );
    // On a light theme the quiet inks sit nearer the text.
    let dim = if light {
        floor_on(&mix_hex(&fg, &bg, 0.28), TEXT_FLOOR)
    } else {
        floor_on(&mix_hex(&fg, &bg, 0.38), QUIET_FLOOR)
    };
    let icon = if light {
        floor_on(&mix_hex(&fg, &bg, 0.18), TEXT_FLOOR)
    } else {
        dim.clone()
    };
    // A raised stage rather than a sunken one.
    let stage = mix_hex(&bg, &fg, if light { 0.16 } else { 0.13 });
    // Hover, divider and line are the mode's ink at a low alpha.
    let ink = if light { "#000000" } else { "#ffffff" };
    let ea = edge_alpha(edges);
    let (divider_pct, line_pct) = if light {
        (ea.divider.light, ea.line.light)
    } else {
        (ea.divider.dark, ea.line.dark)
    };
    // The window is ONE sheet: the title bar, the tab strip and the ground are the same colour and alpha.
    let sheet = if glass {
        format!("{bg}{}", alpha_hex(ground_alpha))
    } else {
        bg.clone()
    };
    let clear = format!("{bg}00");
    let dim2 = if light {
        floor_on(&mix_hex(&fg, &bg, 0.42), TEXT_FLOOR)
    } else {
        floor_on(&mix_hex(&fg, &bg, 0.55), QUIET_FLOOR)
    };
    let text_soft = floor_on(&mix_hex(&fg, &bg, 0.14), TEXT_FLOOR);
    let knob = switch_knob(&composite(&sel_bg, &bg), !light).to_string();
    let (pressed, pressed_ink) = if light {
        (mix_hex(&fg, &bg, 0.08), bg.clone())
    } else {
        (mix_hex(&mix_hex(&bg, &fg, 0.035), &fg, 0.16), fg.clone())
    };
    let mut vars: BTreeMap<&'static str, String> = BTreeMap::new();
    let mut put = |name: &'static str, value: String| {
        vars.insert(name, value);
    };
    put("--p-bg-solid", bg.clone());
    put("--p-bg", sheet.clone());
    // The settings rail sits ON a page that already paints the sheet: on glass it paints nothing.
    put("--p-side", if glass { clear.clone() } else { bg.clone() });
    put("--p-side-flat", flat.clone());
    put(
        "--p-secondary",
        if glass {
            format!("{secondary}{}", alpha_hex(ground_alpha))
        } else {
            secondary.clone()
        },
    );
    put("--p-title", sheet.clone());
    put("--p-tabs", sheet);
    // The active tab is told by its ink, not a fill; on glass it must paint nothing.
    put("--p-tab-active", if glass { clear } else { bg.clone() });
    put("--p-text", fg.clone());
    put("--p-text-soft", text_soft);
    put("--p-dim", dim);
    put("--p-dim2", dim2);
    put("--p-accent", accent);
    put("--p-accent-solid", solid);
    put("--p-accent-hi", accent_hi.clone());
    put("--p-on-accent", on_accent);
    put("--p-icon", icon);
    put(
        "--p-hover",
        format!("{ink}{}", pct_hex(if light { 7.0 } else { 6.0 })),
    );
    put(
        "--p-hover-hi",
        format!("{ink}{}", pct_hex(if light { 12.0 } else { 11.0 })),
    );
    put("--p-divider", format!("{ink}{}", pct_hex(divider_pct)));
    put("--p-line", format!("{ink}{}", pct_hex(line_pct)));
    put("--p-preview", stage.clone());
    put(
        "--p-control",
        mix_hex(&bg, &fg, if light { 0.09 } else { 0.035 }),
    );
    put(
        "--p-track",
        mix_hex(&stage, &fg, if light { 0.5 } else { 0.26 }),
    );
    put("--p-sel-bg", sel_bg);
    put("--p-tree-folder", accent_hi);
    put("--p-switch-knob", knob);
    put("--p-pressed", pressed);
    put("--p-pressed-ink", pressed_ink);
    put("--p-side-text", side_text);
    put("--p-side-dim", side_dim);
    ChromeTokens { mode, vars }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ansi::idx;
    use crate::edges::WINDOW_EDGES;
    use crate::marks::ground_mode;
    use crate::num::js_round;
    use crate::preset::{find_preset, presets, Preset};
    use crate::resolve::resolve_preset;
    use crate::see_through::see_through_alpha;
    use crate::testutil::fixture;

    const HAIR: WindowEdges = WindowEdges::Hairline;

    fn preset(id: &str) -> &'static Preset {
        find_preset(id).unwrap_or_else(|| panic!("no preset {id}"))
    }

    fn theme_of(id: &str) -> TermTheme {
        resolve_preset(preset(id))
    }

    fn of(p: &Preset) -> ChromeTokens {
        chrome_tokens(&resolve_preset(p), 1.0, p.accent, HAIR, None, None)
    }

    fn with(id: &str, alpha: f64, accent: Option<&str>, secondary: Option<&str>) -> ChromeTokens {
        let p = preset(id);
        chrome_tokens(&resolve_preset(p), alpha, p.accent, HAIR, accent, secondary)
    }

    fn v<'a>(c: &'a ChromeTokens, name: &str) -> &'a str {
        c.vars
            .get(name)
            .unwrap_or_else(|| panic!("no token {name}"))
    }

    fn ratio(a: &str, b: &str) -> f64 {
        contrast_ratio(&a[..7], &b[..7])
    }

    fn is_hex(s: &str) -> bool {
        (s.len() == 7 || s.len() == 9)
            && s.starts_with('#')
            && s[1..]
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
    }

    fn plain_theme(bg: &str, fg: &str, cursor: &str, blue: &str) -> TermTheme {
        let mut t = TermTheme {
            background: bg.to_string(),
            foreground: fg.to_string(),
            cursor: cursor.to_string(),
            selection_background: format!("{cursor}55"),
            ansi: Default::default(),
        };
        t.ansi[idx::BLUE] = Some(blue.to_string());
        t
    }

    fn channels(hex: &str) -> [i32; 3] {
        let p = |i: usize| i32::from_str_radix(&hex[i..i + 2], 16).unwrap();
        [p(1), p(3), p(5)]
    }

    #[test]
    fn every_preset_has_readable_inks_and_a_visible_accent_on_its_own_ground() {
        for p in presets() {
            let c = of(p);
            for ground in [v(&c, "--p-bg-solid"), v(&c, "--p-side-flat")] {
                for t in ["--p-text", "--p-text-soft"] {
                    assert!(ratio(v(&c, t), ground) >= 4.5, "{} {t}", p.id);
                }
                for t in [
                    "--p-dim",
                    "--p-dim2",
                    "--p-icon",
                    "--p-accent",
                    "--p-accent-hi",
                    "--p-tree-folder",
                ] {
                    assert!(ratio(v(&c, t), ground) >= 3.0, "{} {t}", p.id);
                }
            }
            assert!(
                ratio(v(&c, "--p-on-accent"), v(&c, "--p-accent")) >= 3.0,
                "{}",
                p.id
            );
            assert!(
                ratio(v(&c, "--p-on-accent"), v(&c, "--p-sel-bg")) >= 4.5,
                "{}",
                p.id
            );
        }
    }

    #[test]
    fn a_program_is_told_the_mode_the_chrome_wears() {
        for p in presets() {
            let t = resolve_preset(p);
            let c = chrome_tokens(&t, 1.0, p.accent, HAIR, None, None);
            assert_eq!(ground_mode(&t.background), c.mode, "{}", p.id);
        }
    }

    #[test]
    fn wears_a_chosen_accent_exactly_when_the_ground_can_show_it() {
        let c = with("prism", 1.0, Some("#e07a2f"), None);
        assert_eq!(v(&c, "--p-accent"), "#e07a2f");
        assert!(ratio(v(&c, "--p-on-accent"), v(&c, "--p-accent")) >= 3.0);
        assert!(ratio(v(&c, "--p-on-accent"), v(&c, "--p-sel-bg")) >= 4.5);
        let own = with("prism", 1.0, None, None);
        assert_ne!(v(&c, "--p-accent"), v(&own, "--p-accent"));
    }

    #[test]
    fn a_chosen_accent_the_ground_cannot_show_is_moved_to_the_floor_not_swapped() {
        for p in presets() {
            for pick in ["#0a0a0a", "#f7f7f7"] {
                let c = with(p.id, 1.0, Some(pick), None);
                assert!(
                    ratio(v(&c, "--p-accent"), v(&c, "--p-bg-solid")) >= 3.0,
                    "{} {pick}",
                    p.id
                );
                assert!(
                    ratio(v(&c, "--p-accent"), v(&c, "--p-side-flat")) >= 3.0,
                    "{} {pick}",
                    p.id
                );
                assert!(
                    ratio(v(&c, "--p-on-accent"), v(&c, "--p-sel-bg")) >= 4.5,
                    "{} {pick}",
                    p.id
                );
            }
        }
    }

    #[test]
    fn a_picked_light_background_makes_a_light_window_whose_inks_still_read() {
        let mut t = theme_of("pt-default");
        t.background = "#f4f1ea".to_string();
        let c = chrome_tokens(&t, 1.0, Some("#fe8f34"), HAIR, None, None);
        assert_eq!(c.mode, Mode::Light);
        assert_eq!(v(&c, "--p-bg-solid"), "#f4f1ea");
        assert!(ratio(v(&c, "--p-text"), v(&c, "--p-bg-solid")) >= 4.5);
        assert!(ratio(v(&c, "--p-accent"), v(&c, "--p-bg-solid")) >= 3.0);
    }

    #[test]
    fn follows_the_theme_when_nothing_is_chosen() {
        let t = theme_of("prism");
        let none = chrome_tokens(&t, 1.0, None, HAIR, None, None);
        assert_eq!(
            none.vars,
            chrome_tokens(&t, 1.0, None, HAIR, Some(""), Some("")).vars
        );
        assert_eq!(
            none.vars,
            chrome_tokens(&t, 1.0, None, HAIR, Some(""), None).vars
        );
        for p in presets() {
            let t = resolve_preset(p);
            let a = chrome_tokens(&t, 1.0, p.accent, HAIR, None, None);
            let b = chrome_tokens(&t, 1.0, p.accent, HAIR, Some(""), Some(""));
            assert_eq!(a, b, "{}", p.id);
        }
    }

    #[test]
    fn publishes_every_colour_token_the_components_read() {
        assert_eq!(CHROME_COLOUR_TOKENS.len(), 31);
        for alpha in [1.0, 153.0 / 255.0] {
            let c = with("prism", alpha, None, None);
            assert_eq!(c.vars.len(), 31);
            for name in CHROME_COLOUR_TOKENS {
                assert!(is_hex(v(&c, name)), "{name} = {}", v(&c, name));
            }
        }
    }

    #[test]
    fn measures_the_mode_instead_of_trusting_a_name() {
        assert_eq!(of(preset("paper")).mode, Mode::Light);
        assert_eq!(of(preset("dracula")).mode, Mode::Dark);
    }

    #[test]
    fn paints_a_translucent_ground_only_when_asked() {
        let opaque = with("prism", 1.0, None, None);
        for name in ["--p-bg", "--p-title", "--p-tabs"] {
            assert_eq!(v(&opaque, name).len(), 7, "{name}");
        }
        assert_eq!(v(&opaque, "--p-side"), v(&opaque, "--p-bg"));
        let glass = with("prism", 153.0 / 255.0, None, None);
        for name in ["--p-bg", "--p-title", "--p-tabs"] {
            assert!(v(&glass, name).ends_with("99"), "{name}");
        }
        for name in ["--p-tab-active", "--p-side"] {
            assert!(v(&glass, name).ends_with("00"), "{name}");
        }
        assert_eq!(v(&glass, "--p-side-flat").len(), 7);
        assert_eq!(v(&glass, "--p-bg-solid").len(), 7);
    }

    #[test]
    fn moves_a_custom_theme_that_fails_the_floors_instead_of_trusting_it() {
        let t = plain_theme("#202020", "#2a2a2a", "#242424", "#222233");
        let c = chrome_tokens(&t, 1.0, None, HAIR, None, None);
        let g = v(&c, "--p-bg-solid");
        assert!(ratio(v(&c, "--p-text"), g) >= 4.5);
        assert!(ratio(v(&c, "--p-dim"), g) >= 3.0);
        assert!(ratio(v(&c, "--p-accent"), g) >= 3.0);
    }

    #[test]
    fn measures_against_a_flat_ground_when_the_theme_publishes_anything_else() {
        let mut t = theme_of("pt-default");
        t.background = "rgba(11, 11, 15, 0.8)".to_string();
        let c = chrome_tokens(&t, 1.0, None, HAIR, None, None);
        assert_eq!(c.mode, Mode::Dark);
        assert_eq!(v(&c, "--p-bg-solid"), "#0b0b0f");
        for name in CHROME_COLOUR_TOKENS {
            assert!(is_hex(v(&c, name)), "{name}");
        }
    }

    fn edged(id: &str, e: WindowEdges) -> ChromeTokens {
        let p = preset(id);
        chrome_tokens(&resolve_preset(p), 1.0, p.accent, e, None, None)
    }

    fn alpha_byte(s: &str) -> u32 {
        u32::from_str_radix(&s[7..9], 16).unwrap()
    }

    #[test]
    fn hairline_is_the_default_and_exactly_the_window_as_it_was() {
        let a = edged("prism", HAIR);
        assert_eq!(v(&a, "--p-divider"), "#ffffff12");
        assert_eq!(v(&a, "--p-line"), "#ffffff17");
        let b = edged("paper", HAIR);
        assert_eq!(v(&b, "--p-divider"), "#00000026");
        assert_eq!(v(&b, "--p-line"), "#0000002b");
    }

    #[test]
    fn none_faint_hairline_solid_order_on_both_line_tokens() {
        for id in ["prism", "paper"] {
            for name in ["--p-divider", "--p-line"] {
                let a: Vec<u32> = WINDOW_EDGES
                    .iter()
                    .map(|e| alpha_byte(v(&edged(id, *e), name)))
                    .collect();
                assert!(
                    a[0] < a[1] && a[1] < a[2] && a[2] < a[3],
                    "{id} {name} {a:?}"
                );
            }
        }
    }

    #[test]
    fn no_edges_is_a_transparent_colour_never_a_missing_one() {
        let c = edged("prism", WindowEdges::None);
        assert_eq!(v(&c, "--p-divider"), "#ffffff00");
        assert_eq!(v(&c, "--p-line"), "#ffffff00");
    }

    #[test]
    fn edges_move_the_two_line_tokens_and_nothing_else() {
        let base = edged("dracula", HAIR);
        for e in WINDOW_EDGES {
            let c = edged("dracula", e);
            for name in CHROME_COLOUR_TOKENS {
                if name == "--p-divider" || name == "--p-line" {
                    continue;
                }
                assert_eq!(v(&c, name), v(&base, name), "{name} under {e:?}");
            }
        }
    }

    const PICKS: [&str; 4] = ["#e07a2f80", "#3b82f633", "#f7f7f7cc", "#0a0a0a99"];

    #[test]
    fn see_through_accent_fills_see_through_lines_solid_text_4_5_on_the_composite() {
        for p in presets() {
            for pick in PICKS {
                let c = with(p.id, 1.0, Some(pick), None);
                let a = &pick[7..9];
                assert_eq!(v(&c, "--p-accent").len(), 9, "{} {pick}", p.id);
                assert!(v(&c, "--p-accent").ends_with(a), "{} {pick}", p.id);
                assert_eq!(v(&c, "--p-sel-bg").len(), 9, "{} {pick}", p.id);
                assert!(v(&c, "--p-sel-bg").ends_with(a), "{} {pick}", p.id);
                assert_eq!(v(&c, "--p-accent-solid").len(), 7);
                assert_eq!(v(&c, "--p-accent-hi").len(), 7);
                let ground = v(&c, "--p-bg-solid");
                assert!(
                    ratio(v(&c, "--p-accent-solid"), ground) >= 3.0,
                    "{} {pick}",
                    p.id
                );
                assert!(
                    ratio(v(&c, "--p-accent-solid"), v(&c, "--p-side-flat")) >= 3.0,
                    "{} {pick}",
                    p.id
                );
                for t in ["--p-accent", "--p-sel-bg"] {
                    let seen = composite(v(&c, t), ground);
                    assert!(
                        contrast_ratio(&v(&c, "--p-on-accent")[..7], &seen) >= 4.5,
                        "{} {pick} {t}",
                        p.id
                    );
                }
            }
        }
        let c = with("pt-default", 1.0, Some("#e07a2f80"), None);
        assert_eq!(v(&c, "--p-accent-solid"), "#e07a2f");
    }

    #[test]
    fn keeps_the_picked_colour_where_the_ground_can_show_it() {
        let c = with("pt-default", 1.0, Some("#e07a2f80"), None);
        assert_eq!(v(&c, "--p-accent-solid"), "#e07a2f");
    }

    #[test]
    fn an_opaque_accent_publishes_its_solid_as_itself() {
        for p in presets() {
            for pick in [None, Some("#e07a2f")] {
                let c = with(p.id, 1.0, pick, None);
                assert_eq!(
                    v(&c, "--p-accent"),
                    v(&c, "--p-accent-solid"),
                    "{} {pick:?}",
                    p.id
                );
            }
        }
    }

    #[test]
    fn under_a_see_through_ground_the_text_fills_are_flattened() {
        let alpha = 153.0 / 255.0;
        let c = with("pt-default", alpha, Some("#e07a2f80"), None);
        assert!(v(&c, "--p-bg").ends_with("99"));
        assert_eq!(v(&c, "--p-accent").len(), 7);
        assert_eq!(v(&c, "--p-sel-bg").len(), 7);
        for t in ["--p-accent", "--p-sel-bg"] {
            assert!(ratio(v(&c, "--p-on-accent"), v(&c, t)) >= 4.5);
        }
        let open = with("pt-default", 1.0, Some("#e07a2f80"), None);
        assert_eq!(
            v(&c, "--p-sel-bg"),
            composite(v(&open, "--p-sel-bg"), v(&open, "--p-bg-solid"))
        );
    }

    #[test]
    fn every_preset_is_legible_at_its_default_see_through() {
        for p in presets() {
            let t = resolve_preset(p);
            let alpha = see_through_alpha(&t.background);
            let c = chrome_tokens(&t, alpha, p.accent, HAIR, None, None);
            let suffix = if c.mode == Mode::Dark { "b9" } else { "d1" };
            assert!(v(&c, "--p-bg").ends_with(suffix), "{}", p.id);
            for g in [v(&c, "--p-bg-solid"), v(&c, "--p-side-flat")] {
                assert!(ratio(v(&c, "--p-text"), g) >= 4.5, "{}", p.id);
            }
            for t in ["--p-accent", "--p-sel-bg"] {
                assert!(
                    ratio(v(&c, "--p-on-accent"), v(&c, t)) >= 4.5,
                    "{} {t}",
                    p.id
                );
            }
        }
    }

    #[test]
    fn bg_solid_is_always_flat() {
        let mut t = theme_of("pt-default");
        t.background = "#10203099".to_string();
        for alpha in [0.3, 0.5, 153.0 / 255.0, 1.0] {
            let c = chrome_tokens(&t, alpha, None, HAIR, None, None);
            assert_eq!(v(&c, "--p-bg-solid"), "#102030", "{alpha}");
        }
    }

    #[test]
    fn there_are_no_segment_tokens() {
        assert!(CHROME_COLOUR_TOKENS
            .iter()
            .all(|n| !n.starts_with("--p-seg")));
        let c = of(preset("prism"));
        assert!(c.vars.keys().all(|n| !n.starts_with("--p-seg")));
    }

    #[test]
    fn the_knob_is_black_on_dark_white_on_light() {
        assert_eq!(v(&of(preset("pt-default")), "--p-switch-knob"), "#0b0b0f");
        assert_eq!(v(&of(preset("paper")), "--p-switch-knob"), "#ffffff");
    }

    #[test]
    fn the_knob_flips_only_under_2_to_1_on_the_on_track() {
        for p in presets() {
            let c = of(p);
            assert!(
                ratio(v(&c, "--p-switch-knob"), v(&c, "--p-sel-bg")) >= 2.0,
                "{}",
                p.id
            );
        }
        let c = with("paper", 1.0, Some("#cfe4ff80"), None);
        assert_eq!(v(&c, "--p-switch-knob"), "#0b0b0f");
        let track = composite(v(&c, "--p-sel-bg"), v(&c, "--p-bg-solid"));
        assert!(contrast_ratio("#ffffff", &track) < 2.0);
        let c = with("pt-default", 1.0, Some("#2a2c3080"), None);
        assert_eq!(v(&c, "--p-switch-knob"), "#ffffff");
        let track = composite(v(&c, "--p-sel-bg"), v(&c, "--p-bg-solid"));
        assert!(contrast_ratio("#0b0b0f", &track) < 2.0);
    }

    #[test]
    fn unpicked_black_stays_others_go_darker_and_readable() {
        for p in presets() {
            let c = of(p);
            let bg = v(&c, "--p-bg-solid");
            let sec = v(&c, "--p-secondary");
            if luminance(bg) < 0.0045 {
                assert_eq!(sec, bg, "{}", p.id);
            } else {
                assert!(luminance(sec) < luminance(bg), "{}", p.id);
            }
            assert!(ratio(v(&c, "--p-text"), sec) >= 4.5, "{}", p.id);
            assert!(ratio(v(&c, "--p-dim"), sec) >= 3.0, "{}", p.id);
        }
    }

    #[test]
    fn a_pick_is_worn_and_the_inks_are_held_to_it() {
        for p in presets() {
            let c = with(p.id, 1.0, None, Some("#3a3a3a"));
            let sec = v(&c, "--p-secondary");
            assert_eq!(sec, "#3a3a3a", "{}", p.id);
        }
        // The inks are held to the pick where the ground allows it. PT tests pt-default only: on a light
        // ground no text can clear 4.5 on both the ground and a dark pick.
        let c = with("pt-default", 1.0, None, Some("#3a3a3a"));
        assert!(ratio(v(&c, "--p-text"), "#3a3a3a") >= 4.5);
        assert!(ratio(v(&c, "--p-dim"), "#3a3a3a") >= 3.0);
    }

    #[test]
    fn on_glass_it_is_see_through_at_the_grounds_alpha() {
        let c = with("prism", 0.73, None, None);
        let sec = v(&c, "--p-secondary");
        assert_eq!(sec.len(), 9);
        assert_eq!(&sec[7..9], &v(&c, "--p-bg")[7..9]);
    }

    #[test]
    fn on_light_themes_pressed_icons_and_quiet_text_read_at_the_body_floor() {
        let mut seen = 0;
        for p in presets() {
            let c = of(p);
            if c.mode != Mode::Light {
                continue;
            }
            seen += 1;
            assert!(
                ratio(v(&c, "--p-pressed-ink"), v(&c, "--p-pressed")) >= 4.5,
                "{}",
                p.id
            );
            for g in ["--p-bg-solid", "--p-secondary", "--p-control"] {
                for t in ["--p-icon", "--p-dim"] {
                    assert!(ratio(v(&c, t), v(&c, g)) >= 4.5, "{} {t} on {g}", p.id);
                }
            }
            assert!(
                ratio(v(&c, "--p-dim2"), v(&c, "--p-bg-solid")) >= 4.5,
                "{}",
                p.id
            );
        }
        assert!(seen > 0);
    }

    fn map_of(value: &serde_json::Value) -> BTreeMap<String, String> {
        value
            .as_object()
            .expect("an object of tokens")
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().expect("a string").to_string()))
            .collect()
    }

    fn assert_vars(got: &ChromeTokens, want: &serde_json::Value, what: &str) {
        let want = map_of(want);
        let got: BTreeMap<String, String> = got
            .vars
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect();
        assert_eq!(got, want, "{what}");
    }

    #[test]
    fn oracle_alpha_sweep() {
        let sweep = fixture("chrome_alpha_sweep.json");
        let mut n_checked = 0;
        for (id, by_alpha) in sweep.as_object().unwrap() {
            let t = theme_of(id);
            for (n, want) in by_alpha.as_object().unwrap() {
                let n: f64 = n.parse().unwrap();
                let alpha = js_round(n / 100.0 * 255.0) / 255.0;
                let got = chrome_tokens(&t, alpha, None, HAIR, None, None);
                assert_vars(&got, want, &format!("{id} at {n}"));
                n_checked += 1;
            }
        }
        assert_eq!(n_checked, 4 * 71);
    }

    #[test]
    fn oracle_every_preset_four_variants() {
        let all = fixture("chrome_presets.json");
        let glass = js_round(0.6 * 255.0) / 255.0;
        for p in presets() {
            let t = resolve_preset(p);
            let want = &all[p.id];
            let a = |g: f64, pick: Option<&str>| chrome_tokens(&t, g, p.accent, HAIR, pick, None);
            assert_vars(&a(1.0, None), &want["theme"], &format!("{} theme", p.id));
            assert_vars(&a(glass, None), &want["glass"], &format!("{} glass", p.id));
            assert_vars(
                &a(1.0, Some("#e07a2f")),
                &want["picked"],
                &format!("{} picked", p.id),
            );
            assert_vars(
                &a(glass, Some("#e07a2f")),
                &want["pickedGlass"],
                &format!("{} pickedGlass", p.id),
            );
        }
    }

    #[test]
    fn oracle_edges_picks_and_levels() {
        let edges = fixture("chrome_edges.json");
        let mut count = 0;
        for (id, by_word) in edges.as_object().unwrap() {
            let p = preset(id);
            for (word, want) in by_word.as_object().unwrap() {
                let e = WindowEdges::from_word(word).unwrap();
                let got = chrome_tokens(&resolve_preset(p), 1.0, p.accent, e, None, None);
                assert_vars(&got, want, &format!("{id} {word}"));
                count += 1;
            }
        }
        assert_eq!(count, 12);

        let levels = fixture("chrome_levels.json");
        count = 0;
        for (id, by_level) in levels.as_object().unwrap() {
            let p = preset(id);
            for (level, want) in by_level.as_object().unwrap() {
                let level: f64 = level.parse().unwrap();
                let got = chrome_tokens(
                    &resolve_preset(p),
                    1.0 - level / 100.0,
                    p.accent,
                    HAIR,
                    None,
                    None,
                );
                assert_vars(&got, want, &format!("{id} level {level}"));
                count += 1;
            }
        }
        assert_eq!(count, 32);

        let picks = fixture("chrome_picks.json");
        let rows = picks.as_array().unwrap();
        assert_eq!(rows.len(), 88);
        for row in rows {
            let mut t = theme_of("pt-default");
            if let Some(bg) = row["bg"].as_str() {
                t.background = bg.to_string();
            }
            let accent = row["accent"].as_str();
            let secondary = row["secondary"].as_str();
            let alpha = row["alpha"].as_f64().unwrap();
            let got = chrome_tokens(&t, alpha, Some("#fe8f34"), HAIR, accent, secondary);
            assert_vars(
                &got,
                &row["vars"],
                &format!("pick {accent:?} {secondary:?} {alpha} {:?}", row["bg"]),
            );
        }
    }

    #[test]
    fn sidebar_names_read_at_7_and_folders_at_4_5_on_the_sidebar_and_its_pill() {
        for p in presets() {
            let c = of(p);
            let side = &v(&c, "--p-secondary")[..7];
            let pill = mix_hex(side, v(&c, "--p-accent-hi"), 0.18);
            for g in [side, pill.as_str()] {
                assert!(
                    ratio(v(&c, "--p-side-text"), g) >= 7.0,
                    "{} text on {g}",
                    p.id
                );
                assert!(
                    ratio(v(&c, "--p-side-dim"), g) >= 4.5,
                    "{} dim on {g}",
                    p.id
                );
            }
        }
    }

    #[test]
    fn a_too_dark_pick_is_what_the_window_paints_its_saves_in() {
        let c = with("pt-default", 1.0, Some("#140707"), None);
        let [r, g, b] = channels(v(&c, "--p-accent"));
        assert!(r - g > 50, "{r} {g}");
        assert!(r - b > 50, "{r} {b}");
        assert!(ratio(v(&c, "--p-on-accent"), v(&c, "--p-accent")) >= 4.5);
    }

    #[test]
    fn the_ink_on_the_accent_is_white_on_strong_red_or_blue_dark_on_pale() {
        for pick in ["#ff0000", "#3b78ff"] {
            let c = with("pt-default", 1.0, Some(pick), None);
            assert_eq!(v(&c, "--p-on-accent"), "#ffffff", "{pick}");
        }
        for pick in ["#fe8f34", "#f5d90a"] {
            let c = with("pt-default", 1.0, Some(pick), None);
            assert_ne!(v(&c, "--p-on-accent"), "#ffffff", "{pick}");
        }
    }

    #[test]
    fn the_on_switch_ink_is_4_5_on_the_track_and_the_track_3_on_the_ground() {
        for p in presets() {
            let c = of(p);
            assert!(
                ratio(v(&c, "--p-on-accent"), v(&c, "--p-sel-bg")) >= 4.5,
                "{}",
                p.id
            );
            assert!(
                ratio(v(&c, "--p-sel-bg"), v(&c, "--p-bg-solid")) >= 3.0,
                "{}",
                p.id
            );
        }
    }

    #[test]
    fn the_knob_is_the_themes_ink_at_least_2_on_the_track() {
        for p in presets() {
            let c = of(p);
            let knob = v(&c, "--p-switch-knob");
            assert!(knob == "#0b0b0f" || knob == "#ffffff", "{}", p.id);
            let ink = if c.mode == Mode::Dark {
                "#0b0b0f"
            } else {
                "#ffffff"
            };
            if knob != ink {
                assert!(ratio(ink, v(&c, "--p-sel-bg")) < 2.0, "{}", p.id);
            }
            assert!(ratio(knob, v(&c, "--p-sel-bg")) >= 2.0, "{}", p.id);
        }
    }
}
