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

// ---- M7 Task 44: the names and the removed developer chords ----------------------------------

fn crate_sources_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Every `.rs` file under any crate's `src` folder, in a stable order.
fn rust_sources() -> Vec<PathBuf> {
    fn walk(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("read a source folder") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(crate_sources_dir()).expect("the crates folder") {
        let src = entry.expect("a directory entry").path().join("src");
        if src.is_dir() {
            walk(&src, &mut out);
        }
    }
    out.sort();
    out
}

/// The parts of a code line between double quotes (odd pieces of a split on `"`).
fn string_literals(line: &str) -> Vec<&str> {
    line.split('"').skip(1).step_by(2).collect()
}

/// The Prism Terminal names a Flare line may not carry: the key prefix `prism.` anywhere in code,
/// the folder name `PrismTerminal` anywhere in code, and the product name `Prism Terminal` inside a
/// string literal. Comment lines are exempt. (The plain word `Prism` is not Prism Terminal's name:
/// it is a theme's name in Prism Terminal's own preset data, and a folder of the owner's repos.)
fn prism_names(line: &str) -> bool {
    let code = line.trim_start();
    if code.starts_with("//") {
        return false;
    }
    if code.contains("prism.") || code.contains("PrismTerminal") {
        return true;
    }
    string_literals(code)
        .iter()
        .any(|lit| lit.contains("Prism Terminal"))
}

/// Where the Prism Terminal names may stand inside `flare-settings/src/import/` (decision O10 and the
/// Global Constraints): `map.rs` and `legacy.rs` whole; `ldb.rs` and `run.rs` only in their
/// `#[cfg(test)]` parts; in `run.rs` only inside `fn candidates`.
fn import_line_allowed(file: &str, lines: &[&str]) -> Vec<bool> {
    let test_at = lines
        .iter()
        .position(|l| l.trim() == "#[cfg(test)]")
        .unwrap_or(lines.len());
    let mut allowed = vec![false; lines.len()];
    if file.ends_with("map.rs") || file.ends_with("legacy.rs") {
        allowed.iter_mut().for_each(|a| *a = true);
    } else if file.ends_with("ldb.rs") || file.ends_with("run.rs") {
        for a in allowed.iter_mut().skip(test_at) {
            *a = true;
        }
        if file.ends_with("run.rs") {
            let mut inside = false;
            for (i, line) in lines.iter().enumerate().take(test_at) {
                if line.contains("fn candidates(") {
                    inside = true;
                }
                if inside {
                    allowed[i] = true;
                }
                if inside && line.starts_with('}') {
                    inside = false;
                }
            }
        }
    }
    allowed
}

#[test]
fn no_prism_name_outside_the_import() {
    let import_dir = crate_sources_dir().join("flare-settings/src/import");
    let mut bad = Vec::new();
    for path in rust_sources() {
        let file = path.to_string_lossy().replace('\\', "/");
        let text = fs::read_to_string(&path).expect("read a source file");
        let lines: Vec<&str> = text.lines().collect();
        let in_import = path.starts_with(&import_dir);
        let allowed = if in_import {
            import_line_allowed(&file, &lines)
        } else {
            vec![false; lines.len()]
        };
        if in_import
            && !file.ends_with("map.rs")
            && !file.ends_with("legacy.rs")
            && !file.ends_with("ldb.rs")
            && !file.ends_with("run.rs")
        {
            // mod.rs and any other file of the import folder carry no Prism name at all.
            for (i, line) in lines.iter().enumerate() {
                if prism_names(line) {
                    bad.push(format!("{file}:{}", i + 1));
                }
            }
            continue;
        }
        for (i, line) in lines.iter().enumerate() {
            if !allowed[i] && prism_names(line) {
                bad.push(format!("{file}:{}", i + 1));
            }
        }
    }
    assert!(bad.is_empty(), "Prism names outside the import: {bad:#?}");
}

#[test]
fn the_f8_and_f9_chords_are_gone() {
    let mut files = rust_sources();
    files.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/app.slint"));
    for path in files {
        let text = fs::read_to_string(&path).expect("read a source file");
        for needle in ["debug_step", "debug-step", "Key.F9", "Key.F8"] {
            assert!(
                !text.contains(needle),
                "{} still has {needle}",
                path.display()
            );
        }
    }
}

// ---- M7 Task 46: the em-dash scan and the settings files listed in app.slint ----------------

/// Every file of the given extensions under `dir`, recursively (nothing if the folder is missing).
fn files_under(dir: &std::path::Path, exts: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(read) = fs::read_dir(dir) else {
        return;
    };
    for entry in read {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            // A package folder is someone else's text (`tools/model-viewer` after `npm install`).
            if path.file_name().is_some_and(|n| n == "node_modules") {
                continue;
            }
            files_under(&path, exts, out);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| exts.contains(&e))
        {
            out.push(path);
        }
    }
}

#[test]
fn no_em_dash_in_settings_files() {
    // The Settings milestone's files: the Slint files, the Rust code of the three crates and of the
    // Settings tests, the JSON data and the Node tools. The repo-wide scan is tests/no_em_dash.rs.
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..");
    let mut files = Vec::new();
    files_under(
        &root.join("crates/flare/ui/settings"),
        &["slint"],
        &mut files,
    );
    files_under(&root.join("crates/flare/src"), &["rs"], &mut files);
    files_under(&root.join("crates/flare-settings/src"), &["rs"], &mut files);
    files_under(&root.join("crates/flare-help/src"), &["rs"], &mut files);
    files_under(&root.join("crates/flare-help/data"), &["json"], &mut files);
    files_under(
        &root.join("tools"),
        &["mjs", "js", "cjs", "ts", "json"],
        &mut files,
    );
    assert!(
        files.len() > 20,
        "the Settings files are found: {}",
        files.len()
    );
    let dash = '\u{2014}';
    let bad: Vec<String> = files
        .iter()
        .filter(|p| fs::read_to_string(p).is_ok_and(|t| t.contains(dash)))
        .map(|p| p.display().to_string())
        .collect();
    assert!(bad.is_empty(), "em dash in: {bad:#?}");
}

#[test]
fn every_settings_slint_file_is_listed_in_app_slint() {
    // A Settings file that app.slint never imports or re-exports is not compiled into the window.
    let app = fs::read_to_string(ui_dir().join("app.slint")).expect("read app.slint");
    let mut files = Vec::new();
    files_under(&ui_dir().join("settings"), &["slint"], &mut files);
    assert!(
        files.len() >= 10,
        "the Settings .slint files are found: {}",
        files.len()
    );
    for path in files {
        let name = path
            .file_name()
            .expect("a file name")
            .to_string_lossy()
            .into_owned();
        assert!(
            app.contains(&format!("settings/{name}\"")),
            "app.slint imports or re-exports settings/{name}"
        );
    }
}
