//! The terminal right-click menu rows (Prism Terminal's `termMenu.tsx`, `termMenuItems`). The menu
//! fits what was clicked: a path adds its open, reveal and copy rows, a link its copy and open rows,
//! a selection its copy row. Paste and Find are always there; Command help only while its setting is on.

#![allow(dead_code)] // wired into the window by Task 23

use crate::menus_ui::MenuRowData;
use flare_term::link_target::TermPathAt;
use flare_term::path_open::PathKind;

/// Where the terminal's menu was opened and what lay under the pointer.
pub struct TermMenuAt {
    pub tab: String,
    pub x: f32,
    pub y: f32,
    pub selection: String,
    pub link: Option<String>,
    pub path: Option<TermPathAt>,
}

/// What a menu row does when picked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TermMenuAct {
    OpenPath { path: TermPathAt, reveal: bool },
    Copy(String),
    OpenLink(String),
    Paste,
    Find,
    Help,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TermMenuRow {
    pub label: String,
    pub hint: String,
    pub icon: &'static str,
    pub act: TermMenuAct,
}

/// Rows in order: path rows, link rows, `Copy` (only with a selection), `Paste`, `Find in scrollback`,
/// and `Command help` last (only when `help_on`). No `Close tab`.
pub fn term_menu_rows(
    m: &TermMenuAt,
    help_on: bool,
    find_label: &str,
    help_label: &str,
) -> Vec<TermMenuRow> {
    let mut rows = Vec::new();
    if let Some(p) = &m.path {
        let dir = p.kind == PathKind::Dir;
        let (label, icon) = if dir {
            ("Open folder", "menu-folder")
        } else {
            ("Open", "menu-open")
        };
        rows.push(row(
            label,
            "",
            icon,
            TermMenuAct::OpenPath {
                path: p.clone(),
                reveal: false,
            },
        ));
        rows.push(row(
            "Show in Explorer",
            "",
            "menu-folder",
            TermMenuAct::OpenPath {
                path: p.clone(),
                reveal: true,
            },
        ));
        rows.push(row(
            "Copy path",
            "",
            "menu-copy",
            TermMenuAct::Copy(p.abs.clone()),
        ));
    }
    if let Some(link) = &m.link {
        rows.push(row(
            "Copy link",
            "",
            "menu-link",
            TermMenuAct::Copy(link.clone()),
        ));
        rows.push(row(
            "Open link",
            "",
            "menu-open",
            TermMenuAct::OpenLink(link.clone()),
        ));
    }
    if !m.selection.is_empty() {
        rows.push(row(
            "Copy",
            "Ctrl+C",
            "menu-copy",
            TermMenuAct::Copy(m.selection.clone()),
        ));
    }
    rows.push(row("Paste", "Ctrl+V", "menu-paste", TermMenuAct::Paste));
    rows.push(row(
        "Find in scrollback",
        find_label,
        "menu-find",
        TermMenuAct::Find,
    ));
    if help_on {
        rows.push(row(
            "Command help",
            help_label,
            "menu-help",
            TermMenuAct::Help,
        ));
    }
    rows
}

fn row(label: &str, hint: &str, icon: &'static str, act: TermMenuAct) -> TermMenuRow {
    TermMenuRow {
        label: label.to_string(),
        hint: hint.to_string(),
        icon,
        act,
    }
}

/// The Slint-facing row: the same `MenuRowData` the other menus use (no second conversion).
pub fn to_menu_row(r: &TermMenuRow) -> MenuRowData {
    MenuRowData {
        label: r.label.clone(),
        hint: r.hint.clone(),
        icon: r.icon,
        filled_icon: false,
        disabled: false,
        trailing_icon: None,
        trailing_filled: false,
        trailing_tip: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_tabs::keymap::{ChordId, key_label};

    const FIND: &str = "Ctrl+F";
    const HELP: &str = "F1";

    fn file_path() -> TermPathAt {
        TermPathAt {
            kind: PathKind::File,
            abs: "C:\\work\\notes.txt".to_string(),
            text: "notes.txt".to_string(),
            base: "C:\\work".to_string(),
        }
    }

    fn folder_path() -> TermPathAt {
        TermPathAt {
            kind: PathKind::Dir,
            abs: "C:\\work\\src".to_string(),
            text: "src".to_string(),
            base: "C:\\work".to_string(),
        }
    }

    fn at(selection: &str, link: Option<&str>, path: Option<TermPathAt>) -> TermMenuAt {
        TermMenuAt {
            tab: "t1".to_string(),
            x: 10.0,
            y: 20.0,
            selection: selection.to_string(),
            link: link.map(str::to_string),
            path,
        }
    }

    fn labels(rows: &[TermMenuRow]) -> Vec<&str> {
        rows.iter().map(|r| r.label.as_str()).collect()
    }

    #[test]
    fn the_menu_fits_what_was_clicked() {
        let plain = term_menu_rows(&at("", None, None), false, FIND, HELP);
        assert_eq!(labels(&plain), ["Paste", "Find in scrollback"]);

        let selected = term_menu_rows(&at("x", None, None), false, FIND, HELP);
        assert_eq!(labels(&selected), ["Copy", "Paste", "Find in scrollback"]);

        let link = term_menu_rows(&at("", Some("https://a.b"), None), false, FIND, HELP);
        assert_eq!(
            labels(&link),
            ["Copy link", "Open link", "Paste", "Find in scrollback"]
        );

        let folder = term_menu_rows(&at("", None, Some(folder_path())), false, FIND, HELP);
        assert_eq!(
            labels(&folder),
            [
                "Open folder",
                "Show in Explorer",
                "Copy path",
                "Paste",
                "Find in scrollback"
            ]
        );

        let file = term_menu_rows(&at("", None, Some(file_path())), false, FIND, HELP);
        assert_eq!(file[0].label, "Open");
    }

    #[test]
    fn command_help_is_last_and_only_while_on() {
        let on = term_menu_rows(&at("", None, None), true, FIND, HELP);
        assert_eq!(on.last().map(|r| r.label.as_str()), Some("Command help"));
        assert_eq!(on.last().map(|r| r.hint.as_str()), Some(HELP));

        let off = term_menu_rows(&at("", None, None), false, FIND, HELP);
        assert!(!labels(&off).contains(&"Command help"));
    }

    #[test]
    fn there_is_no_close_tab() {
        let rows = term_menu_rows(
            &at("sel", Some("https://a.b"), Some(file_path())),
            true,
            FIND,
            HELP,
        );
        assert!(!labels(&rows).contains(&"Close tab"));
    }

    #[test]
    fn copies_exactly_what_was_under_the_pointer() {
        let rows = term_menu_rows(
            &at("sel", Some("https://a.b"), Some(folder_path())),
            false,
            FIND,
            HELP,
        );
        let act = |label: &str| {
            rows.iter()
                .find(|r| r.label == label)
                .map(|r| r.act.clone())
                .unwrap()
        };
        assert_eq!(
            act("Copy link"),
            TermMenuAct::Copy("https://a.b".to_string())
        );
        assert_eq!(act("Copy"), TermMenuAct::Copy("sel".to_string()));
        assert_eq!(
            act("Copy path"),
            TermMenuAct::Copy("C:\\work\\src".to_string())
        );
    }

    #[test]
    fn open_path_rows_carry_the_path_and_the_mode() {
        let rows = term_menu_rows(&at("", None, Some(file_path())), false, FIND, HELP);
        let open = rows.iter().find(|r| r.label == "Open").unwrap();
        assert_eq!(
            open.act,
            TermMenuAct::OpenPath {
                path: file_path(),
                reveal: false
            }
        );
        let reveal = rows.iter().find(|r| r.label == "Show in Explorer").unwrap();
        assert_eq!(
            reveal.act,
            TermMenuAct::OpenPath {
                path: file_path(),
                reveal: true
            }
        );
    }

    #[test]
    fn the_hints_are_the_keys() {
        let find = key_label(ChordId::Find, false);
        let rows = term_menu_rows(&at("sel", Some("https://a.b"), None), true, find, HELP);
        let hint = |label: &str| {
            rows.iter()
                .find(|r| r.label == label)
                .map(|r| r.hint.clone())
                .unwrap()
        };
        assert_eq!(hint("Copy"), "Ctrl+C");
        assert_eq!(hint("Paste"), "Ctrl+V");
        assert_eq!(hint("Find in scrollback"), find);
        assert_eq!(hint("Copy link"), "");
        assert_eq!(hint("Open link"), "");
    }

    #[test]
    fn every_row_has_an_icon_that_exists_in_glyphs() {
        let slint = include_str!("../ui/glyphs.slint");
        let rows = term_menu_rows(
            &at("sel", Some("https://a.b"), Some(folder_path())),
            true,
            FIND,
            HELP,
        );
        for r in &rows {
            let decl = format!("out property <string> {}:", r.icon);
            assert!(slint.contains(&decl), "missing glyph {}", r.icon);
        }
    }

    #[test]
    fn no_em_dash_in_any_label() {
        let rows = term_menu_rows(
            &at("sel", Some("https://a.b"), Some(folder_path())),
            true,
            FIND,
            HELP,
        );
        for r in &rows {
            assert!(!r.label.contains('\u{2014}'));
            assert!(!r.hint.contains('\u{2014}'));
        }
    }

    #[test]
    fn to_menu_row_keeps_label_hint_and_icon() {
        let rows = term_menu_rows(&at("sel", None, None), false, FIND, HELP);
        let m = to_menu_row(&rows[0]);
        assert_eq!(m.label, "Copy");
        assert_eq!(m.hint, "Ctrl+C");
        assert_eq!(m.icon, "menu-copy");
        assert!(!m.disabled);
    }
}
