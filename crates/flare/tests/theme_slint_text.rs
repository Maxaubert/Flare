//! Task 22: the theme tokens are settable (`in` properties), each holding its Prism Terminal pt-default
//! value. These read the .slint text and the flare-theme fixtures, so they need no Slint build.

use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

/// One `in` / `out` / `in-out` property declared in theme.slint.
#[derive(Debug)]
struct Decl {
    qualifier: String,
    ty: String,
    name: String,
    value: String,
}

fn theme_text() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("ui")
        .join("theme.slint");
    fs::read_to_string(path).expect("read theme.slint")
}

fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("flare-theme")
        .join("tests")
        .join("fixtures")
        .join(name);
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("fixture {name}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("fixture {name}: {e}"))
}

/// Every property line in theme.slint, e.g. `in property <color> ground: #171717;`.
fn declarations(theme: &str) -> Vec<Decl> {
    theme
        .lines()
        .filter_map(|line| {
            let t = line.trim();
            let (qualifier, rest) = if let Some(r) = t.strip_prefix("in property ") {
                ("in", r)
            } else if let Some(r) = t.strip_prefix("out property ") {
                ("out", r)
            } else {
                ("in-out", t.strip_prefix("in-out property ")?)
            };
            let rest = rest.strip_prefix('<')?;
            let (ty, rest) = rest.split_once('>')?;
            let (name, value) = rest.split_once(':')?;
            let value = value.split(';').next()?.trim();
            Some(Decl {
                qualifier: qualifier.to_string(),
                ty: ty.trim().to_string(),
                name: name.trim().to_string(),
                value: value.to_string(),
            })
        })
        .collect()
}

fn decl<'a>(decls: &'a [Decl], name: &str) -> Option<&'a Decl> {
    decls.iter().find(|d| d.name == name)
}

/// The Slint property for a Prism Terminal chrome token: strip `--p-`, except `--p-bg` (`ground`)
/// and `--p-bg-solid` (`ground-solid`).
fn slint_name(token: &str) -> String {
    match token {
        "--p-bg" => "ground".to_string(),
        "--p-bg-solid" => "ground-solid".to_string(),
        other => other.trim_start_matches("--p-").to_string(),
    }
}

/// The 31 chrome token names, in Prism Terminal's order.
fn chrome_tokens() -> Vec<String> {
    fixture("chrome_token_names.json")
        .as_array()
        .expect("chrome_token_names.json is an array")
        .iter()
        .map(|v| v.as_str().expect("token name is a string").to_string())
        .collect()
}

#[test]
fn every_chrome_token_is_an_in_property_with_its_pt_default() {
    let decls = declarations(&theme_text());
    let tokens = chrome_tokens();
    assert_eq!(tokens.len(), 31, "PT has 31 chrome colour tokens");
    let pt = fixture("chrome_presets.json");
    let defaults = &pt["pt-default"]["theme"];
    for token in &tokens {
        let name = slint_name(token);
        let d = decl(&decls, &name)
            .unwrap_or_else(|| panic!("theme.slint lacks `property <color> {name}`"));
        assert_eq!(d.qualifier, "in", "Theme.{name} must be settable (in)");
        assert_eq!(d.ty, "color", "Theme.{name} is a color");
        let want = defaults[token]
            .as_str()
            .unwrap_or_else(|| panic!("pt-default has no {token}"));
        assert_eq!(
            d.value.to_ascii_lowercase(),
            want.to_ascii_lowercase(),
            "Theme.{name} must hold PT's pt-default {token}"
        );
    }
}

#[test]
fn the_token_names_are_the_chrome_tokens_minus_the_prefix() {
    let decls = declarations(&theme_text());
    let mapped: BTreeSet<String> = chrome_tokens().iter().map(|t| slint_name(t)).collect();
    assert_eq!(mapped.len(), 31, "the 31 Slint names are distinct");
    let in_colours: BTreeSet<String> = decls
        .iter()
        .filter(|d| d.qualifier == "in" && d.ty == "color")
        .map(|d| d.name.clone())
        .collect();
    let mut expected = mapped;
    expected.insert("window-clear".to_string());
    // Task 25: the window edge, drawn by Slint because the DWM border does not show (docs/material.md E4).
    expected.insert("edge".to_string());
    // Flare's own inks beside PT's 31 (flare_theme::chrome::FLARE_EXTRA_TOKENS).
    for name in [
        "secondary-text",
        "secondary-icon",
        "accent-ink",
        "hover-ink",
        "pressed-text",
        "track-off",
    ] {
        expected.insert(name.to_string());
    }
    assert_eq!(
        in_colours, expected,
        "the settable colours are the 31 chrome tokens plus window-clear and edge"
    );
}

#[test]
fn window_clear_and_light_exist() {
    let decls = declarations(&theme_text());
    let clear = decl(&decls, "window-clear").expect("in property <color> window-clear");
    assert_eq!(clear.qualifier, "in");
    assert_eq!(clear.ty, "color");
    assert_eq!(clear.value.to_ascii_lowercase(), "#171717");
    let light = decl(&decls, "light").expect("in property <bool> light");
    assert_eq!(light.qualifier, "in");
    assert_eq!(light.ty, "bool");
    assert_eq!(light.value, "false");
}

#[test]
fn lengths_and_fixed_colours_stay_out() {
    let decls = declarations(&theme_text());
    for name in [
        "bar-height",
        "radius-button",
        "term-padding",
        "close-hover",
        "danger-fill",
        "scrim",
        "font-ui",
    ] {
        let d = decl(&decls, name).unwrap_or_else(|| panic!("Theme.{name} is declared"));
        assert_eq!(d.qualifier, "out", "Theme.{name} stays out");
    }
}

#[test]
fn there_is_no_segment_property() {
    let decls = declarations(&theme_text());
    let seg: Vec<&str> = decls
        .iter()
        .map(|d| d.name.as_str())
        .filter(|n| n.starts_with("seg"))
        .collect();
    assert!(seg.is_empty(), "no segment property: {seg:?}");
}
