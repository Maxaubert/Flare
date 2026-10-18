//! slint text checks (Task 19). These read the .slint files as text, so they need no Slint build.

use std::fs;
use std::path::{Path, PathBuf};

fn ui_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui")
}

fn read(name: &str) -> String {
    fs::read_to_string(ui_dir().join(name)).unwrap_or_else(|e| panic!("read {name}: {e}"))
}

/// Every `*.slint` file under ui/ and its folders, as (path relative to ui/ with `/`, text), sorted.
fn all_slint() -> Vec<(String, String)> {
    let mut out = Vec::new();
    collect_slint(&ui_dir(), &ui_dir(), &mut out);
    out.sort();
    out
}

fn collect_slint(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
    for entry in fs::read_dir(dir).expect("read ui dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            collect_slint(root, &path, out);
        } else if path.extension().is_some_and(|e| e == "slint") {
            let name = path
                .strip_prefix(root)
                .expect("a file under ui/")
                .to_string_lossy()
                .replace('\\', "/");
            let text = fs::read_to_string(&path).expect("read slint file");
            out.push((name, text));
        }
    }
}

/// The value of `property <color> NAME: VALUE;` in theme.slint, if declared.
fn color_value(theme: &str, name: &str) -> Option<String> {
    let needle = format!("property <color> {name}:");
    let line = theme.lines().find(|l| l.contains(&needle))?;
    let after = &line[line.find(&needle)? + needle.len()..];
    let value = after.split(';').next()?.trim();
    Some(value.to_ascii_lowercase())
}

/// M1's cog (the Settings title button's icon before Task 25 moved it into `Glyphs.cog`).
const M1_GEAR: &str = "M15 12a3 3 0 1 1-6 0a3 3 0 1 1 6 0z M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09a1.65 1.65 0 0 0-1-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09a1.65 1.65 0 0 0 1.51-1 1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33h.01a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51h.01a1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82v.01a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z";

/// The Settings title button's icon: Task 25 points it at `Glyphs.cog`, which must stay M1's cog.
fn cog_commands() -> String {
    let app = read("app.slint");
    let at = app
        .find("label: \"Settings\";")
        .expect("app.slint has the Settings button");
    let tail = &app[at..];
    let start = tail
        .find("commands: ")
        .expect("Settings button has commands")
        + "commands: ".len();
    let end = tail[start..].find(';').expect("commands line ends");
    let value = tail[start..start + end].trim();
    assert_eq!(value, "Glyphs.cog", "the Settings button draws Glyphs.cog");
    M1_GEAR.to_string()
}

#[test]
fn theme_tokens_are_the_pt_default_values() {
    let theme = read("theme.slint");
    let rows: [(&str, &str); 15] = [
        ("tabs", "#171717"),
        ("tab-active", "#171717"),
        ("secondary", "#0d0d0d"),
        ("text-soft", "#c2bfb5"),
        ("dim2", "#716f6a"),
        ("hover", "#ffffff0f"),
        ("side-flat", "#1f1f1e"),
        ("side-text", "#dedacf"),
        ("side-dim", "#959390"),
        ("accent-solid", "#fe8f34"),
        ("on-accent", "#0b0b0f"),
        ("tree-folder", "#f6a25b"),
        ("control", "#1e1e1d"),
        ("danger-fill", "#b4353f"),
        ("scrim", "#00000080"),
    ];
    for (name, value) in rows {
        assert_eq!(
            color_value(&theme, name).as_deref(),
            Some(value),
            "Theme.{name} must be {value}"
        );
    }
    let font = theme
        .lines()
        .find(|l| l.contains("property <string> font-ui:"))
        .expect("Theme.font-ui declared");
    assert!(font.contains("\"Segoe UI\""), "font-ui is Segoe UI: {font}");
}

#[test]
fn glyph_paths_match_prism_terminal() {
    let glyphs = read("glyphs.slint");
    let cog = cog_commands();
    let rows: [(&str, &str); 15] = [
        ("close", "M6 6l12 12M18 6L6 18"),
        ("plus", "M12 6v12m6-6H6"),
        ("plus-sidebar", "M12 5v14m7-7H5"),
        ("plus-start", "M12 5.5v13M5.5 12h13"),
        ("chevron-down", "M6 9l6 6 6-6"),
        ("chevron-right", "M9 6l6 6-6 6"),
        (
            "search",
            "M17.5 11a6.5 6.5 0 1 0-13 0a6.5 6.5 0 1 0 13 0zM16 16l4 4",
        ),
        (
            "search-small",
            "M17 11a6 6 0 1 0-12 0a6 6 0 1 0 12 0zM20 20l-4.5-4.5",
        ),
        ("prompt", "M5 7l5 5-5 5M12 17h7"),
        ("cog", cog.as_str()),
        (
            "folder",
            "M3.5 7.5a2 2 0 0 1 2-2h3.6l2 2.4h7.4a2 2 0 0 1 2 2v6.6a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2z",
        ),
        ("folder-fill", "M2.5 5.5h6.2l2 2.6h10.8v10.4H2.5z"),
        (
            "pin",
            "M15 3l6 6-3 1-4 4 .5 4.5L10 14l-6 6-1-1 6-6-4.5-4.5L9 8l4-4z",
        ),
        ("menu-copy", "M8 8h12v12H8zM16 8V4H4v12h4"),
        ("menu-close", "M7 7l10 10M17 7L7 17"),
    ];
    for (name, commands) in rows {
        let line = format!("out property <string> {name}: \"{commands}\";");
        assert!(glyphs.contains(&line), "glyphs.slint lacks: {line}");
    }
}

/// The body text of every `global NAME { ... }` block in a file.
fn global_bodies(text: &str) -> Vec<String> {
    let mut bodies = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("global ") {
        let tail = &rest[at..];
        let Some(open) = tail.find('{') else { break };
        let mut depth = 0usize;
        let mut end = None;
        for (i, c) in tail[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(open + i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else { break };
        bodies.push(tail[open..end].to_string());
        rest = &tail[end..];
    }
    bodies
}

#[test]
fn px_helpers_exist_and_no_global_uses_phx() {
    let theme = read("theme.slint");
    assert!(
        theme.contains("export global Px"),
        "theme.slint has global Px"
    );
    assert!(
        theme.contains("pure public function snap(l: length) -> length"),
        "Px.snap"
    );
    assert!(
        theme.contains("pure public function hair() -> length"),
        "Px.hair"
    );
    for (name, text) in all_slint() {
        for body in global_bodies(&text) {
            assert!(
                !body.contains("phx"),
                "{name}: a global block uses phx: {body}"
            );
        }
    }
}

/// Every run of hex digits that follows a `#` in a text.
fn hex_literals(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (i, c) in text.char_indices() {
        if c == '#' {
            let run: String = text[i + 1..]
                .chars()
                .take_while(|c| c.is_ascii_hexdigit())
                .collect();
            if !run.is_empty() {
                found.push(run);
            }
        }
    }
    found
}

#[test]
fn no_hex_literal_outside_the_token_files() {
    for (name, text) in all_slint() {
        if name == "theme.slint" {
            continue;
        }
        for literal in hex_literals(&text) {
            let lower = literal.to_ascii_lowercase();
            let plain = matches!(lower.len(), 6 | 8)
                && (lower.starts_with("ffffff") || lower.starts_with("000000"))
                && (lower.len() == 6 || lower[6..].chars().all(|c| c.is_ascii_hexdigit()));
            assert!(
                plain,
                "{name}: #{literal} is a colour that must be a Theme token"
            );
        }
    }
}

#[test]
fn no_em_dash_in_any_slint_file() {
    for (name, text) in all_slint() {
        assert!(!text.contains('\u{2014}'), "{name} contains an em-dash");
    }
}

/// The Settings tab's Slint files (M7 Task 1 creates them; each later task fills its own).
const SETTINGS_SLINT: [&str; 11] = [
    "settings/types.slint",
    "settings/controls.slint",
    "settings/slider.slint",
    "settings/select.slint",
    "settings/row.slint",
    "settings/frame.slint",
    "settings/colour.slint",
    "settings/wall.slint",
    "settings/about.slint",
    "settings/help_panel.slint",
    "settings/view.slint",
];

#[test]
fn settings_slint_files_exist() {
    let found: Vec<String> = all_slint().into_iter().map(|(name, _)| name).collect();
    for name in SETTINGS_SLINT {
        assert!(found.iter().any(|f| f == name), "{name} exists under ui/");
    }
}

#[test]
fn settings_slint_has_no_em_dash() {
    let scanned = all_slint();
    assert!(
        scanned.len() >= 12,
        "the walk scans ui/ and its folders: {} files",
        scanned.len()
    );
    assert!(
        scanned
            .iter()
            .any(|(name, _)| name.starts_with("settings/")),
        "the walk reaches ui/settings/"
    );
    for (name, text) in scanned {
        assert!(!text.contains('\u{2014}'), "{name} contains an em-dash");
    }
}

#[test]
fn update_slint_exists() {
    let found: Vec<String> = all_slint().into_iter().map(|(name, _)| name).collect();
    assert!(
        found.iter().any(|f| f == "update.slint"),
        "update.slint exists under ui/"
    );
}
