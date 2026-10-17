//! Resolving a theme: preset, Custom and the picked ground (Task 10).
//! Source: PT `core/renderer/lib/termTheme.ts` (the resolvers and `presetAccent`) and `termGround.ts`.

use crate::ansi::{PartialAnsi, derive_ansi, ensure_contrast, legible_palette, normalize_color};
use crate::colour::{opaque, parse_colour};
use crate::colour_contrast::{composite, legible_on};
use crate::ground::DEFAULT_GROUND;
use crate::preset::{DEFAULT_TERM_THEME, HOST_DEFAULT_THEME, Preset, find_preset};
use crate::retired::live_theme_id;

/// Text needs 4.5:1, as everywhere else in the app.
const TEXT_FLOOR: f64 = 4.5;
/// The cursor and the sixteen need 3:1, the ANSI floor.
const MARK_FLOOR: f64 = 3.0;

/// A theme as the terminal draws it: the four base colours and the sixteen (an absent one is `None`).
#[derive(Clone, Debug, PartialEq)]
pub struct TermTheme {
    pub background: String,
    pub foreground: String,
    pub cursor: String,
    pub selection_background: String,
    pub ansi: PartialAnsi,
}

impl TermTheme {
    /// The ANSI colour at `i` (0 to 15), if the theme has one.
    pub fn ansi_at(&self, i: usize) -> Option<&str> {
        self.ansi.get(i).and_then(|c| c.as_deref())
    }
    pub fn blue(&self) -> Option<&str> {
        self.ansi_at(crate::ansi::idx::BLUE)
    }
    pub fn red(&self) -> Option<&str> {
        self.ansi_at(crate::ansi::idx::RED)
    }
    pub fn green(&self) -> Option<&str> {
        self.ansi_at(crate::ansi::idx::GREEN)
    }
    pub fn magenta(&self) -> Option<&str> {
        self.ansi_at(crate::ansi::idx::MAGENTA)
    }
}

/// The palette half of a Custom (Task 16's CustomTheme wraps it with the extras).
#[derive(Clone, Debug, PartialEq)]
pub struct CustomPalette {
    pub bg: String,
    pub fg: String,
    pub cursor: String,
    pub selection: Option<String>,
    pub ansi: PartialAnsi,
    pub accent: Option<String>,
}

/// A preset as saved: fg and cursor as written, selection the cursor at 55, the sixteen walked to the floor.
pub fn resolve_preset(p: &Preset) -> TermTheme {
    let sixteen: PartialAnsi = match p.ansi {
        Some(a) => a.map(|c| Some(c.to_string())),
        None => derive_ansi(p.bg, p.fg).map(Some),
    };
    TermTheme {
        background: p.bg.to_string(),
        foreground: p.fg.to_string(),
        cursor: p.cursor.to_string(),
        selection_background: format!("{}55", p.cursor),
        ansi: legible_palette(&sixteen, p.bg),
    }
}

/// A Custom palette as the terminal draws it (PT `resolveCustomTheme`). Its text, cursor and sixteen are
/// composited once against the ground in force and handed on opaque.
pub fn resolve_custom_theme(c: &CustomPalette, ground: Option<&str>) -> TermTheme {
    let own = opaque(&c.bg, DEFAULT_GROUND);
    let g = match ground {
        Some(gr) if parse_colour(gr).is_some() => opaque(gr, DEFAULT_GROUND),
        _ => own.clone(),
    };
    let cursor = legible_on(&c.cursor, &g, MARK_FLOOR);
    let composited: PartialAnsi = c.ansi.clone().map(|e| e.map(|v| composite(&v, &g)));
    TermTheme {
        background: c.bg.clone(),
        foreground: legible_on(&c.fg, &g, TEXT_FLOOR),
        selection_background: c.selection.clone().unwrap_or_else(|| format!("{cursor}55")),
        cursor,
        ansi: legible_palette(&composited, &own),
    }
}

/// A theme as saved, never the editor's draft. "custom" with a saved one is the Custom; otherwise the preset
/// of the live id, else the host default's, else the core's.
pub fn resolve_saved_theme(
    id: &str,
    custom: Option<&CustomPalette>,
    ground: Option<&str>,
) -> TermTheme {
    if let (true, Some(c)) = (id == "custom", custom) {
        return resolve_custom_theme(c, ground);
    }
    let p = find_preset(live_theme_id(id))
        .or_else(|| find_preset(HOST_DEFAULT_THEME))
        .or_else(|| find_preset(DEFAULT_TERM_THEME))
        .unwrap_or(&crate::preset::presets()[0]);
    resolve_preset(p)
}

/// The colour editor's draft is worn for the theme in force only.
pub fn resolve_theme(
    id: &str,
    in_force: &str,
    custom: Option<&CustomPalette>,
    draft: Option<&CustomPalette>,
    ground: Option<&str>,
) -> TermTheme {
    if let (Some(d), true) = (draft, id == in_force) {
        return resolve_custom_theme(d, ground);
    }
    resolve_saved_theme(id, custom, ground)
}

/// The chrome accent a theme asks for: the draft's (for the theme in force), a Custom's own, or the preset's.
pub fn preset_accent(
    id: &str,
    in_force: &str,
    custom: Option<&CustomPalette>,
    draft: Option<&CustomPalette>,
) -> Option<String> {
    if let (Some(d), true) = (draft, id == in_force) {
        return d.accent.clone();
    }
    if id == "custom" {
        return custom.and_then(|c| c.accent.clone());
    }
    find_preset(live_theme_id(id)).and_then(|p| p.accent.map(str::to_string))
}

/// The theme on the ground it is actually painted on (PT `onGround`): text, cursor and the sixteen are
/// floored again against the picked ground, each moved only as far as it must be.
pub fn on_ground(theme: &TermTheme, ground: Option<&str>) -> TermTheme {
    let Some(ground) = ground.filter(|g| !g.is_empty()) else {
        return theme.clone();
    };
    let own = normalize_color(&theme.background, DEFAULT_GROUND).to_ascii_lowercase();
    let bg = normalize_color(ground, &own).to_ascii_lowercase();
    if bg == own {
        return theme.clone();
    }
    let own_cursor = normalize_color(&theme.cursor, "#5b5bd6");
    let cursor = ensure_contrast(&own_cursor, &bg, MARK_FLOOR);
    let derived = theme.selection_background.to_ascii_lowercase()
        == format!("{}55", own_cursor.to_ascii_lowercase());
    let mut ansi: PartialAnsi = Default::default();
    for (i, slot) in theme.ansi.iter().enumerate() {
        ansi[i] = slot
            .as_ref()
            .map(|v| ensure_contrast(&normalize_color(v, "#888888"), &bg, MARK_FLOOR));
    }
    TermTheme {
        foreground: ensure_contrast(
            &normalize_color(&theme.foreground, "#d7dae1"),
            &bg,
            TEXT_FLOOR,
        ),
        selection_background: if derived {
            format!("{cursor}55")
        } else {
            theme.selection_background.clone()
        },
        cursor,
        background: bg,
        ansi,
    }
}

/// A theme from the oracle's JSON: keys background foreground cursor selectionBackground and the sixteen.
#[cfg(test)]
pub(crate) fn theme_from_json(v: &serde_json::Value) -> TermTheme {
    use crate::ansi::ANSI_KEYS;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let mut ansi: PartialAnsi = Default::default();
    for (i, k) in ANSI_KEYS.iter().enumerate() {
        ansi[i] = s(k);
    }
    TermTheme {
        background: s("background").expect("background"),
        foreground: s("foreground").expect("foreground"),
        cursor: s("cursor").expect("cursor"),
        selection_background: s("selectionBackground").expect("selectionBackground"),
        ansi,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ansi::{ANSI_KEYS, contrast_ratio};
    use crate::colour::with_alpha;
    use crate::preset::presets;
    use crate::testutil::fixture;

    fn preset(id: &str) -> &'static Preset {
        find_preset(id).expect("preset")
    }

    fn hex6(s: &str) -> bool {
        s.len() == 7
            && s.starts_with('#')
            && s[1..].chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
    }

    fn hex6_or_8(s: &str) -> bool {
        let ok = |n: usize| {
            s.len() == n + 1
                && s.starts_with('#')
                && s[1..].chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
        };
        ok(6) || ok(8)
    }

    fn all_strings(t: &TermTheme) -> Vec<&str> {
        let mut v = vec![
            t.background.as_str(),
            t.foreground.as_str(),
            t.cursor.as_str(),
            t.selection_background.as_str(),
        ];
        v.extend(t.ansi.iter().flatten().map(String::as_str));
        v
    }

    fn ansi_of(pairs: &[(usize, &str)]) -> PartialAnsi {
        let mut a: PartialAnsi = Default::default();
        for (i, c) in pairs {
            a[*i] = Some(c.to_string());
        }
        a
    }

    fn pt_custom() -> CustomPalette {
        CustomPalette {
            bg: "#000000".into(),
            fg: "#ffffff4d".into(),
            cursor: "#ff800080".into(),
            selection: None,
            ansi: ansi_of(&[(1, "#ff000066"), (2, "#00ff00"), (4, "#3355ff80")]),
            accent: None,
        }
    }

    fn custom_of(t: &TermTheme, accent: Option<&str>) -> CustomPalette {
        CustomPalette {
            bg: t.background.clone(),
            fg: t.foreground.clone(),
            cursor: t.cursor.clone(),
            selection: None,
            ansi: t.ansi.clone(),
            accent: accent.map(str::to_string),
        }
    }

    #[test]
    fn resolves_a_preset_with_a_full_sixteen() {
        let t = resolve_saved_theme("dracula", None, None);
        assert_eq!(t.background, "#1e1f29");
        assert_eq!(t.selection_background, "#bbbbbb55");
        assert!((0..16).all(|i| t.ansi_at(i).is_some()));
    }

    #[test]
    fn an_unknown_id_and_legacy_style_resolve_to_the_host_default() {
        let d = resolve_preset(preset(HOST_DEFAULT_THEME));
        assert_eq!(resolve_saved_theme("no-such-theme", None, None), d);
        assert_eq!(resolve_saved_theme("style", None, None), d);
        let p = resolve_saved_theme("prism", None, None);
        assert_eq!(p.background, "#0b0b0f");
        assert_eq!(p.foreground, "#e7e7ee");
        assert_eq!(p.cursor, "#7c7cf0");
    }

    #[test]
    fn custom_with_nothing_saved_falls_back_the_same_way() {
        assert_eq!(
            resolve_saved_theme("custom", None, None),
            resolve_preset(preset(HOST_DEFAULT_THEME))
        );
    }

    #[test]
    fn a_hex8_cursor_derives_a_nine_char_selection_from_the_opaque_cursor() {
        let c = CustomPalette {
            bg: "#101010".into(),
            fg: "#eeeeee".into(),
            cursor: "#ff800080".into(),
            selection: None,
            ansi: Default::default(),
            accent: None,
        };
        let t = resolve_custom_theme(&c, None);
        assert_eq!(t.selection_background.len(), 9);
        assert!(hex6(&t.cursor));
        assert_eq!(t.selection_background, format!("{}55", t.cursor));
    }

    #[test]
    fn a_chosen_selection_is_honoured_and_survives_a_picked_ground() {
        let mut c = pt_custom();
        c.selection = Some("#3355ff80".into());
        let t = resolve_custom_theme(&c, None);
        assert_eq!(t.selection_background, "#3355ff80");
        assert_eq!(
            on_ground(&t, Some("#f4f1e8")).selection_background,
            "#3355ff80"
        );
    }

    #[test]
    fn a_derived_selection_follows_the_floored_cursor_on_a_picked_ground() {
        let t = on_ground(&resolve_preset(preset("dracula")), Some("#f4f1e8"));
        assert_eq!(t.selection_background, format!("{}55", t.cursor));
    }

    #[test]
    fn foreground_cursor_and_sixteen_are_composited_against_the_ground_in_force() {
        let c = CustomPalette {
            bg: "#000000".into(),
            fg: "#ffffff80".into(),
            cursor: "#ffffff".into(),
            selection: None,
            ansi: ansi_of(&[(1, "#ff000080")]),
            accent: None,
        };
        let t = resolve_custom_theme(&c, None);
        assert_eq!(t.foreground, "#808080");
        let red = t.red().expect("red");
        assert!(hex6(red));
        assert!(contrast_ratio(red, "#000000") >= 3.0);
        let w = resolve_custom_theme(&c, Some("#ffffff"));
        assert_ne!(w.foreground, t.foreground);
    }

    #[test]
    fn resolve_custom_equals_what_the_terminal_resolves() {
        let c = pt_custom();
        for g in [None, Some("#f4f1e8")] {
            assert_eq!(
                resolve_theme("custom", "custom", Some(&c), None, g),
                resolve_custom_theme(&c, g)
            );
        }
    }

    #[test]
    fn a_30_percent_foreground_is_drawn_floored() {
        let t = resolve_custom_theme(&pt_custom(), None);
        assert!(hex6(&t.foreground));
        assert!(contrast_ratio(&t.foreground, "#000000") >= 4.5);
    }

    #[test]
    fn the_derived_selection_is_the_drawn_cursor_at_55() {
        let t = resolve_custom_theme(&pt_custom(), None);
        assert_eq!(t.selection_background, format!("{}55", t.cursor));
        assert_ne!(t.selection_background, "#ff800055");
    }

    #[test]
    fn every_resolved_value_is_hex6_or_hex8() {
        let alpha = CustomPalette {
            bg: "#10101080".into(),
            fg: "#EEEEEE99".into(),
            cursor: "#ff800040".into(),
            selection: Some("#3355ff80".into()),
            ansi: ansi_of(&[
                (1, "#ff000080"),
                (2, "#00ff00"),
                (4, "rgba(0, 0, 255, 0.5)"),
            ]),
            accent: None,
        };
        let grounds = [None, Some("#f4f1e8"), Some("#20304080")];
        for g in grounds {
            let mut themes = vec![resolve_custom_theme(&alpha, g)];
            for p in presets() {
                themes.push(resolve_preset(p));
            }
            for t in themes {
                for x in [t.clone(), on_ground(&t, g)] {
                    for s in all_strings(&x) {
                        assert!(hex6_or_8(s), "{s} on {g:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_custom_at_every_alpha_reads() {
        for p in presets() {
            let base = resolve_preset(p);
            for i in 1..=9 {
                let a = f64::from(i) / 10.0;
                let c = CustomPalette {
                    bg: p.bg.to_string(),
                    fg: with_alpha(&base.foreground, a),
                    cursor: with_alpha(&base.cursor, a),
                    selection: None,
                    ansi: base.ansi.clone().map(|e| e.map(|v| with_alpha(&v, a))),
                    accent: None,
                };
                let t = resolve_custom_theme(&c, None);
                assert!(hex6(&t.foreground) && hex6(&t.cursor), "{} {i}", p.id);
                let fg_want = 4.5_f64.min(contrast_ratio(&base.foreground, p.bg)) - 0.02;
                assert!(
                    contrast_ratio(&t.foreground, p.bg) >= fg_want,
                    "{} {i} fg",
                    p.id
                );
                let cu_want = 3.0_f64.min(contrast_ratio(&base.cursor, p.bg)) - 0.02;
                assert!(
                    contrast_ratio(&t.cursor, p.bg) >= cu_want,
                    "{} {i} cursor",
                    p.id
                );
                for k in 0..16 {
                    let v = t.ansi_at(k).expect("sixteen");
                    assert!(hex6(v), "{} {i} {k}", p.id);
                    assert!(contrast_ratio(v, p.bg) >= 2.99, "{} {i} {k}", p.id);
                }
            }
        }
    }

    #[test]
    fn all_sixteen_of_every_preset_clear_the_floor_against_its_background() {
        assert_eq!(presets().len(), 39);
        for p in presets() {
            let t = resolve_preset(p);
            for k in 0..16 {
                let v = t.ansi_at(k).expect("sixteen");
                assert!(contrast_ratio(v, p.bg) >= 2.99, "{} {k} {v}", p.id);
            }
        }
    }

    #[test]
    fn legible_palette_leaves_dracula_bright_blue_and_nudges_bright_black() {
        let p = preset("dracula");
        let curated = p.ansi.expect("curated");
        let t = resolve_preset(p);
        assert_eq!(t.ansi_at(12), Some(curated[12]));
        assert_ne!(t.ansi_at(8), Some(curated[8]));
        assert!(contrast_ratio(t.ansi_at(8).expect("bb"), p.bg) >= 2.99);
    }

    fn from_fixture(name: &str) -> Vec<(String, TermTheme)> {
        let j = fixture(name);
        j.as_object()
            .expect("object")
            .iter()
            .map(|(k, v)| (k.clone(), theme_from_json(v)))
            .collect()
    }

    #[test]
    fn oracle_every_preset_resolves_as_it_did() {
        let want = from_fixture("resolved_presets.json");
        assert_eq!(want.len(), 39);
        for (id, t) in want {
            assert_eq!(resolve_preset(preset(&id)), t, "{id}");
        }
    }

    fn json_custom(v: &serde_json::Value) -> CustomPalette {
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
        let mut ansi: PartialAnsi = Default::default();
        if let Some(o) = v.get("ansi").and_then(|a| a.as_object()) {
            for (i, k) in ANSI_KEYS.iter().enumerate() {
                ansi[i] = o.get(*k).and_then(|x| x.as_str()).map(str::to_string);
            }
        }
        CustomPalette {
            bg: s("bg").expect("bg"),
            fg: s("fg").expect("fg"),
            cursor: s("cursor").expect("cursor"),
            selection: s("selection"),
            ansi,
            accent: None,
        }
    }

    #[test]
    fn oracle_opaque_custom_with_a_failing_cursor() {
        let j = fixture("custom_opaque.json");
        let c = json_custom(&j["input"]);
        let want = theme_from_json(&j["resolved"]);
        let got = resolve_custom_theme(&c, None);
        assert_eq!(got, want);
        assert_eq!(got.cursor, "#20212c");
        assert_eq!(got.selection_background, "#20212c55");
    }

    fn check_on_grounds(name: &str, custom: Option<&CustomPalette>) {
        let want = from_fixture(name);
        for (key, t) in want {
            let (id, ground) = key.split_once('@').expect("key");
            let base = if id == "custom" {
                resolve_custom_theme(custom.expect("custom"), None)
            } else {
                resolve_preset(preset(id))
            };
            assert_eq!(on_ground(&base, Some(ground)), t, "{key}");
        }
    }

    #[test]
    fn oracle_picked_grounds() {
        let j = fixture("custom_opaque.json");
        let c = json_custom(&j["input"]);
        check_on_grounds("on_grounds.json", Some(&c));
        assert_eq!(fixture("on_grounds.json").as_object().expect("o").len(), 80);
    }

    #[test]
    fn oracle_on_ground_and_custom_alpha_vectors() {
        check_on_grounds("on_ground_vectors.json", None);
        let want = from_fixture("custom_alpha_vectors.json");
        assert_eq!(want.len(), 351);
        for (key, t) in want {
            let (id, i) = key.split_once('@').expect("key");
            let a = i.parse::<f64>().expect("i") / 10.0;
            let p = preset(id);
            let base = resolve_preset(p);
            let c = CustomPalette {
                bg: p.bg.to_string(),
                fg: with_alpha(p.fg, a),
                cursor: with_alpha(p.cursor, a),
                selection: None,
                ansi: base.ansi.map(|e| e.map(|v| with_alpha(&v, a))),
                accent: None,
            };
            assert_eq!(resolve_custom_theme(&c, None), t, "{key}");
        }
    }

    #[test]
    fn on_ground_leaves_the_theme_alone_with_no_ground_or_its_own() {
        let t = resolve_preset(preset("nord"));
        assert_eq!(on_ground(&t, None), t);
        assert_eq!(on_ground(&t, Some(&t.background.to_ascii_uppercase())), t);
    }

    #[test]
    fn every_preset_reads_on_a_picked_ground() {
        for g in ["#f4f1e8", "#101010", "#1d3fbf"] {
            for p in presets() {
                let t = on_ground(&resolve_preset(p), Some(g));
                assert_eq!(t.background, g);
                assert!(contrast_ratio(&t.foreground, g) >= 4.5, "{} {g}", p.id);
                assert!(contrast_ratio(&t.cursor, g) >= 3.0, "{} {g}", p.id);
                for k in 0..16 {
                    let v = t.ansi_at(k).expect("sixteen");
                    assert!(contrast_ratio(v, g) >= 3.0, "{} {g} {k}", p.id);
                }
            }
        }
    }

    #[test]
    fn on_ground_moves_only_what_fails() {
        let base = resolve_preset(preset("pitch"));
        let t = on_ground(&base, Some("#101010"));
        for k in 0..16 {
            let before = base.ansi_at(k).expect("b");
            if contrast_ratio(before, "#101010") >= 3.0 {
                assert_eq!(
                    t.ansi_at(k),
                    Some(before.to_ascii_lowercase().as_str()),
                    "{k}"
                );
            }
        }
    }

    #[test]
    fn a_retired_wearer_resolves_to_the_replacement_not_the_default() {
        assert_eq!(
            resolve_saved_theme("espresso", None, None).background,
            preset("umber").bg
        );
    }

    #[test]
    fn a_custom_keeps_the_accent_of_the_theme_it_was_made_from() {
        let c = custom_of(&resolve_preset(preset("pt-default")), Some("#fe8f34"));
        assert_eq!(
            preset_accent("custom", "custom", Some(&c), None),
            Some("#fe8f34".to_string())
        );
    }

    #[test]
    fn an_older_custom_with_no_accent_asks_for_none() {
        let c = custom_of(&resolve_preset(preset("pt-default")), None);
        assert_eq!(preset_accent("custom", "custom", Some(&c), None), None);
    }

    #[test]
    fn the_draft_is_worn_for_the_theme_in_force_only() {
        let saved = resolve_theme("pt-default", "pt-default", None, None, None);
        let mut d = custom_of(&saved, None);
        d.cursor = "#ff00aa".into();
        let worn = resolve_theme("pt-default", "pt-default", None, Some(&d), None);
        assert_ne!(worn.cursor, saved.cursor);
        assert_eq!(
            resolve_theme("prism", "pt-default", None, Some(&d), None),
            resolve_saved_theme("prism", None, None)
        );
    }

    #[test]
    fn preset_accent_follows_the_draft_while_it_is_open() {
        let saved = resolve_theme("pt-default", "pt-default", None, None, None);
        let d = custom_of(&saved, Some("#00ff88"));
        assert_eq!(
            preset_accent("pt-default", "pt-default", None, Some(&d)),
            Some("#00ff88".to_string())
        );
        assert_eq!(
            preset_accent("pt-default", "pt-default", None, None),
            Some("#fe8f34".to_string())
        );
    }
}
