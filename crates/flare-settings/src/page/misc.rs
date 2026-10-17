//! The Diagnostics, About and Dictation page models and `build_page` (M7 Task 23).

use super::{
    BlockModel, Button, Control, DictationPage, PageEnv, PageModel, RowModel, plain_row, section,
    switch_row,
};
use crate::rows::{RowDef, row};

/// How long the Mark button says Marked (the UI reads it; the model only holds the word).
pub const MARKED_MS: u64 = 1200;

/// The table entry of a row this page draws. Every id passed here is in the tables.
fn def(id: &str) -> &'static RowDef {
    row(id).expect("the misc pages draw only rows of the tables")
}

/// The subtext of the log folder row while its path is not known.
const FOLDER_UNKNOWN: &str = "Kept on this PC, never sent.";

/// The Diagnostics page model: the verbose switch, the log folder and Mark a problem.
pub fn build_diagnostics(env: &PageEnv<'_>) -> PageModel {
    let verbose = switch_row(def("diag-verbose"), env.diag.verbose);
    let has_dir = !env.diag.dir.is_empty();
    let folder = RowModel {
        sub: if has_dir {
            env.diag.dir.clone()
        } else {
            FOLDER_UNKNOWN.to_string()
        },
        buttons: vec![Button {
            id: "open",
            label: "Open folder".to_string(),
            alt: None,
            off: !has_dir,
        }],
        ..plain_row(def("diag-folder"), Control::None)
    };
    // Both words are held by the button; `alt` is the word showing while the mark is fresh.
    let mark = RowModel {
        buttons: vec![Button {
            id: "mark",
            label: "Mark".to_string(),
            alt: env.diag_marked.then(|| "Marked".to_string()),
            off: false,
        }],
        ..plain_row(def("diag-mark"), Control::None)
    };
    PageModel {
        id: "diagnostics",
        label: "Diagnostics",
        sections: vec![section(
            "diagnostics",
            vec![
                BlockModel::Row(verbose),
                BlockModel::Row(folder),
                BlockModel::Row(mark),
            ],
        )],
    }
}

/// The About page model: the card, the version pill and the Slint badge. No update row.
pub fn build_about(env: &PageEnv<'_>) -> PageModel {
    let (line_one, line_two) = about_lines(&env.version, &env.copyright);
    let version = plain_row(def("app-version"), Control::Pill(env.version.clone()));
    PageModel {
        id: "about",
        label: "About",
        sections: vec![section(
            "about",
            vec![
                BlockModel::AboutCard {
                    name: "Flare".to_string(),
                    line_one,
                    line_two,
                },
                BlockModel::Row(version),
                BlockModel::SlintBadge,
            ],
        )],
    }
}

/// The Dictation page model: whatever the provider says.
pub fn build_dictation(env: &PageEnv<'_>, d: &dyn DictationPage) -> PageModel {
    PageModel {
        id: "dictation",
        label: "Dictation",
        sections: d.sections(env),
    }
}

/// The two lines of the About card: `Flare <version>. Made with Slint.` and the copyright.
pub fn about_lines(version: &str, copyright: &str) -> (String, String) {
    (
        format!("Flare {version}. Made with Slint."),
        copyright.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::test_env::{with_env, with_env_fields};
    use crate::page::{
        BlockModel, Control, DiagInfo, PlaceholderDictation, SectionModel, all_row_ids, row_of,
        section,
    };

    const COPYRIGHT: &str = "Copyright (c) 2026 Max Aubert. All rights reserved.";

    fn diag(verbose: bool, dir: &str) -> DiagInfo {
        DiagInfo {
            verbose,
            dir: dir.to_string(),
        }
    }

    #[test]
    fn diagnostics_has_the_three_rows_in_order() {
        with_env(|env| {
            let page = build_diagnostics(env);
            assert_eq!(page.id, "diagnostics");
            assert_eq!(page.sections.len(), 1);
            assert_eq!(page.sections[0].title, "");
            assert_eq!(
                all_row_ids(&page),
                ["diag-verbose", "diag-folder", "diag-mark"]
            );
        });
    }

    #[test]
    fn the_verbose_row_is_a_tap_switch_from_the_flag() {
        with_env_fields("0.0.0", COPYRIGHT, diag(true, ""), false, |env| {
            let page = build_diagnostics(env);
            let r = row_of(&page, "diag-verbose").expect("row");
            assert!(r.tap);
            assert_eq!(r.control, Control::Switch { on: true });
        });
    }

    #[test]
    fn the_folder_row_shows_the_path_and_disables_its_button_until_known() {
        with_env_fields("0.0.0", COPYRIGHT, diag(false, ""), false, |env| {
            let page = build_diagnostics(env);
            let r = row_of(&page, "diag-folder").expect("row");
            assert_eq!(r.sub, "Kept on this PC, never sent.");
            assert_eq!(r.buttons.len(), 1);
            assert_eq!(r.buttons[0].id, "open");
            assert_eq!(r.buttons[0].label, "Open folder");
            assert!(r.buttons[0].off);
        });
        let path = "C:\\Users\\Admin\\AppData\\Roaming\\Flare\\logs";
        with_env_fields("0.0.0", COPYRIGHT, diag(false, path), false, |env| {
            let page = build_diagnostics(env);
            let r = row_of(&page, "diag-folder").expect("row");
            assert_eq!(r.sub, path);
            assert!(!r.buttons[0].off);
        });
    }

    #[test]
    fn the_mark_button_holds_both_words_in_one_cell() {
        with_env_fields("0.0.0", COPYRIGHT, diag(false, ""), false, |env| {
            let page = build_diagnostics(env);
            let b = &row_of(&page, "diag-mark").expect("row").buttons[0];
            assert_eq!(b.id, "mark");
            assert_eq!(b.label, "Mark");
            assert_eq!(b.alt, None);
        });
        with_env_fields("0.0.0", COPYRIGHT, diag(false, ""), true, |env| {
            let page = build_diagnostics(env);
            let b = &row_of(&page, "diag-mark").expect("row").buttons[0];
            assert_eq!(b.label, "Mark");
            assert_eq!(b.alt.as_deref(), Some("Marked"));
        });
    }

    #[test]
    fn about_is_the_card_the_version_pill_and_the_slint_badge() {
        with_env_fields("1.2.3", COPYRIGHT, diag(false, ""), false, |env| {
            let page = build_about(env);
            assert_eq!(page.id, "about");
            assert_eq!(page.sections.len(), 1);
            assert_eq!(page.sections[0].title, "");
            let blocks = &page.sections[0].blocks;
            assert_eq!(blocks.len(), 3);
            match &blocks[0] {
                BlockModel::AboutCard {
                    name,
                    line_one,
                    line_two,
                } => {
                    assert_eq!(name, "Flare");
                    assert_eq!(line_one, "Flare 1.2.3. Made with Slint.");
                    assert_eq!(line_two, COPYRIGHT);
                }
                other => panic!("first block is the about card, got {other:?}"),
            }
            match &blocks[1] {
                BlockModel::Row(r) => {
                    assert_eq!(r.id, "app-version");
                    assert_eq!(r.control, Control::Pill("1.2.3".to_string()));
                }
                other => panic!("second block is the version row, got {other:?}"),
            }
            assert!(matches!(blocks[2], BlockModel::SlintBadge));
        });
    }

    #[test]
    fn the_about_line_names_the_version_and_slint() {
        let (one, two) = about_lines("0.1.0", COPYRIGHT);
        assert_eq!(one, "Flare 0.1.0. Made with Slint.");
        assert_eq!(two, COPYRIGHT);
        for line in [&one, &two] {
            assert!(!line.contains('\u{2014}'), "no em-dash in {line}");
        }
    }

    #[test]
    fn about_has_no_update_row() {
        with_env(|env| {
            let page = build_about(env);
            assert_eq!(all_row_ids(&page), ["app-version"]);
        });
    }

    struct TwoSections;

    impl DictationPage for TwoSections {
        fn sections(&self, _env: &PageEnv<'_>) -> Vec<SectionModel> {
            vec![
                section("listening", vec![BlockModel::Note("one".to_string())]),
                section("models", vec![BlockModel::Note("two".to_string())]),
            ]
        }
    }

    #[test]
    fn the_dictation_page_is_whatever_the_provider_says() {
        with_env(|env| {
            let page = build_dictation(env, &TwoSections);
            assert_eq!(page.id, "dictation");
            let ids: Vec<&str> = page.sections.iter().map(|s| s.id).collect();
            assert_eq!(ids, ["listening", "models"]);

            let placeholder = build_dictation(env, &PlaceholderDictation);
            assert_eq!(placeholder.sections.len(), 1);
            assert!(matches!(
                &placeholder.sections[0].blocks[0],
                BlockModel::Note(text) if text == "Dictation arrives in a later update."
            ));
        });
    }
}
