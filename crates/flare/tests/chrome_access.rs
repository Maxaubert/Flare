//! Source scans of the chrome's accessible names (general review, ui-3; parity lines 165 and 166).
//!
//! Scan, not a runtime read, for the reason `settings_access.rs` gives: Slint exposes accessible
//! labels only through its testing backend, which replaces the platform. The tab strip, the +N
//! button, the + button, the close question's buttons, the right-click menus, the +N list and the
//! start screen must each be a named control a screen reader can read and press.

use std::fs;
use std::path::PathBuf;

fn ui(rel: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("ui")
        .join(rel);
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn every_tab_is_a_named_tab_and_the_plus_buttons_are_named_buttons() {
    let t = squash(&ui("tabs.slint"));
    assert!(t.contains("accessible-role: AccessibleRole.tab;"));
    assert!(t.contains(r#"accessible-label: row.label != "" ? row.label : row.tip;"#));
    assert!(t.contains("accessible-checked: row.active;"));
    assert!(t.contains(r#"accessible-label: root.geom.more-count + " more tabs";"#));
    assert!(t.contains("accessible-label: root.new-tab-title;"));
    // Each of the three can be pressed by a screen reader, not only named.
    assert!(t.matches("accessible-action-default").count() >= 3);
}

#[test]
fn the_close_questions_buttons_are_named_and_space_presses_them() {
    let d = squash(&ui("dialog.slint"));
    assert!(d.contains("accessible-role: button; accessible-label: root.label;"));
    assert!(d.contains("accessible-action-default"));
    assert!(
        d.contains(r#"if e.text == Key.Return || e.text == " " {"#),
        "Space acts like Enter on the focused button"
    );
}

#[test]
fn menu_rows_overflow_rows_and_the_start_screen_are_named_buttons() {
    let m = squash(&ui("menus.slint"));
    assert!(m.contains("accessible-label: root.item.label;"));
    assert!(m.contains(r#"accessible-label: root.item.label + " " + root.item.folder;"#));
    assert!(m.matches("accessible-action-default").count() >= 2);
    let s = squash(&ui("start.slint"));
    assert!(s.contains("accessible-label: root.name;"));
    assert!(s.contains(r#"accessible-label: "GitHub";"#));
    assert!(s.matches("accessible-action-default").count() >= 2);
}
