//! The two legacy migrations of the first-run import (M7 Task 15).
//!
//! Source: Prism Terminal's `opacityMigration.ts` and `seeThroughMigration.ts` (every comment there is
//! a reviewed decision) with `termLook.ts` (`opacityPct`, `paintsAlpha`, `seeThroughLevel`). They fold
//! the retired Opacity slider and the retired acrylic switch into the see-through level, so nobody's
//! window changes on import.
//!
//! This file reads Prism Terminal's legacy keys only to fold them. It never writes the retired acrylic
//! or opacity keys, and the result map has neither. The map is the source's text as Local Storage
//! held it, keyed by the old names.

use std::collections::BTreeMap;

use flare_theme::colour::{alpha_of, canonical_stored, opaque, parse_colour, with_alpha};
use flare_theme::ground::DEFAULT_GROUND;
use flare_theme::num::js_round;
use flare_theme::preset::HOST_DEFAULT_THEME;
use flare_theme::resolve::resolve_saved_theme;
use flare_theme::retired::live_theme_id;
use flare_theme::see_through::{SEE_THROUGH_TOP, paints_alpha};
use serde_json::{Map, Value};

use crate::custom_theme::parse_custom;

pub use flare_theme::see_through::{
    SEE_THROUGH_ALPHA_DARK, SEE_THROUGH_ALPHA_LIGHT, see_through_alpha,
};

/// Prism Terminal's keys to their text, exactly as Local Storage held it.
pub type Source = BTreeMap<String, String>;

const OPACITY: &str = "prism.term.opacity";
const ACRYLIC: &str = "prism.term.acrylic";
const THEME: &str = "prism.term.theme";
const CUSTOM: &str = "prism.term.custom";
const LEVEL: &str = "prism.term.seeThrough";
const PICKED: &str = "prism.window.background";
const KEPT: &str = "prism.window.customColours";

/// JavaScript's `Number(text)`: blank is 0, junk is NaN.
fn js_number(text: &str) -> f64 {
    let t = text.trim();
    if t.is_empty() {
        return 0.0;
    }
    t.parse::<f64>().unwrap_or(f64::NAN)
}

fn pct_of(n: f64) -> u32 {
    if n.is_finite() {
        js_round(n).clamp(30.0, 100.0) as u32
    } else {
        100
    }
}

/// A stored opacity as the window read it: None, empty and junk read 100, else rounded into 30..=100.
pub fn opacity_pct(raw: Option<&str>) -> u32 {
    match raw {
        None | Some("") => 100,
        Some(t) => pct_of(js_number(t)),
    }
}

/// The same read for a value inside a saved Custom's JSON.
fn opacity_pct_value(v: &Value) -> u32 {
    match v {
        Value::Number(n) => pct_of(n.as_f64().unwrap_or(f64::NAN)),
        Value::String(t) if t.is_empty() => 100,
        Value::String(t) => pct_of(js_number(t)),
        Value::Bool(b) => pct_of(if *b { 1.0 } else { 0.0 }),
        _ => 100,
    }
}

fn same_byte(a: f64, b: f64) -> bool {
    js_round(a * 255.0) == js_round(b * 255.0)
}

/// The picked Background as the window read it: the stored form of a 6 or 8 digit hex, or None.
fn picked(s: &Source) -> Option<String> {
    canonical_stored(s.get(PICKED).map(String::as_str))
}

fn set_picked(s: &mut Source, hex: &str) {
    match canonical_stored(Some(hex)) {
        Some(v) => {
            s.insert(PICKED.to_string(), v);
        }
        None => {
            s.remove(PICKED);
        }
    }
}

/// The theme id in force: never touched or `style` reads as the host's default, a retired id as its heir.
fn theme_id(s: &Source) -> String {
    match s.get(THEME).map(String::as_str) {
        None | Some("") | Some("style") => HOST_DEFAULT_THEME.to_string(),
        Some(v) => live_theme_id(v).to_string(),
    }
}

/// The saved Custom's ground (the Custom's own `bg`, as the reader keeps it), only while Custom is the theme.
fn own_bg(s: &Source) -> Option<String> {
    if theme_id(s) != "custom" {
        return None;
    }
    s.get(CUSTOM)
        .and_then(|raw| parse_custom(raw))
        .map(|c| c.palette.bg)
}

/// The ground of the theme in force, resolved as the window resolves it.
fn theme_ground(s: &Source) -> String {
    let custom = s
        .get(CUSTOM)
        .and_then(|raw| parse_custom(raw))
        .map(|c| c.palette);
    resolve_saved_theme(&theme_id(s), custom.as_ref(), None).background
}

/// The saved Custom as a JSON object, or None when absent, corrupt or not an object.
fn custom_object(s: &Source) -> Option<Map<String, Value>> {
    let raw = s.get(CUSTOM).filter(|r| !r.is_empty())?;
    match serde_json::from_str::<Value>(raw).ok()? {
        Value::Object(m) => Some(m),
        _ => None,
    }
}

fn store_custom(s: &mut Source, m: Map<String, Value>) {
    s.insert(CUSTOM.to_string(), Value::Object(m).to_string());
}

/// The opacity slider becomes the Background's alpha. Folds the saved Custom, then the live value;
/// removes the live key; idempotent.
pub fn migrate_opacity(s: &mut Source) {
    let folded = fold_saved_custom_opacity(s);
    fold_live(s, folded);
}

/// The saved Custom's opacity folds into its own `bg` when it was saved with acrylic (on, or never
/// said). The field then goes. Answers whether it gave `bg` an alpha.
fn fold_saved_custom_opacity(s: &mut Source) -> bool {
    let Some(mut c) = custom_object(s) else {
        return false;
    };
    let Some(opacity) = c.remove("opacity") else {
        return false;
    };
    let n = opacity_pct_value(&opacity);
    let mut folded = false;
    if n < 100 && c.get("acrylic") != Some(&Value::Bool(false)) {
        let bg = c.get("bg").and_then(Value::as_str).map(str::to_string);
        if let Some(bg) = bg.filter(|b| parse_colour(b).is_some()) {
            c.insert(
                "bg".to_string(),
                Value::String(with_alpha(&bg, f64::from(n) / 100.0)),
            );
            folded = true;
        }
    }
    store_custom(s, c);
    folded
}

/// The live value, which is what was painting (100 when never stored). Only with acrylic on did it
/// show; then it goes onto the picked Background, or nowhere when the Custom's own `bg` carries the
/// same alpha, or onto a picked Background that is the theme's own opaque ground.
fn fold_live(s: &mut Source, custom_folded: bool) {
    let has_live = s.contains_key(OPACITY);
    if !has_live && !custom_folded {
        return;
    }
    let n = opacity_pct(s.get(OPACITY).map(String::as_str));
    if s.get(ACRYLIC).map(String::as_str) == Some("1") {
        let a = f64::from(n) / 100.0;
        if let Some(p) = picked(s) {
            if n < 100 {
                set_picked(s, &with_alpha(&p, a));
            }
        } else {
            let needs = match own_bg(s) {
                Some(bg) => !same_byte(alpha_of(&bg), a),
                None => n < 100,
            };
            if needs {
                let ground = opaque(&theme_ground(s), DEFAULT_GROUND);
                set_picked(s, &with_alpha(&ground, a));
            }
        }
    }
    if has_live {
        s.remove(OPACITY);
    }
}

/// The level an old alpha painted at: never 0, which would turn it off.
fn level_of(alpha: f64) -> u8 {
    js_round((1.0 - alpha) * 100.0).clamp(1.0, f64::from(SEE_THROUGH_TOP)) as u8
}

/// The switch and the Background's alpha become the see-through slider. Runs only when no level is
/// stored (a stored level is the sign it has run), and writes one. Removes the acrylic key.
pub fn migrate_see_through(s: &mut Source) {
    if s.contains_key(LEVEL) {
        return;
    }
    let was_on = s.get(ACRYLIC).map(String::as_str) == Some("1");
    let pick = picked(s);
    let own = own_bg(s);
    let mut level = 0u8;
    if was_on {
        let raw = match (&pick, &own) {
            (Some(p), _) => alpha_of(p),
            (None, Some(o)) => alpha_of(o),
            _ => 1.0,
        };
        let base = pick
            .clone()
            .or_else(|| own.clone())
            .unwrap_or_else(|| theme_ground(s));
        let ground = opaque(&base, DEFAULT_GROUND);
        level = level_of(paints_alpha(raw, &ground, true));
    }
    fold_saved_custom_glass(s);
    if let Some(p) = &pick
        && alpha_of(p) < 1.0
    {
        set_picked(s, &opaque(p, DEFAULT_GROUND));
    }
    s.insert(LEVEL.to_string(), level.to_string());
    s.remove(ACRYLIC);
}

/// The window colours Custom keeps, read as the reader does: a 6 or 8 digit hex each, else none.
fn kept_colours(s: &Source) -> Option<[Option<String>; 3]> {
    let v: Value = serde_json::from_str(s.get(KEPT)?).ok()?;
    let one = |k: &str| -> Option<String> {
        let t = v.get(k)?.as_str()?;
        let h = t.strip_prefix('#')?;
        ((h.len() == 6 || h.len() == 8) && h.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| t.to_lowercase())
    };
    Some([one("background"), one("accent"), one("secondary")])
}

fn store_kept(s: &mut Source, c: &[Option<String>; 3]) {
    let mut m = Map::new();
    for (k, v) in ["background", "accent", "secondary"].iter().zip(c) {
        m.insert(
            (*k).to_string(),
            v.clone().map_or(Value::Null, Value::String),
        );
    }
    s.insert(KEPT.to_string(), Value::Object(m).to_string());
}

/// The saved Custom: its switch and its ground's alpha (or the Background it keeps, which the window
/// wore over it) become its `seeThrough`, its ground opaque. A save that never said acrylic read off.
fn fold_saved_custom_glass(s: &mut Source) {
    let mut kept = kept_colours(s).unwrap_or([None, None, None]);
    let kept_bg = kept[0].clone();
    if let Some(bg) = &kept_bg
        && alpha_of(bg) < 1.0
    {
        kept[0] = Some(opaque(bg, DEFAULT_GROUND));
        store_kept(s, &kept);
    }
    let Some(mut c) = custom_object(s) else {
        return;
    };
    let Some(bg) = c
        .get("bg")
        .and_then(Value::as_str)
        .filter(|b| parse_colour(b).is_some())
        .map(str::to_string)
    else {
        return;
    };
    let acrylic = c.remove("acrylic");
    let alpha = alpha_of(kept_bg.as_deref().unwrap_or(&bg));
    let ground = opaque(kept_bg.as_deref().unwrap_or(&bg), DEFAULT_GROUND);
    let level = if acrylic == Some(Value::Bool(true)) {
        level_of(paints_alpha(alpha, &ground, true))
    } else {
        0
    };
    c.insert("seeThrough".to_string(), Value::from(level));
    c.insert("bg".to_string(), Value::String(opaque(&bg, DEFAULT_GROUND)));
    store_custom(s, c);
}

/// Both, in the order the app runs them.
pub fn migrate(s: &mut Source) {
    migrate_opacity(s);
    migrate_see_through(s);
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_theme::see_through::{SEE_THROUGH_MAX, see_through_level};

    /// The byte a saved N becomes: round(N / 100 * 255), what N painted.
    fn byte(n: u32) -> String {
        format!("{:02x}", js_round(f64::from(n) / 100.0 * 255.0) as u32)
    }
    fn custom(extra: Value) -> String {
        let mut m = serde_json::json!({
            "bg": "#1d1f21", "fg": "#c5c8c6", "cursor": "#ff0000", "ansi": { "red": "#cc6666" }
        });
        for (k, v) in extra.as_object().unwrap() {
            m[k] = v.clone();
        }
        m.to_string()
    }
    fn src(pairs: &[(&str, &str)]) -> Source {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }
    fn saved(s: &Source) -> Value {
        serde_json::from_str(s.get(CUSTOM).unwrap()).unwrap()
    }
    fn window_background(s: &Source) -> Option<String> {
        picked(s)
    }
    fn custom_bg(s: &Source) -> String {
        saved(s)["bg"].as_str().unwrap().to_string()
    }
    fn dracula() -> String {
        opaque(
            &resolve_saved_theme("dracula", None, None).background,
            DEFAULT_GROUND,
        )
    }
    const DEFAULT_DARK: f64 = 185.0 / 255.0;
    /// What the window paints once the app has started: the slider's move runs next.
    fn painted(s: &mut Source) -> f64 {
        migrate_see_through(s);
        1.0 - f64::from(see_through_level(s.get(LEVEL).map(String::as_str))) / 100.0
    }
    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 0.005, "{a} vs {b}");
    }

    #[test]
    fn a_preset_at_60_with_acrylic_on_the_theme_ground_at_60_becomes_the_picked_background() {
        let mut s = src(&[(THEME, "dracula"), (ACRYLIC, "1"), (OPACITY, "60")]);
        migrate_opacity(&mut s);
        assert_eq!(
            window_background(&s),
            Some(format!("{}{}", dracula(), byte(60)))
        );
        assert!(!s.contains_key(OPACITY));
        close(painted(&mut s), f64::from(0x99) / 255.0);
    }

    #[test]
    fn a_picked_background_takes_the_live_alpha() {
        let mut s = src(&[(ACRYLIC, "1"), (OPACITY, "45"), (PICKED, "#102030")]);
        migrate_opacity(&mut s);
        assert_eq!(window_background(&s), Some(format!("#102030{}", byte(45))));
    }

    #[test]
    fn a_custom_whose_live_value_equals_its_saved_one_the_alpha_rides_on_custom_bg_alone() {
        let c = custom(serde_json::json!({ "acrylic": true, "opacity": 80 }));
        let mut s = src(&[
            (THEME, "custom"),
            (ACRYLIC, "1"),
            (OPACITY, "80"),
            (CUSTOM, &c),
        ]);
        migrate_opacity(&mut s);
        assert_eq!(custom_bg(&s), format!("#1d1f21{}", byte(80)));
        assert!(saved(&s).get("opacity").is_none());
        assert_eq!(window_background(&s), None);
        close(painted(&mut s), f64::from(204) / 255.0);
    }

    #[test]
    fn a_custom_whose_live_value_differs_the_saved_slot_keeps_its_own_the_live_one_is_picked() {
        let c = custom(serde_json::json!({ "acrylic": true, "opacity": 80 }));
        let mut s = src(&[
            (THEME, "custom"),
            (ACRYLIC, "1"),
            (OPACITY, "60"),
            (CUSTOM, &c),
        ]);
        migrate_opacity(&mut s);
        assert_eq!(custom_bg(&s), format!("#1d1f21{}", byte(80)));
        assert_eq!(window_background(&s), Some(format!("#1d1f21{}", byte(60))));
        close(painted(&mut s), f64::from(0x99) / 255.0);
    }

    #[test]
    fn a_custom_saved_at_60_left_at_a_live_100_with_acrylic_on_keeps_an_opaque_ground() {
        let c = custom(serde_json::json!({ "acrylic": true, "opacity": 60 }));
        let mut s = src(&[
            (THEME, "custom"),
            (ACRYLIC, "1"),
            (OPACITY, "100"),
            (CUSTOM, &c),
        ]);
        migrate_opacity(&mut s);
        close(painted(&mut s), DEFAULT_DARK);
        assert_eq!(window_background(&s), Some("#1d1f21".to_string()));
        let v = saved(&s);
        assert_eq!(v["bg"], "#1d1f21");
        assert_eq!(v["seeThrough"], 40);
        assert!(!s.contains_key(OPACITY));
    }

    #[test]
    fn a_custom_saved_at_60_with_no_live_value_stored_keeps_an_opaque_ground() {
        let c = custom(serde_json::json!({ "acrylic": true, "opacity": 60 }));
        let mut s = src(&[(THEME, "custom"), (ACRYLIC, "1"), (CUSTOM, &c)]);
        migrate_opacity(&mut s);
        close(painted(&mut s), DEFAULT_DARK);
        let mut t = src(&[(THEME, "custom"), (ACRYLIC, "1"), (CUSTOM, &c)]);
        migrate_opacity(&mut t);
        let once = t.clone();
        migrate_opacity(&mut t);
        assert_eq!(t, once);
    }

    #[test]
    fn a_custom_saved_at_60_under_a_preset_leaves_the_preset_alone() {
        let c = custom(serde_json::json!({ "acrylic": true, "opacity": 60 }));
        let mut s = src(&[(THEME, "dracula"), (ACRYLIC, "1"), (CUSTOM, &c)]);
        migrate_opacity(&mut s);
        assert_eq!(window_background(&s), None);
        close(painted(&mut s), DEFAULT_DARK);
    }

    #[test]
    fn a_custom_with_a_picked_background_the_picked_one_takes_the_live_value() {
        let c = custom(serde_json::json!({ "acrylic": true, "opacity": 70 }));
        let mut s = src(&[
            (THEME, "custom"),
            (ACRYLIC, "1"),
            (OPACITY, "70"),
            (PICKED, "#333333"),
            (CUSTOM, &c),
        ]);
        migrate_opacity(&mut s);
        assert_eq!(window_background(&s), Some(format!("#333333{}", byte(70))));
        assert_eq!(custom_bg(&s), format!("#1d1f21{}", byte(70)));
    }

    #[test]
    fn a_custom_saved_with_acrylic_off_is_not_folded_its_opacity_painted_nothing() {
        let c = custom(serde_json::json!({ "acrylic": false, "opacity": 50 }));
        let mut s = src(&[(THEME, "custom"), (CUSTOM, &c)]);
        migrate_opacity(&mut s);
        assert_eq!(custom_bg(&s), "#1d1f21");
        assert!(saved(&s).get("opacity").is_none());
    }

    #[test]
    fn a_custom_saved_without_the_acrylic_field_is_folded_it_never_said_off() {
        let c = custom(serde_json::json!({ "opacity": 50 }));
        let mut s = src(&[(CUSTOM, &c)]);
        migrate_opacity(&mut s);
        assert_eq!(custom_bg(&s), format!("#1d1f21{}", byte(50)));
    }

    #[test]
    fn a_live_value_with_acrylic_off_had_no_effect_and_is_only_removed() {
        let mut s = src(&[(THEME, "dracula"), (OPACITY, "40")]);
        migrate_opacity(&mut s);
        assert_eq!(window_background(&s), None);
        assert!(!s.contains_key(OPACITY));
    }

    #[test]
    fn a_live_100_is_only_removed() {
        let mut s = src(&[(ACRYLIC, "1"), (OPACITY, "100")]);
        migrate_opacity(&mut s);
        assert_eq!(window_background(&s), None);
        assert!(!s.contains_key(OPACITY));
    }

    #[test]
    fn is_idempotent_a_second_run_changes_nothing() {
        let c = custom(serde_json::json!({ "acrylic": true, "opacity": 80 }));
        let mut s = src(&[
            (THEME, "custom"),
            (ACRYLIC, "1"),
            (OPACITY, "60"),
            (CUSTOM, &c),
        ]);
        migrate_opacity(&mut s);
        let once = s.clone();
        migrate_opacity(&mut s);
        assert_eq!(s, once);
    }

    #[test]
    fn a_fresh_install_has_nothing_to_do() {
        let mut s = Source::new();
        migrate_opacity(&mut s);
        assert!(s.is_empty());
    }

    #[test]
    fn junk_reads_as_the_window_read_it_and_never_throws() {
        let mut s = src(&[(ACRYLIC, "1"), (THEME, "dracula"), (OPACITY, "soup")]);
        migrate_opacity(&mut s);
        assert_eq!(window_background(&s), None);
        assert!(!s.contains_key(OPACITY));
        // Below the slider's floor reads as 30, as the window clamped it.
        s.insert(OPACITY.to_string(), "5".to_string());
        migrate_opacity(&mut s);
        assert_eq!(
            window_background(&s),
            Some(format!("{}{}", dracula(), byte(30)))
        );
        // A corrupt Custom is left for its own reader, which reads it as none.
        s.insert(CUSTOM.to_string(), "{nope".to_string());
        migrate_opacity(&mut s);
        assert_eq!(s.get(CUSTOM).map(String::as_str), Some("{nope"));
        let bad = custom(serde_json::json!({ "bg": "soup", "opacity": 50 }));
        s.insert(CUSTOM.to_string(), bad);
        migrate_opacity(&mut s);
    }

    #[test]
    fn opacity_pct_reads_none_empty_and_nan_as_100() {
        assert_eq!(opacity_pct(None), 100);
        assert_eq!(opacity_pct(Some("")), 100);
        assert_eq!(opacity_pct(Some("soup")), 100);
        assert_eq!(opacity_pct(Some("5")), 30);
        assert_eq!(opacity_pct(Some("59.5")), 60);
        assert_eq!(opacity_pct(Some("500")), 100);
    }

    #[test]
    fn a_fresh_install_or_the_switch_off_is_0_the_window_solid() {
        let mut s = Source::new();
        migrate_see_through(&mut s);
        assert_eq!(s.get(LEVEL).map(String::as_str), Some("0"));
        assert!(!s.contains_key(ACRYLIC));
        let mut t = src(&[(ACRYLIC, "0"), (PICKED, "#20202099")]);
        migrate_see_through(&mut t);
        assert_eq!(t.get(LEVEL).map(String::as_str), Some("0"));
        assert_eq!(window_background(&t), Some("#202020".to_string()));
        assert!(!t.contains_key(ACRYLIC));
    }

    #[test]
    fn on_over_a_dark_preset_the_default_it_painted_b9_as_a_level() {
        let mut s = src(&[(THEME, "dracula"), (ACRYLIC, "1")]);
        migrate_see_through(&mut s);
        assert_eq!(s.get(LEVEL).map(String::as_str), Some("27"));
        close(painted(&mut s), f64::from(0xb9) / 255.0);
    }

    #[test]
    fn on_over_a_light_preset_the_light_default_d1() {
        let mut s = src(&[(THEME, "paper"), (ACRYLIC, "1")]);
        migrate_see_through(&mut s);
        close(painted(&mut s), f64::from(0xd1) / 255.0);
    }

    #[test]
    fn on_with_a_see_through_picked_background_its_alpha_and_the_pick_goes_opaque() {
        let mut s = src(&[(THEME, "dracula"), (ACRYLIC, "1"), (PICKED, "#20202099")]);
        migrate_see_through(&mut s);
        assert_eq!(s.get(LEVEL).map(String::as_str), Some("40"));
        assert_eq!(window_background(&s), Some("#202020".to_string()));
    }

    #[test]
    fn a_saved_custom_keeps_the_glass_it_was_saved_with_on_an_opaque_ground() {
        let c = custom(serde_json::json!({ "bg": "#1d1f2180", "acrylic": true }));
        let mut s = src(&[(THEME, "dracula"), (CUSTOM, &c)]);
        migrate_see_through(&mut s);
        let v = saved(&s);
        assert_eq!(v["bg"], "#1d1f21");
        assert_eq!(v["seeThrough"], 50);
        assert!(v.get("acrylic").is_none());
        assert_eq!(s.get(LEVEL).map(String::as_str), Some("0"));
    }

    #[test]
    fn a_custom_saved_with_the_switch_off_or_never_said_keeps_none() {
        let c = custom(serde_json::json!({ "bg": "#1d1f2180" }));
        let mut s = src(&[(CUSTOM, &c)]);
        migrate_see_through(&mut s);
        let v = saved(&s);
        assert_eq!(v["bg"], "#1d1f21");
        assert_eq!(v["seeThrough"], 0);
    }

    #[test]
    fn the_background_a_custom_keeps_is_what_it_painted_and_it_goes_opaque_too() {
        let c = custom(serde_json::json!({ "acrylic": true }));
        let mut s = src(&[
            (CUSTOM, &c),
            (
                KEPT,
                r##"{"background":"#ffffff99","accent":null,"secondary":null}"##,
            ),
        ]);
        migrate_see_through(&mut s);
        assert_eq!(saved(&s)["seeThrough"], 40);
        let kept: Value = serde_json::from_str(s.get(KEPT).unwrap()).unwrap();
        assert_eq!(kept["background"], "#ffffff");
    }

    #[test]
    fn runs_once_a_stored_level_is_left_alone() {
        let mut s = src(&[(LEVEL, "12"), (ACRYLIC, "1")]);
        migrate_see_through(&mut s);
        assert_eq!(s.get(LEVEL).map(String::as_str), Some("12"));
        assert_eq!(s.get(ACRYLIC).map(String::as_str), Some("1"));
    }

    #[test]
    fn a_corrupt_custom_is_left_for_its_own_reader() {
        let mut s = src(&[(CUSTOM, "{nope")]);
        migrate_see_through(&mut s);
        assert_eq!(s.get(CUSTOM).map(String::as_str), Some("{nope"));
    }

    #[test]
    fn migrate_runs_opacity_then_see_through_and_leaves_no_legacy_key() {
        let mut s = src(&[(THEME, "dracula"), (ACRYLIC, "1"), (OPACITY, "60")]);
        migrate(&mut s);
        assert!(!s.contains_key(OPACITY) && !s.contains_key(ACRYLIC));
        assert!(s.contains_key(LEVEL));
    }

    #[test]
    fn the_default_see_through_is_prisms_two_levels_as_they_paint() {
        assert_eq!(js_round(SEE_THROUGH_ALPHA_DARK * 255.0) as u32, 0xb9);
        assert_eq!(js_round(SEE_THROUGH_ALPHA_LIGHT * 255.0) as u32, 0xd1);
        assert_eq!(see_through_alpha("#0b0b0f"), SEE_THROUGH_ALPHA_DARK);
        assert_eq!(see_through_alpha("#ffffff"), SEE_THROUGH_ALPHA_LIGHT);
    }

    #[test]
    fn light_is_measured_relative_luminance_above_point_four() {
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
        assert_eq!(js_round(SEE_THROUGH_MAX * 255.0) as u32, 0xf2);
        const { assert!(SEE_THROUGH_MAX > SEE_THROUGH_ALPHA_LIGHT) };
        const { assert!(SEE_THROUGH_MAX < 1.0) };
    }
}
