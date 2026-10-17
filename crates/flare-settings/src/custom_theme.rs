//! The Custom theme slot (Task 16).
//! Source: PT `core/renderer/lib/termLook.ts` (`customTermTheme`, `CustomTermTheme`, `withGroundAlpha`) and
//! `core/renderer/settings/theme/palette.ts` (`paletteOf`).
//!
//! There is one Custom slot, edited and saved over. A save carries the palette and the extras. The
//! font, its size, the opacity and the acrylic switch of older saves are read no more and never
//! written.

use flare_theme::ansi::{ANSI_KEYS, PartialAnsi, normalize_color};
use flare_theme::colour::{canonical_stored, opaque, parse_colour, to_stored};
use flare_theme::ground::DEFAULT_GROUND;
use flare_theme::resolve::{CustomPalette, preset_accent, resolve_saved_theme};
use flare_theme::see_through::see_through_level;
use serde_json::{Map, Value};

/// The store key of the one Custom theme.
pub const CUSTOM_KEY: &str = "flare.term.custom";

/// A saved Custom: the palette and the rest of the setup the save captured.
#[derive(Clone, Debug, PartialEq)]
pub struct CustomTheme {
    pub palette: CustomPalette,
    pub indicator: Option<String>,
    pub indicator_color: Option<String>,
    pub done_color: Option<String>,
    pub question_color: Option<String>,
    pub see_through: Option<u8>,
    // read no more, never written: acrylic, opacity, font, fontPct (older saves may carry them)
}

/// A colour in its stored form, or None for anything that is not one.
fn stored(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str)
        .and_then(parse_colour)
        .map(|c| to_stored(&c))
}

/// An agent colour in its canonical stored form (six or eight digits), or None.
fn agent_colour(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str)
        .and_then(|s| canonical_stored(Some(s)))
}

/// A saved Custom read back. Corrupt JSON, or a background or foreground that is not a colour, is no
/// Custom. A bad cursor is the text colour, a bad selection or accent is absent, and each of the
/// sixteen goes through the one parser (a bad one is dropped).
pub fn parse_custom(raw: &str) -> Option<CustomTheme> {
    let v: Value = serde_json::from_str(raw).ok()?;
    let bg = stored(v.get("bg"))?;
    let fg = stored(v.get("fg"))?;
    let cursor = stored(v.get("cursor")).unwrap_or_else(|| fg.clone());
    let mut ansi: PartialAnsi = Default::default();
    if let Some(obj) = v.get("ansi").and_then(Value::as_object) {
        for (i, key) in ANSI_KEYS.iter().enumerate() {
            ansi[i] = stored(obj.get(*key));
        }
    }
    let level = v
        .get("seeThrough")
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite())
        .map(|n| see_through_level(Some(&n.to_string())));
    Some(CustomTheme {
        palette: CustomPalette {
            bg,
            fg,
            cursor,
            selection: stored(v.get("selection")),
            ansi,
            accent: stored(v.get("accent")),
        },
        indicator: v
            .get("indicator")
            .and_then(Value::as_str)
            .map(str::to_string),
        indicator_color: agent_colour(v.get("indicatorColor")),
        done_color: agent_colour(v.get("doneColor")),
        question_color: agent_colour(v.get("questionColor")),
        see_through: level,
    })
}

/// The saved form, with Prism Terminal's field names.
pub fn serialize_custom(c: &CustomTheme) -> String {
    let p = &c.palette;
    let mut m = Map::new();
    m.insert("bg".into(), p.bg.clone().into());
    m.insert("fg".into(), p.fg.clone().into());
    m.insert("cursor".into(), p.cursor.clone().into());
    if let Some(s) = &p.selection {
        m.insert("selection".into(), s.clone().into());
    }
    let mut ansi = Map::new();
    for (i, key) in ANSI_KEYS.iter().enumerate() {
        if let Some(col) = &p.ansi[i] {
            ansi.insert((*key).into(), col.clone().into());
        }
    }
    m.insert("ansi".into(), Value::Object(ansi));
    let opt = |m: &mut Map<String, Value>, k: &str, v: &Option<String>| {
        if let Some(v) = v {
            m.insert(k.into(), v.clone().into());
        }
    };
    opt(&mut m, "accent", &p.accent);
    opt(&mut m, "indicator", &c.indicator);
    opt(&mut m, "indicatorColor", &c.indicator_color);
    opt(&mut m, "doneColor", &c.done_color);
    opt(&mut m, "questionColor", &c.question_color);
    if let Some(l) = c.see_through {
        m.insert("seeThrough".into(), l.into());
    }
    Value::Object(m).to_string()
}

/// The palette the editor opens and the Custom slot holds for a theme (PT `paletteOf`). A saved Custom
/// is handed over raw, its alphas and chosen selection kept. A preset is resolved and normalised, its
/// accent attached.
pub fn palette_of(id: &str, custom: Option<&CustomTheme>, in_force: &str) -> CustomPalette {
    let saved = custom.map(|c| &c.palette);
    if id == "custom"
        && let Some(p) = saved
    {
        return p.clone();
    }
    let t = resolve_saved_theme(id, saved, None);
    CustomPalette {
        bg: normalize_color(&t.background, DEFAULT_GROUND),
        fg: normalize_color(&t.foreground, "#e7e7ee"),
        cursor: normalize_color(&t.cursor, "#5b5bd6"),
        selection: None,
        ansi: t.ansi.clone(),
        accent: preset_accent(id, in_force, saved, None),
    }
}

/// The ground opaque: the see-through is the slider's, never the colour's.
pub fn with_ground_alpha(p: &CustomPalette) -> CustomPalette {
    CustomPalette {
        bg: opaque(&p.bg, DEFAULT_GROUND),
        ..p.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pal(bg: &str) -> CustomPalette {
        CustomPalette {
            bg: bg.into(),
            fg: "#eeeeee".into(),
            cursor: "#ffffff".into(),
            selection: None,
            ansi: Default::default(),
            accent: None,
        }
    }

    fn theme(p: CustomPalette) -> CustomTheme {
        CustomTheme {
            palette: p,
            indicator: None,
            indicator_color: None,
            done_color: None,
            question_color: None,
            see_through: None,
        }
    }

    #[test]
    fn a_corrupt_save_reads_as_no_custom_theme() {
        assert_eq!(parse_custom("{nope"), None);
        assert_eq!(parse_custom(""), None);
    }

    #[test]
    fn a_bad_background_or_foreground_is_no_custom_theme() {
        assert_eq!(
            parse_custom(r##"{"bg":"soup","fg":"#eeeeee","cursor":"#fff","ansi":{}}"##),
            None
        );
        assert_eq!(
            parse_custom(r##"{"bg":"#111111","fg":"soup","cursor":"#fff","ansi":{}}"##),
            None
        );
    }

    #[test]
    fn a_bad_cursor_or_selection_reads_as_absent() {
        let c = parse_custom(
            r##"{"bg":"#111111","fg":"#EEEEEE","cursor":"soup","selection":"soup","ansi":{}}"##,
        )
        .unwrap();
        assert_eq!(c.palette.fg, "#eeeeee");
        assert_eq!(c.palette.cursor, "#eeeeee");
        assert_eq!(c.palette.selection, None);
        let c = parse_custom(
            r##"{"bg":"#111111","fg":"#EEEEEE","cursor":"#ff000080","selection":"#33445566","ansi":{}}"##,
        )
        .unwrap();
        assert_eq!(c.palette.cursor, "#ff000080");
        assert_eq!(c.palette.selection.as_deref(), Some("#33445566"));
    }

    #[test]
    fn the_custom_theme_drops_a_bad_ansi_value_and_keeps_the_rest() {
        let c = parse_custom(
            r##"{"bg":"#111111","fg":"#eeeeee","cursor":"#fff","ansi":{"red":"#ff5555","blue":"#0000ff40","green":"soup"}}"##,
        )
        .unwrap();
        assert_eq!(c.palette.ansi[1].as_deref(), Some("#ff5555"));
        assert_eq!(c.palette.ansi[4].as_deref(), Some("#0000ff40"));
        assert_eq!(c.palette.ansi[2], None);
        assert_eq!(c.palette.cursor, "#ffffff");
    }

    #[test]
    fn a_bad_accent_reads_as_absent_and_a_good_one_is_kept() {
        let c = parse_custom(
            r##"{"bg":"#111111","fg":"#eeeeee","cursor":"#fff","ansi":{},"accent":"nope"}"##,
        )
        .unwrap();
        assert_eq!(c.palette.accent, None);
        let c = parse_custom(
            r##"{"bg":"#111111","fg":"#eeeeee","cursor":"#fff","ansi":{},"accent":"#FE8F34"}"##,
        )
        .unwrap();
        assert_eq!(c.palette.accent.as_deref(), Some("#fe8f34"));
    }

    #[test]
    fn serialize_uses_prism_terminals_field_names_and_round_trips() {
        let mut p = pal("#111111");
        p.selection = Some("#33445566".into());
        p.accent = Some("#fe8f34".into());
        p.ansi[0] = Some("#000000".into());
        p.ansi[15] = Some("#ffffff".into());
        let c = CustomTheme {
            palette: p,
            indicator: Some("minimal".into()),
            indicator_color: Some("#22c55e".into()),
            done_color: None,
            question_color: Some("#11223380".into()),
            see_through: Some(35),
        };
        let text = serialize_custom(&c);
        let v: Value = serde_json::from_str(&text).unwrap();
        for k in [
            "bg",
            "fg",
            "cursor",
            "selection",
            "ansi",
            "accent",
            "indicator",
            "indicatorColor",
            "questionColor",
            "seeThrough",
        ] {
            assert!(v.get(k).is_some(), "missing {k}");
        }
        assert!(v.get("doneColor").is_none());
        assert_eq!(v["ansi"]["brightWhite"], "#ffffff");
        assert_eq!(v["seeThrough"], 35);
        assert_eq!(parse_custom(&text), Some(c));
    }

    #[test]
    fn older_keys_are_read_no_more() {
        let c = parse_custom(
            r##"{"bg":"#111111","fg":"#eeeeee","cursor":"#fff","ansi":{},"font":"cascadia","fontPct":90,"opacity":60,"acrylic":true}"##,
        )
        .unwrap();
        let text = serialize_custom(&c);
        for k in ["font", "fontPct", "opacity", "acrylic"] {
            assert!(!text.contains(&format!("\"{k}\"")), "{k} written");
        }
    }

    #[test]
    fn with_ground_alpha_drops_the_background_alpha_only() {
        let mut p = pal("#20202080");
        p.cursor = "#ff000080".into();
        let q = with_ground_alpha(&p);
        assert_eq!(q.bg, "#202020");
        assert_eq!(q.cursor, "#ff000080");
    }

    #[test]
    fn palette_of_a_preset_attaches_its_accent_and_normalises() {
        let p = palette_of("pt-default", None, "pt-default");
        assert_eq!(p.accent.as_deref(), Some("#fe8f34"));
        assert_eq!(p.bg.len(), 7);
        assert!(p.ansi.iter().all(Option::is_some));
    }

    #[test]
    fn palette_of_a_saved_custom_is_the_raw_save() {
        let mut p = pal("#11111180");
        p.selection = Some("#33445566".into());
        let c = theme(p.clone());
        assert_eq!(palette_of("custom", Some(&c), "custom"), p);
    }
}
