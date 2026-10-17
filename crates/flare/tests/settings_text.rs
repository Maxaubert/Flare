//! Source checks of the Settings tab: copy, neutral colour and the closed tables (M7 Tasks 4, 44 and 46).
//! Task 4 holds the three settings token globals in `theme.slint` to Prism Terminal's numbers.

use std::fs;
use std::path::PathBuf;

fn ui_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui")
}

fn theme() -> String {
    fs::read_to_string(ui_dir().join("theme.slint")).expect("read theme.slint")
}

/// The text between the braces of `export global NAME { ... }`.
fn global_body(text: &str, name: &str) -> String {
    let head = format!("export global {name} {{");
    let at = text
        .find(&head)
        .unwrap_or_else(|| panic!("theme.slint declares global {name}"));
    let tail = &text[at + head.len()..];
    let mut depth = 1usize;
    for (i, c) in tail.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return tail[..i].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("global {name} has no closing brace");
}

/// The value after `> NAME:` on its property line, trimmed, with the trailing `;` removed.
fn property_value(body: &str, name: &str) -> Option<String> {
    let needle = format!("> {name}:");
    let line = body.lines().find(|l| l.contains(&needle))?;
    let after = &line[line.find(&needle)? + needle.len()..];
    Some(after.split(';').next()?.trim().to_string())
}

#[test]
fn settings_globals_exist() {
    let text = theme();
    for name in ["SettingsShape", "SettingsInk", "SettingsFixed"] {
        assert!(
            text.contains(&format!("export global {name} {{")),
            "theme.slint exports global {name}"
        );
    }
}

#[test]
fn shape_numbers_are_prism_terminals() {
    let text = theme();
    let body = global_body(&text, "SettingsShape");
    let rows: [(&str, &str); 28] = [
        ("radius", "2px"),
        ("radius-sm", "3px"),
        ("panel-radius", "5px"),
        ("tile-radius", "4px"),
        ("rail-w", "244px"),
        ("rail-narrow-w", "56px"),
        ("narrow-below", "760px"),
        ("row-min-h", "58px"),
        ("tile", "32px"),
        ("column-max", "960px"),
        ("switch-w", "36px"),
        ("switch-h", "20px"),
        ("switch-knob", "16px"),
        ("control-h", "32px"),
        ("select-min-w", "168px"),
        ("option-h", "28px"),
        ("slider-w", "168px"),
        ("slider-track-h", "6px"),
        ("slider-thumb", "18px"),
        ("popover-w", "226px"),
        ("sv-w", "200px"),
        ("sv-h", "140px"),
        ("bar-h", "12px"),
        ("bar-thumb", "16px"),
        ("card-w", "196px"),
        ("wall-gap", "12px"),
        ("help-min-w", "420px"),
        ("help-max-w", "1100px"),
    ];
    for (name, value) in rows {
        assert_eq!(
            property_value(&body, name).as_deref(),
            Some(value),
            "SettingsShape.{name} must be {value}"
        );
        assert!(
            body.contains(&format!("out property <length> {name}:")),
            "SettingsShape.{name} is an out length"
        );
    }
}

#[test]
fn ink_uses_mix_with_the_right_factor() {
    let text = theme();
    let body = global_body(&text, "SettingsInk");
    let names = [
        "panel-fill",
        "tile-fill",
        "row-hover",
        "sub",
        "warn",
        "focus-edge",
        "flash",
        "help-stripe",
        "help-hover",
        "help-active",
        "help-well",
        "help-field",
        "help-focus-glow",
        "selection",
    ];
    for name in names {
        assert!(
            property_value(&body, name).is_some(),
            "SettingsInk.{name} is declared"
        );
    }
    let expect = [
        ("panel-fill", "Theme.text.with-alpha(0.036)"),
        ("tile-fill", "Theme.text.with-alpha(0.06)"),
        ("sub", "Theme.dim.mix(Theme.text, 0.72)"),
        ("flash", "Theme.accent-hi.with-alpha(0.16)"),
        (
            "warn",
            "Theme.light ? SettingsFixed.warn-light.mix(Theme.text, 0.75) : SettingsFixed.warn-dark.mix(Theme.text, 0.85)",
        ),
    ];
    for (name, value) in expect {
        assert_eq!(
            property_value(&body, name).as_deref(),
            Some(value),
            "SettingsInk.{name}"
        );
    }
}

#[test]
fn the_hex_rule_still_holds() {
    // theme.slint is the one token file: the settings globals hold their hex here and nowhere else.
    let text = theme();
    let fixed = global_body(&text, "SettingsFixed");
    assert!(
        fixed.contains("#e0a84b"),
        "SettingsFixed holds the warn hex"
    );
    let settings_dir = ui_dir().join("settings");
    for entry in fs::read_dir(&settings_dir).expect("read ui/settings") {
        let path = entry.expect("dir entry").path();
        if path.extension().is_some_and(|e| e == "slint") {
            let body = fs::read_to_string(&path).expect("read slint file");
            let hex = body
                .char_indices()
                .filter(|(_, c)| *c == '#')
                .filter(|(i, _)| {
                    body[i + 1..]
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_hexdigit())
                })
                .count();
            assert_eq!(hex, 0, "{} has no hex literal", path.display());
        }
    }
}

#[test]
fn no_text_property_uses_a_dash_family_name() {
    // The font families are quoted string literals in SettingsFixed. A bare hyphenated identifier
    // (a dash family name written as an expression) is refused.
    let text = theme();
    let fixed = global_body(&text, "SettingsFixed");
    for name in ["font-text", "font-display", "font-fallback", "font-mono"] {
        let value = property_value(&fixed, name)
            .unwrap_or_else(|| panic!("SettingsFixed.{name} is declared"));
        assert!(
            value.starts_with('"') && value.ends_with('"') && value.len() > 2,
            "SettingsFixed.{name} is a quoted string: {value}"
        );
        assert!(
            fixed.contains(&format!("property <string> {name}:")),
            "SettingsFixed.{name} is a string property"
        );
    }
    assert_eq!(
        property_value(&fixed, "font-mono").as_deref(),
        Some("\"Cascadia Mono\"")
    );
}
