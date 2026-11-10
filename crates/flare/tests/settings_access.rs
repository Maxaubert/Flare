//! Source scans of the Settings and help panel names and fonts (M10 Task 45, parity lines 701, 705,
//! 710, 729 and 806).
//!
//! Scan, not a runtime read: Slint exposes accessible labels only through its testing backend, which
//! replaces the platform (the real window and renderer would no longer be under test), so these tests
//! read the `.slint` source the way `settings_text.rs` does.

use std::fs;
use std::path::PathBuf;

fn ui(rel: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("ui")
        .join(rel);
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

fn src(rel: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join(rel);
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

/// The text with every run of whitespace (line ends included) squeezed to one space.
fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The text of `component NAME inherits ...` up to the next top-level component.
fn component(text: &str, name: &str) -> String {
    let head = format!("component {name} inherits");
    let at = text
        .find(&head)
        .unwrap_or_else(|| panic!("component {name} exists"));
    let tail = &text[at + head.len()..];
    let end = ["\ncomponent ", "\nexport component "]
        .iter()
        .filter_map(|m| tail.find(m))
        .min()
        .unwrap_or(tail.len());
    tail[..end].to_string()
}

#[test]
fn the_rail_is_a_tab_list_named_settings_pages_and_each_button_names_its_page() {
    let frame = ui("settings/frame.slint");
    let set_frame = component(&frame, "SetFrame");
    assert!(
        set_frame.contains("accessible-role: tab-list;"),
        "rail role"
    );
    assert!(
        set_frame.contains("accessible-label: \"Settings pages\";"),
        "rail name"
    );
    let button = component(&frame, "RailButton");
    assert!(button.contains("accessible-role: tab;"), "button role");
    assert!(
        button.contains("accessible-label: root.item.label;"),
        "button name is the page label"
    );
    assert!(
        button.contains("accessible-checked: root.on;"),
        "the chosen page is the checked one"
    );
    assert!(
        set_frame.contains("chosen => { root.page-chosen(item.id); }"),
        "a button reports its page id"
    );
}

#[test]
fn the_find_field_and_its_clear_button_are_named() {
    let frame = ui("settings/frame.slint");
    let set_frame = component(&frame, "SetFrame");
    assert!(
        squash(&set_frame)
            .contains("accessible-role: text-input; accessible-label: \"Find a setting\";"),
        "the field is a text input named Find a setting"
    );
    assert!(
        set_frame.contains("text: \"Find a setting\";"),
        "the placeholder says Find a setting"
    );
    assert!(
        set_frame.contains("out property <bool> clear-visible: root.find-text != \"\";"),
        "the clear button shows only with text"
    );
    let at = set_frame
        .find("if root.clear-visible: clear-ta := TouchArea")
        .expect("the clear button is drawn only while clear-visible");
    let clear = &set_frame[at..at + 400];
    assert!(clear.contains("accessible-role: button;"));
    assert!(clear.contains("accessible-label: \"Clear the search\";"));
}

#[test]
fn every_value_control_is_named_by_its_row_label_and_described_by_its_subtext() {
    let cases = [
        ("settings/controls.slint", "SetSwitch"),
        ("settings/controls.slint", "SetSegmented"),
        ("settings/select.slint", "SetSelect"),
        ("settings/slider.slint", "SetSlider"),
        ("settings/colour.slint", "ColourField"),
        ("settings/controls.slint", "SetTextField"),
    ];
    for (file, name) in cases {
        let body = component(&ui(file), name);
        assert!(
            body.contains("in property <string> sub;"),
            "{name} takes the row's subtext"
        );
        assert!(
            body.contains("accessible-label: root.label;"),
            "{name} is named by the row label"
        );
        assert!(
            body.contains("accessible-description: root.sub;"),
            "{name} is described by the row subtext"
        );
    }
}

#[test]
fn the_row_hands_its_label_and_subtext_to_each_control() {
    let view = ui("settings/view.slint");
    let controls = component(&view, "RowControls");
    // switch, segmented, select, slider and the colour field, then the dictation key field and the microphone
    // (M8 Task 25), and the text field (plugins phase 7): eight controls, each given both.
    assert_eq!(
        controls.matches("label: root.data.label;").count(),
        8,
        "eight controls carry the row label"
    );
    assert_eq!(
        controls.matches("sub: root.data.sub;").count(),
        8,
        "eight controls carry the row subtext"
    );
}

#[test]
fn the_help_search_field_is_a_combobox_with_the_long_label() {
    let panel = ui("settings/help_panel.slint");
    assert!(
        squash(&panel)
            .contains("accessible-role: combobox; accessible-label: HelpBridge.search-label;"),
        "the field is a combobox named by the bridge's search label"
    );
    assert_eq!(
        flare_help::panel::SEARCH_LABEL,
        "Search commands. Up and Down move through the results, Enter copies the highlighted command."
    );
    assert_eq!(
        flare_help::panel::PLACEHOLDER,
        "Try: find big files, what is using port 3000"
    );
    assert!(
        src("help_ctl.rs").contains("b.set_search_label(SEARCH_LABEL.into());"),
        "the host hands the label to the bridge"
    );
}

#[test]
fn the_settings_font_is_the_system_ui_stack_whatever_the_theme() {
    // The two page faces are fixed tokens: Segoe UI Variable, falling back to Segoe UI.
    // The faces sit behind the platform seam (macOS port phase 2, step 4): these are the Windows
    // ones, and theme.slint reads them from there.
    let theme = ui("theme.slint");
    assert!(theme.contains("font-text: PlatformFonts.text;"));
    assert!(theme.contains("font-display: PlatformFonts.display;"));
    assert!(theme.contains("font-fallback: PlatformFonts.ui;"));
    assert!(theme.contains("out property <string> font-ui: PlatformFonts.ui;"));
    let fonts = ui("platform_fonts.slint");
    assert!(fonts.contains("windows-text: \"Segoe UI Variable Text\";"));
    assert!(fonts.contains("windows-display: \"Segoe UI Variable Display\";"));
    assert!(fonts.contains("windows-ui: \"Segoe UI\";"));
    assert!(fonts.contains("text: self.mac ? self.macos-ui : self.windows-text;"));
    assert!(fonts.contains("display: self.mac ? self.macos-ui : self.windows-display;"));
    let ctl = src("settings_ctl.rs");
    assert!(
        ctl.contains("\"Segoe UI\".to_string()"),
        "a missing variable face falls back to Segoe UI"
    );
    // No theme sets any face: Rust never writes the UI font or the page faces from a theme.
    for file in ["settings_ctl.rs", "theme_apply.rs", "theme_bridge.rs"] {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join(file);
        if let Ok(text) = fs::read_to_string(p) {
            assert!(
                !text.contains("set_font_ui"),
                "{file} must not change the UI font"
            );
        }
    }
    // No Settings file draws in the terminal face; the font option is the one face-per-option use.
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("ui")
        .join("settings");
    for entry in fs::read_dir(dir).expect("read ui/settings") {
        let path = entry.expect("entry").path();
        if path.extension().is_some_and(|e| e == "slint") {
            let text = fs::read_to_string(&path).expect("read slint");
            assert!(
                !text.contains("term-font"),
                "{} must not use the terminal font",
                path.display()
            );
        }
    }
}
