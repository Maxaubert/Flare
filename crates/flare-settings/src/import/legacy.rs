//! The legacy transparency of the first-run import (M7 Task 15; changed 2026-11-28).
//!
//! Prism Terminal's retired Opacity slider and acrylic switch once folded into Flare's see-through
//! level (`opacityMigration.ts`, `seeThroughMigration.ts`). Flare's see-through window was removed on
//! 2026-11-28 (Liquid Glass on the Mac comes later, as its own feature), so the import maps
//! transparency to nothing and every window imports solid:
//!
//! - the two legacy keys (`prism.term.acrylic`, `prism.term.opacity`) are dropped here; the see-through
//!   level (`prism.term.seeThrough`) is dropped by the key table (`map.rs`, `DROPPED_KEYS`);
//! - a saved Custom loses its `acrylic`, `opacity` and `seeThrough` fields;
//! - the grounds the glass made see-through (the Custom's own `bg`, the picked Background and the
//!   Background a Custom keeps) are written opaque, their colour kept.
//!
//! This file never writes a legacy key. The map is the source's text as Local Storage held it, keyed
//! by the old names.

use std::collections::BTreeMap;

use flare_theme::colour::{alpha_of, canonical_stored, opaque, parse_colour};
use flare_theme::ground::DEFAULT_GROUND;
use serde_json::{Map, Value};

/// Prism Terminal's keys to their text, exactly as Local Storage held it.
pub type Source = BTreeMap<String, String>;

const OPACITY: &str = "prism.term.opacity";
const ACRYLIC: &str = "prism.term.acrylic";
const CUSTOM: &str = "prism.term.custom";
const PICKED: &str = "prism.window.background";
const KEPT: &str = "prism.window.customColours";

/// The fields of a saved Custom that only ever said how see-through the window was.
const CUSTOM_GLASS_FIELDS: [&str; 3] = ["acrylic", "opacity", "seeThrough"];

/// The picked Background as the window read it: the stored form of a 6 or 8 digit hex, or None.
fn picked(s: &Source) -> Option<String> {
    canonical_stored(s.get(PICKED).map(String::as_str))
}

/// The saved Custom as a JSON object, or None when absent, corrupt or not an object.
fn custom_object(s: &Source) -> Option<Map<String, Value>> {
    let raw = s.get(CUSTOM).filter(|r| !r.is_empty())?;
    match serde_json::from_str::<Value>(raw).ok()? {
        Value::Object(m) => Some(m),
        _ => None,
    }
}

/// The saved Custom without its glass fields and with an opaque ground. A corrupt one is left for its
/// own reader, which reads it as none.
fn solid_custom(s: &mut Source) {
    let Some(mut c) = custom_object(s) else {
        return;
    };
    let mut changed = false;
    for k in CUSTOM_GLASS_FIELDS {
        changed |= c.remove(k).is_some();
    }
    let bg = c
        .get("bg")
        .and_then(Value::as_str)
        .filter(|b| parse_colour(b).is_some() && alpha_of(b) < 1.0)
        .map(str::to_string);
    if let Some(bg) = bg {
        c.insert("bg".to_string(), Value::String(opaque(&bg, DEFAULT_GROUND)));
        changed = true;
    }
    if changed {
        s.insert(CUSTOM.to_string(), Value::Object(c).to_string());
    }
}

/// The picked Background, opaque.
fn solid_picked(s: &mut Source) {
    if let Some(p) = picked(s)
        && alpha_of(&p) < 1.0
    {
        s.insert(PICKED.to_string(), opaque(&p, DEFAULT_GROUND));
    }
}

/// The Background a Custom keeps, opaque (read as the reader does: a 6 or 8 digit hex each, else none).
fn solid_kept(s: &mut Source) {
    let Some(mut v) = s
        .get(KEPT)
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
    else {
        return;
    };
    let Some(bg) = v
        .get("background")
        .and_then(Value::as_str)
        .and_then(|t| canonical_stored(Some(t)))
        .filter(|b| alpha_of(b) < 1.0)
    else {
        return;
    };
    v["background"] = Value::String(opaque(&bg, DEFAULT_GROUND));
    s.insert(KEPT.to_string(), v.to_string());
}

/// Maps Prism Terminal's transparency to nothing: the legacy keys go, the Custom loses its glass
/// fields and every ground the glass touched is opaque. Idempotent.
pub fn migrate(s: &mut Source) {
    s.remove(OPACITY);
    s.remove(ACRYLIC);
    solid_custom(s);
    solid_picked(s);
    solid_kept(s);
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEVEL: &str = "prism.term.seeThrough";
    const THEME: &str = "prism.term.theme";

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

    #[test]
    fn a_fresh_install_has_nothing_to_do() {
        let mut s = Source::new();
        migrate(&mut s);
        assert!(s.is_empty());
    }

    #[test]
    fn the_legacy_keys_map_to_nothing_and_no_background_is_made_from_them() {
        let mut s = src(&[(THEME, "dracula"), (ACRYLIC, "1"), (OPACITY, "60")]);
        migrate(&mut s);
        assert_eq!(s, src(&[(THEME, "dracula")]));
    }

    #[test]
    fn the_see_through_level_is_left_for_the_key_table_to_drop() {
        let mut s = src(&[(LEVEL, "40"), (ACRYLIC, "1")]);
        migrate(&mut s);
        assert_eq!(s, src(&[(LEVEL, "40")]));
    }

    #[test]
    fn a_saved_custom_loses_its_glass_fields_and_its_ground_goes_opaque() {
        let c = custom(serde_json::json!({
            "bg": "#1d1f2180", "acrylic": true, "opacity": 60, "seeThrough": 40
        }));
        let mut s = src(&[(THEME, "custom"), (CUSTOM, &c)]);
        migrate(&mut s);
        let v = saved(&s);
        assert_eq!(v["bg"], "#1d1f21");
        for k in CUSTOM_GLASS_FIELDS {
            assert!(v.get(k).is_none(), "{k}");
        }
        assert_eq!(v["ansi"]["red"], "#cc6666");
    }

    #[test]
    fn a_custom_with_nothing_see_through_is_left_as_it_was() {
        let c = custom(serde_json::json!({}));
        let mut s = src(&[(CUSTOM, &c)]);
        migrate(&mut s);
        assert_eq!(s.get(CUSTOM), Some(&c));
    }

    #[test]
    fn a_see_through_picked_background_keeps_its_colour_opaque() {
        let mut s = src(&[(PICKED, "#20202099")]);
        migrate(&mut s);
        assert_eq!(picked(&s), Some("#202020".to_string()));
        let mut t = src(&[(PICKED, "#102030")]);
        migrate(&mut t);
        assert_eq!(picked(&t), Some("#102030".to_string()));
    }

    #[test]
    fn the_background_a_custom_keeps_goes_opaque_too() {
        let mut s = src(&[(
            KEPT,
            r##"{"background":"#ffffff99","accent":"#ff880080","secondary":null}"##,
        )]);
        migrate(&mut s);
        let kept: Value = serde_json::from_str(s.get(KEPT).unwrap()).unwrap();
        assert_eq!(kept["background"], "#ffffff");
        assert_eq!(
            kept["accent"], "#ff880080",
            "an accent's alpha is a colour, kept"
        );
    }

    #[test]
    fn a_corrupt_custom_is_left_for_its_own_reader() {
        let mut s = src(&[(CUSTOM, "{nope")]);
        migrate(&mut s);
        assert_eq!(s.get(CUSTOM).map(String::as_str), Some("{nope"));
    }

    #[test]
    fn is_idempotent_a_second_run_changes_nothing() {
        let c = custom(serde_json::json!({ "bg": "#1d1f2180", "acrylic": true, "opacity": 80 }));
        let mut s = src(&[
            (THEME, "custom"),
            (ACRYLIC, "1"),
            (OPACITY, "60"),
            (PICKED, "#33333380"),
            (CUSTOM, &c),
        ]);
        migrate(&mut s);
        let once = s.clone();
        migrate(&mut s);
        assert_eq!(s, once);
    }
}
