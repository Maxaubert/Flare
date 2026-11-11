//! The terminal right-click menu rows (Prism Terminal's `termMenu.tsx`, `termMenuItems`). The menu
//! fits what was clicked: a path adds its open, reveal and copy rows, a link its copy and open rows,
//! a selection its copy row. Paste and Find are always there; Command help only while its setting is on.

use crate::menus_ui::MenuRowData;
use flare_term::link_target::TermPathAt;
use flare_term::path_open::PathKind;
use flare_win::openers::REVEAL_LABEL;

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
    OpenPath {
        path: TermPathAt,
        reveal: bool,
    },
    /// "Show in side panel": the same vetted preview a left click makes, never an open.
    ShowInPanel {
        path: TermPathAt,
    },
    Copy(String),
    /// "Copy path" for a file link: a worker copies the first spelling that names something, else the text as written.
    CopyPath {
        path: TermPathAt,
    },
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
        // Only what a left click would preview: a file of a kind the panel draws (a worker vets it again, rules 13 and
        // 14, when the row is picked).
        if !dir && flare_surfaces::preview::classify(&p.abs).is_some() {
            rows.push(row(
                "Show in side panel",
                "",
                "menu-panel",
                TermMenuAct::ShowInPanel { path: p.clone() },
            ));
        }
        rows.push(row(
            REVEAL_LABEL,
            "",
            "menu-folder",
            TermMenuAct::OpenPath {
                path: p.clone(),
                reveal: true,
            },
        ));
        // A worker copies the first spelling that names something (a space where a row broke), else the text as
        // written: never a path read across a table's borders that names nothing (rule 274).
        rows.push(row(
            "Copy path",
            "",
            "menu-copy",
            TermMenuAct::CopyPath { path: p.clone() },
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
        group: false,
        flyout: false,
        parent: -1,
        icon_cmd: String::new(),
        logo: None,
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
            line: None,
            alts: Vec::new(),
        }
    }

    fn folder_path() -> TermPathAt {
        TermPathAt {
            kind: PathKind::Dir,
            abs: "C:\\work\\src".to_string(),
            text: "src".to_string(),
            base: "C:\\work".to_string(),
            line: None,
            alts: Vec::new(),
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
                REVEAL_LABEL,
                "Copy path",
                "Paste",
                "Find in scrollback"
            ]
        );

        let file = term_menu_rows(&at("", None, Some(file_path())), false, FIND, HELP);
        assert_eq!(file[0].label, "Open");
    }

    fn file_named(name: &str) -> TermPathAt {
        TermPathAt {
            kind: PathKind::File,
            abs: format!(r"C:\work\{name}"),
            text: name.to_string(),
            base: r"C:\work".to_string(),
            line: None,
            alts: Vec::new(),
        }
    }

    #[test]
    fn show_in_side_panel_follows_open_for_a_file_the_panel_draws() {
        for name in ["notes.txt", "pic.png", "main.rs", "data.json", "a.MD"] {
            let rows = term_menu_rows(&at("", None, Some(file_named(name))), false, FIND, HELP);
            assert_eq!(
                labels(&rows)[..4],
                ["Open", "Show in side panel", REVEAL_LABEL, "Copy path"],
                "{name}"
            );
            let side = &rows[1];
            assert_eq!(side.icon, "menu-panel");
            assert_eq!(
                side.act,
                TermMenuAct::ShowInPanel {
                    path: file_named(name)
                }
            );
        }
    }

    #[test]
    fn show_in_side_panel_is_offered_only_where_a_left_click_would_preview() {
        // Not a folder, not svg or pdf, not a script or program (rule 14). The click setting does not hide it: this
        // is an explicit ask.
        let none = |p: TermPathAt| term_menu_rows(&at("", None, Some(p)), false, FIND, HELP);
        for name in [
            "x.svg",
            "x.pdf",
            "run.exe",
            "go.ps1",
            "go.bat",
            "go.sh",
            "x.html",
            "Dockerfile",
        ] {
            assert!(
                !labels(&none(file_named(name))).contains(&"Show in side panel"),
                "{name}"
            );
        }
        assert!(!labels(&none(folder_path())).contains(&"Show in side panel"));
        assert!(labels(&none(file_path())).contains(&"Show in side panel"));
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
            TermMenuAct::CopyPath {
                path: folder_path()
            }
        );
    }

    /// Rule 1147: the reveal row names the file manager it opens, Finder on the Mac.
    #[test]
    fn the_reveal_row_names_the_os_file_manager() {
        let want = if cfg!(target_os = "macos") {
            "Show in Finder"
        } else {
            "Show in Explorer"
        };
        assert_eq!(REVEAL_LABEL, want);
        let rows = term_menu_rows(&at("", None, Some(file_path())), false, FIND, HELP);
        let reveal = rows
            .iter()
            .find(|r| {
                r.act
                    == TermMenuAct::OpenPath {
                        path: file_path(),
                        reveal: true,
                    }
            })
            .unwrap();
        assert_eq!(reveal.label, want);
    }

    #[test]
    fn copy_path_of_every_file_link_is_left_to_the_worker_that_finds_the_one_that_exists() {
        let mut p = file_named("MyDocs\\a.pdf");
        p.alts = vec!["My Docs\\a.pdf".to_string()];
        for p in [p, file_named("a.pdf")] {
            let rows = term_menu_rows(&at("", None, Some(p.clone())), false, FIND, HELP);
            let copy = rows.iter().find(|r| r.label == "Copy path").unwrap();
            assert_eq!(copy.act, TermMenuAct::CopyPath { path: p });
        }
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
        let reveal = rows.iter().find(|r| r.label == REVEAL_LABEL).unwrap();
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
            // The window draws what `glyph_path` answers; "" draws nothing (Task 27).
            assert!(
                !crate::menus_ui::glyph_path(r.icon).is_empty(),
                "no path for {}",
                r.icon
            );
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
