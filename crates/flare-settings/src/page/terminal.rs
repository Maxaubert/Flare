//! The Terminal page model (M7 Task 21).
//!
//! Source: Prism Terminal's `TerminalPage.tsx`, `ShellSection`, `TerminalTextSection` and
//! `HelpSection`. Sections in order: shell (only once the detected list is in), opening, keys,
//! links (this app's, Surfaces stage 1), text, help.

use super::{
    BlockModel, Button, Control, Opt, PageEnv, PageModel, RowModel, SectionModel, plain_row,
    section, switch_row,
};
use crate::agent_prefs::agent_guard;
use crate::help_prefs::help_enabled;
use crate::look::CARET_SHAPES;
use crate::new_tab::new_tab_folder;
use crate::rows::{RowDef, row};
use crate::term_prefs::{
    agent_eval, agent_keys, app_network, dev_preview, hover_thumbs, open_previews,
    right_click_reports, saved_shell_id, wrap_previews,
};
use crate::verb::VerbState;
use flare_theme::fonts::{FONT_PCTS, TERM_FONTS};

/// The tooltip of the keep-agents-open row: its one exception (PT's `agent-guard` title).
pub const GUARD_TIP: &str = "Ctrl+C and Ctrl+D never reach an agent. Ctrl+C still stops one while it works, and copies a selection.";

/// The row's table entry. Every id used here is a row of `rows`, so a missing one is a bug.
fn def(id: &str) -> &'static RowDef {
    row(id).expect("a row of the closed tables")
}

/// The Shell section: drawn only once the detected list has arrived and is not empty. A saved id
/// that is no longer in the list shows the first shell, the one a new terminal would launch.
fn shell_section(env: &PageEnv<'_>) -> Option<SectionModel> {
    let list = env.shells.as_ref().filter(|l| !l.is_empty())?;
    let saved = saved_shell_id(env.prefs);
    let value = saved
        .filter(|id| list.iter().any(|(sid, _)| sid == id))
        .or_else(|| list.first().map(|(id, _)| id.clone()))?;
    let options = list
        .iter()
        .map(|(id, name)| Opt {
            id: id.clone(),
            name: name.clone(),
            face: String::new(),
        })
        .collect();
    let row = plain_row(def("term-shell"), Control::Select { value, options });
    Some(section("shell", vec![BlockModel::Row(row)]))
}

/// The Opening terminals section: the new-tab folder and its buttons, and the Explorer row. There is no
/// ask mode any more (2026-10-27): the folder menu's "Choose a folder…" row does that job.
fn opening_section(env: &PageEnv<'_>) -> SectionModel {
    let folder = new_tab_folder(env.prefs);
    let has_folder = !folder.is_empty();

    let newtab = def("newtab-mode");
    let sub = if has_folder {
        "Opens in the folder you chose.".to_string()
    } else {
        newtab.sub.to_string()
    };
    let row_tip = if has_folder {
        folder.clone()
    } else {
        env.home.clone()
    };
    let mut buttons = Vec::new();
    if has_folder {
        buttons.push(Button {
            id: "use-home",
            label: "Use my user folder".to_string(),
            alt: None,
            off: false,
            ..Button::default()
        });
    }
    buttons.push(Button {
        id: "choose-folder",
        label: "Choose folder\u{2026}".to_string(),
        alt: None,
        off: false,
        ..Button::default()
    });
    let newtab_row = RowModel {
        sub,
        row_tip,
        buttons,
        ..plain_row(newtab, Control::None)
    };

    // The Explorer menu row reads the registry's state: while Windows is being asked it is off.
    let verb = def("explorer-verb");
    let busy = env.verb == VerbState::Busy;
    let verb_row = RowModel {
        tap: true,
        off: busy,
        sub: if busy {
            "Checking with Windows.".to_string()
        } else {
            verb.sub.to_string()
        },
        ..plain_row(
            verb,
            Control::Switch {
                on: env.verb == VerbState::On,
            },
        )
    };

    section(
        "opening",
        vec![BlockModel::Row(newtab_row), BlockModel::Row(verb_row)],
    )
}

/// The Keys section: the agent hotkeys (the tooltip lists the chords) and keep agents open.
fn keys_section(env: &PageEnv<'_>) -> SectionModel {
    let hotkeys = RowModel {
        row_tip: env.agent_key_labels.clone(),
        ..switch_row(def("agent-keys"), agent_keys(env.prefs))
    };
    let guard = RowModel {
        row_tip: GUARD_TIP.to_string(),
        ..switch_row(def("agent-guard"), agent_guard(env.prefs))
    };
    let right_click = switch_row(def("right-click-reports"), right_click_reports(env.prefs));
    section(
        "keys",
        vec![
            BlockModel::Row(hotkeys),
            BlockModel::Row(guard),
            BlockModel::Row(right_click),
        ],
    )
}

/// The Links section: the previews switch, on by default (Surfaces stage 1), and the switch that lets an
/// agent run scripts in its web panel, off by default (stage 4), and the one that lets a mini-app load from
/// the web, off by default (stage 5).
fn links_section(env: &PageEnv<'_>) -> SectionModel {
    section(
        "links",
        vec![
            BlockModel::Row(switch_row(def("open-previews"), open_previews(env.prefs))),
            BlockModel::Row(switch_row(def("wrap-previews"), wrap_previews(env.prefs))),
            BlockModel::Row(switch_row(def("hover-thumbs"), hover_thumbs(env.prefs))),
            BlockModel::Row(switch_row(def("agent-eval"), agent_eval(env.prefs))),
            BlockModel::Row(switch_row(def("app-network"), app_network(env.prefs))),
            BlockModel::Row(switch_row(def("dev-preview"), dev_preview(env.prefs))),
        ],
    )
}

/// The Text section: the face (each name in its own face) and the size, 50 to 200 in tens.
fn text_section(env: &PageEnv<'_>) -> SectionModel {
    let faces = TERM_FONTS
        .iter()
        .map(|f| Opt {
            id: f.id.to_string(),
            name: f.name.to_string(),
            face: (env.face_of)(f.id),
        })
        .collect();
    let family = plain_row(
        def("term-font-family"),
        Control::Select {
            value: env.look.font_id().to_string(),
            options: faces,
        },
    );
    let sizes = FONT_PCTS
        .iter()
        .map(|p| Opt {
            id: p.to_string(),
            name: format!("{p}%"),
            face: String::new(),
        })
        .collect();
    let size = plain_row(
        def("term-font"),
        Control::Select {
            value: env.look.font_pct().to_string(),
            options: sizes,
        },
    );
    // The caret (rule 1233): its shape from the list, and whether it blinks.
    let shapes = CARET_SHAPES
        .iter()
        .map(|(id, name)| Opt {
            id: (*id).to_string(),
            name: (*name).to_string(),
            face: String::new(),
        })
        .collect();
    let caret = plain_row(
        def("term-caret"),
        Control::Select {
            value: env.look.caret_shape().to_string(),
            options: shapes,
        },
    );
    let blink = switch_row(def("term-caret-blink"), env.look.caret_blink());
    section(
        "text",
        vec![
            BlockModel::Row(family),
            BlockModel::Row(size),
            BlockModel::Row(caret),
            BlockModel::Row(blink),
        ],
    )
}

/// The Command help section: one switch, on by default.
fn help_section(env: &PageEnv<'_>) -> SectionModel {
    section(
        "help",
        vec![BlockModel::Row(switch_row(
            def("help-enabled"),
            help_enabled(env.prefs),
        ))],
    )
}

/// The Terminal page, in Prism Terminal's order.
pub fn build_terminal(env: &PageEnv<'_>) -> PageModel {
    let mut sections: Vec<SectionModel> = Vec::new();
    sections.extend(shell_section(env));
    sections.push(opening_section(env));
    sections.push(keys_section(env));
    sections.push(links_section(env));
    sections.push(text_section(env));
    sections.push(help_section(env));
    PageModel {
        id: "terminal",
        label: "Terminal",
        sections,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_prefs::set_agent_guard;
    use crate::help_prefs::set_help_enabled;
    use crate::kv::PrefStore;
    use crate::look::LookStore;
    use crate::new_tab::set_new_tab_folder;
    use crate::page::all_row_ids;
    use crate::page::{DiagInfo, GlassSupport, PageEnv, ThemeColours};
    use crate::term_prefs::{save_shell_id, set_agent_keys};

    fn face_of_test(id: &str) -> String {
        format!("face:{id}")
    }

    fn env<'a>(
        prefs: &'a PrefStore,
        look: &'a LookStore,
        face: &'a dyn Fn(&str) -> String,
    ) -> PageEnv<'a> {
        PageEnv {
            prefs,
            look,
            shells: None,
            verb: VerbState::Off,
            home: String::new(),
            version: "0.0.0".to_string(),
            copyright: String::new(),
            glass: GlassSupport::Ok,
            diag: DiagInfo {
                verbose: false,
                dir: String::new(),
            },
            diag_marked: false,
            nvidia: false,
            dictation_wired: false,
            theme_colours: ThemeColours {
                accent: String::new(),
                background: String::new(),
                secondary: String::new(),
            },
            agent_colours: [String::new(), String::new(), String::new()],
            agent_key_labels: "Ctrl+S, Ctrl+Tab".to_string(),
            face_of: face,
            mods: None,
            machines: None,
        }
    }

    fn shells() -> Option<Vec<(String, String)>> {
        Some(vec![
            ("pwsh".to_string(), "PowerShell".to_string()),
            ("cmd".to_string(), "Command Prompt".to_string()),
        ])
    }

    /// The row with this id on the built page, or a panic naming it.
    fn the_row<'p>(page: &'p PageModel, id: &str) -> &'p RowModel {
        super::super::row_of(page, id).expect("the row is on the page")
    }

    fn section_ids(page: &PageModel) -> Vec<&'static str> {
        page.sections.iter().map(|s| s.id).collect()
    }

    #[test]
    fn no_shell_section_until_the_list_arrives() {
        let prefs = PrefStore::memory();
        let look = LookStore::new(prefs.clone());
        let face: &dyn Fn(&str) -> String = &face_of_test;
        let mut e = env(&prefs, &look, face);
        assert!(!section_ids(&build_terminal(&e)).contains(&"shell"));
        e.shells = Some(Vec::new());
        assert!(!section_ids(&build_terminal(&e)).contains(&"shell"));
        e.shells = shells();
        assert!(section_ids(&build_terminal(&e)).contains(&"shell"));
    }

    #[test]
    fn a_saved_shell_that_has_gone_shows_the_first_shell() {
        let prefs = PrefStore::memory();
        let look = LookStore::new(prefs.clone());
        let face: &dyn Fn(&str) -> String = &face_of_test;
        let mut e = env(&prefs, &look, face);
        e.shells = shells();
        save_shell_id(&prefs, "gone");
        match &the_row(&build_terminal(&e), "term-shell").control {
            Control::Select { value, .. } => assert_eq!(value, "pwsh"),
            other => panic!("expected a select, got {other:?}"),
        }
    }

    #[test]
    fn a_saved_shell_in_the_list_is_the_value() {
        let prefs = PrefStore::memory();
        let look = LookStore::new(prefs.clone());
        let face: &dyn Fn(&str) -> String = &face_of_test;
        let mut e = env(&prefs, &look, face);
        e.shells = shells();
        save_shell_id(&prefs, "cmd");
        match &the_row(&build_terminal(&e), "term-shell").control {
            Control::Select { value, options } => {
                assert_eq!(value, "cmd");
                assert_eq!(options.len(), 2);
                assert_eq!(options[1].name, "Command Prompt");
            }
            other => panic!("expected a select, got {other:?}"),
        }
    }

    #[test]
    fn sections_and_rows_are_in_the_reference_order() {
        let prefs = PrefStore::memory();
        let look = LookStore::new(prefs.clone());
        let face: &dyn Fn(&str) -> String = &face_of_test;
        let mut e = env(&prefs, &look, face);
        e.shells = shells();
        let page = build_terminal(&e);
        assert_eq!(page.id, "terminal");
        assert_eq!(
            section_ids(&page),
            vec!["shell", "opening", "keys", "links", "text", "help"]
        );
        assert_eq!(
            all_row_ids(&page),
            vec![
                "term-shell",
                "newtab-mode",
                "explorer-verb",
                "agent-keys",
                "agent-guard",
                "right-click-reports",
                "open-previews",
                "wrap-previews",
                "hover-thumbs",
                "agent-eval",
                "app-network",
                "dev-preview",
                "term-font-family",
                "term-font",
                "term-caret",
                "term-caret-blink",
                "help-enabled",
            ]
        );
    }

    #[test]
    fn the_caret_rows_are_a_shape_list_and_a_blink_switch_with_no_tooltip() {
        let prefs = PrefStore::memory();
        let look = LookStore::new(prefs.clone());
        let face: &dyn Fn(&str) -> String = &face_of_test;
        let page = build_terminal(&env(&prefs, &look, face));
        let shape = the_row(&page, "term-caret");
        assert_eq!(shape.label, "Caret shape");
        assert_eq!(shape.row_tip, "", "a labelled control has no tooltip");
        match &shape.control {
            Control::Select { value, options } => {
                assert_eq!(value, "block");
                let names: Vec<&str> = options.iter().map(|o| o.name.as_str()).collect();
                assert_eq!(names, ["Block", "Hollow block", "Bar", "Underline", "Pill"]);
            }
            other => panic!("expected a select, got {other:?}"),
        }
        let blink = the_row(&page, "term-caret-blink");
        assert_eq!(blink.label, "Caret blink");
        assert_eq!(blink.row_tip, "");
        assert_eq!(blink.control, Control::Switch { on: true });

        look.set_caret_shape("pill");
        look.set_caret_blink(false);
        let page = build_terminal(&env(&prefs, &look, face));
        match &the_row(&page, "term-caret").control {
            Control::Select { value, .. } => assert_eq!(value, "pill"),
            other => panic!("expected a select, got {other:?}"),
        }
        assert_eq!(
            the_row(&page, "term-caret-blink").control,
            Control::Switch { on: false }
        );
    }

    #[test]
    fn the_previews_row_is_a_switch_that_is_on_until_turned_off() {
        let prefs = PrefStore::memory();
        let look = LookStore::new(prefs.clone());
        let face: &dyn Fn(&str) -> String = &face_of_test;
        let e = env(&prefs, &look, face);
        let page = build_terminal(&e);
        assert_eq!(
            the_row(&page, "open-previews").control,
            Control::Switch { on: true }
        );
        crate::term_prefs::set_open_previews(&prefs, false);
        let page = build_terminal(&env(&prefs, &look, face));
        assert_eq!(
            the_row(&page, "open-previews").control,
            Control::Switch { on: false }
        );
    }

    #[test]
    fn the_hover_thumbnails_row_is_a_switch_that_is_on_until_turned_off() {
        let prefs = PrefStore::memory();
        let look = LookStore::new(prefs.clone());
        let face: &dyn Fn(&str) -> String = &face_of_test;
        let page = build_terminal(&env(&prefs, &look, face));
        assert_eq!(
            the_row(&page, "hover-thumbs").control,
            Control::Switch { on: true }
        );
        crate::term_prefs::set_hover_thumbs(&prefs, false);
        let page = build_terminal(&env(&prefs, &look, face));
        assert_eq!(
            the_row(&page, "hover-thumbs").control,
            Control::Switch { on: false }
        );
    }

    #[test]
    fn the_wrap_row_is_a_switch_that_is_on_until_turned_off() {
        let prefs = PrefStore::memory();
        let look = LookStore::new(prefs.clone());
        let face: &dyn Fn(&str) -> String = &face_of_test;
        let page = build_terminal(&env(&prefs, &look, face));
        let row = the_row(&page, "wrap-previews");
        assert_eq!(row.control, Control::Switch { on: true });
        assert_eq!(row.label, "Wrap long lines in previews");
        crate::term_prefs::set_wrap_previews(&prefs, false);
        let page = build_terminal(&env(&prefs, &look, face));
        assert_eq!(
            the_row(&page, "wrap-previews").control,
            Control::Switch { on: false }
        );
    }

    #[test]
    fn the_scripts_row_is_a_switch_that_is_off_until_turned_on() {
        let prefs = PrefStore::memory();
        let look = LookStore::new(prefs.clone());
        let face: &dyn Fn(&str) -> String = &face_of_test;
        let page = build_terminal(&env(&prefs, &look, face));
        assert_eq!(
            the_row(&page, "agent-eval").control,
            Control::Switch { on: false }
        );
        crate::term_prefs::set_agent_eval(&prefs, true);
        let page = build_terminal(&env(&prefs, &look, face));
        assert_eq!(
            the_row(&page, "agent-eval").control,
            Control::Switch { on: true }
        );
    }

    #[test]
    fn the_mini_app_network_row_is_a_switch_that_is_off_until_turned_on() {
        let prefs = PrefStore::memory();
        let look = LookStore::new(prefs.clone());
        let face: &dyn Fn(&str) -> String = &face_of_test;
        let page = build_terminal(&env(&prefs, &look, face));
        assert_eq!(
            the_row(&page, "app-network").control,
            Control::Switch { on: false }
        );
        crate::term_prefs::set_app_network(&prefs, true);
        let page = build_terminal(&env(&prefs, &look, face));
        assert_eq!(
            the_row(&page, "app-network").control,
            Control::Switch { on: true }
        );
    }

    #[test]
    fn the_local_server_preview_row_is_a_switch_that_is_on_until_turned_off() {
        let prefs = PrefStore::memory();
        let look = LookStore::new(prefs.clone());
        let face: &dyn Fn(&str) -> String = &face_of_test;
        let page = build_terminal(&env(&prefs, &look, face));
        let row = the_row(&page, "dev-preview");
        assert_eq!(row.control, Control::Switch { on: true });
        assert_eq!(row.label, "Offer previews of local servers");
        crate::term_prefs::set_dev_preview(&prefs, false);
        let page = build_terminal(&env(&prefs, &look, face));
        assert_eq!(
            the_row(&page, "dev-preview").control,
            Control::Switch { on: false }
        );
    }

    #[test]
    fn the_new_tab_row_has_no_mode_to_pick() {
        let prefs = PrefStore::memory();
        let look = LookStore::new(prefs.clone());
        let face: &dyn Fn(&str) -> String = &face_of_test;
        let e = env(&prefs, &look, face);
        // A profile from before 2026-10-27 may still say "ask": the row reads as the folder row all the same.
        prefs.set("flare.newtab.mode", "ask");
        let r = the_row(&build_terminal(&e), "newtab-mode").clone();
        assert_eq!(r.control, Control::None);
        assert_eq!(r.sub, "Opens in your user folder.");
        let ids: Vec<&str> = r.buttons.iter().map(|b| b.id).collect();
        assert_eq!(ids, vec!["choose-folder"]);
    }

    #[test]
    fn the_new_tab_row_says_where_tabs_open_and_offers_its_buttons() {
        let prefs = PrefStore::memory();
        let look = LookStore::new(prefs.clone());
        let face: &dyn Fn(&str) -> String = &face_of_test;
        let mut e = env(&prefs, &look, face);
        e.home = "C:/Users/Admin".to_string();

        // Fresh: no folder chosen.
        let page = build_terminal(&e);
        let r = the_row(&page, "newtab-mode");
        assert_eq!(r.sub, "Opens in your user folder.");
        let ids: Vec<&str> = r.buttons.iter().map(|b| b.id).collect();
        assert_eq!(ids, vec!["choose-folder"]);
        assert_eq!(r.buttons[0].label, "Choose folder\u{2026}");
        assert_eq!(r.row_tip, "C:/Users/Admin");

        // A folder: both buttons, the chosen sub, the folder as the tip.
        set_new_tab_folder(&prefs, "D:/work");
        let page = build_terminal(&e);
        let r = the_row(&page, "newtab-mode");
        assert_eq!(r.sub, "Opens in the folder you chose.");
        let ids: Vec<&str> = r.buttons.iter().map(|b| b.id).collect();
        assert_eq!(ids, vec!["use-home", "choose-folder"]);
        assert_eq!(r.buttons[0].label, "Use my user folder");
        assert_eq!(r.row_tip, "D:/work");

        // No folder and no home known: no tip.
        set_new_tab_folder(&prefs, "");
        e.home = String::new();
        let page = build_terminal(&e);
        assert_eq!(the_row(&page, "newtab-mode").row_tip, "");
    }

    #[test]
    fn the_explorer_row_is_disabled_and_says_checking_while_busy() {
        let prefs = PrefStore::memory();
        let look = LookStore::new(prefs.clone());
        let face: &dyn Fn(&str) -> String = &face_of_test;
        let mut e = env(&prefs, &look, face);

        e.verb = VerbState::Busy;
        let r = the_row(&build_terminal(&e), "explorer-verb").clone();
        assert!(r.off);
        assert_eq!(r.sub, "Checking with Windows.");
        assert_eq!(r.control, Control::Switch { on: false });

        e.verb = VerbState::On;
        let r = the_row(&build_terminal(&e), "explorer-verb").clone();
        assert!(!r.off);
        assert_eq!(r.sub, "Open in Flare, on every folder.");
        assert_eq!(r.control, Control::Switch { on: true });

        e.verb = VerbState::Off;
        let r = the_row(&build_terminal(&e), "explorer-verb").clone();
        assert!(!r.off);
        assert_eq!(r.control, Control::Switch { on: false });
    }

    #[test]
    fn right_clicks_to_programs_is_off_by_default_beside_the_agent_keys() {
        let prefs = PrefStore::memory();
        let look = LookStore::new(prefs.clone());
        let face: &dyn Fn(&str) -> String = &face_of_test;
        let e = env(&prefs, &look, face);
        let page = build_terminal(&e);
        let r = the_row(&page, "right-click-reports");
        assert_eq!(r.control, Control::Switch { on: false });
        assert!(
            r.sub.split_whitespace().count() <= 8,
            "plain, at most eight words"
        );
        crate::term_prefs::set_right_click_reports(&prefs, true);
        let page = build_terminal(&e);
        assert_eq!(
            the_row(&page, "right-click-reports").control,
            Control::Switch { on: true }
        );
    }

    #[test]
    fn agent_hotkeys_and_keep_agents_open_are_on_until_switched_off() {
        let prefs = PrefStore::memory();
        let look = LookStore::new(prefs.clone());
        let face: &dyn Fn(&str) -> String = &face_of_test;
        let e = env(&prefs, &look, face);
        let page = build_terminal(&e);
        assert_eq!(
            the_row(&page, "agent-keys").control,
            Control::Switch { on: true }
        );
        assert_eq!(
            the_row(&page, "agent-guard").control,
            Control::Switch { on: true }
        );

        set_agent_keys(&prefs, false);
        set_agent_guard(&prefs, false);
        let page = build_terminal(&e);
        assert_eq!(
            the_row(&page, "agent-keys").control,
            Control::Switch { on: false }
        );
        assert_eq!(
            the_row(&page, "agent-guard").control,
            Control::Switch { on: false }
        );
    }

    #[test]
    fn the_guard_row_carries_the_exception_in_its_tooltip() {
        let prefs = PrefStore::memory();
        let look = LookStore::new(prefs.clone());
        let face: &dyn Fn(&str) -> String = &face_of_test;
        let e = env(&prefs, &look, face);
        let page = build_terminal(&e);
        assert_eq!(
            the_row(&page, "agent-guard").row_tip,
            "Ctrl+C and Ctrl+D never reach an agent. Ctrl+C still stops one while it works, and copies a selection."
        );
        assert_eq!(the_row(&page, "agent-keys").row_tip, "Ctrl+S, Ctrl+Tab");
        assert!(the_row(&page, "agent-keys").tap);
        assert!(the_row(&page, "agent-guard").tap);
    }

    #[test]
    fn fifteen_faces_each_in_its_own_face() {
        let prefs = PrefStore::memory();
        let look = LookStore::new(prefs.clone());
        let face: &dyn Fn(&str) -> String = &face_of_test;
        let e = env(&prefs, &look, face);
        let page = build_terminal(&e);
        match &the_row(&page, "term-font-family").control {
            Control::Select { value, options } => {
                assert_eq!(options.len(), 15);
                assert_eq!(value, "cascadia");
                for o in options {
                    assert_eq!(o.face, face_of_test(&o.id));
                }
                assert_eq!(options[0].name, "Cascadia Mono");
            }
            other => panic!("expected a select, got {other:?}"),
        }
    }

    #[test]
    fn sizes_run_50_to_200_in_tens() {
        let prefs = PrefStore::memory();
        let look = LookStore::new(prefs.clone());
        let face: &dyn Fn(&str) -> String = &face_of_test;
        let e = env(&prefs, &look, face);
        match &the_row(&build_terminal(&e), "term-font").control {
            Control::Select { value, options } => {
                assert_eq!(options.len(), 16);
                assert_eq!(options[0].name, "50%");
                assert_eq!(options[15].name, "200%");
                assert_eq!(options[5].name, "100%");
                assert_eq!(value, "100");
            }
            other => panic!("expected a select, got {other:?}"),
        }
        // An off-list stored size reads as 100 percent.
        prefs.set("flare.term.fontPct", "733");
        match &the_row(&build_terminal(&e), "term-font").control {
            Control::Select { value, .. } => assert_eq!(value, "100"),
            other => panic!("expected a select, got {other:?}"),
        }
    }

    #[test]
    fn command_help_is_on_by_default() {
        let prefs = PrefStore::memory();
        let look = LookStore::new(prefs.clone());
        let face: &dyn Fn(&str) -> String = &face_of_test;
        let e = env(&prefs, &look, face);
        assert_eq!(
            the_row(&build_terminal(&e), "help-enabled").control,
            Control::Switch { on: true }
        );
        set_help_enabled(&prefs, false);
        assert_eq!(
            the_row(&build_terminal(&e), "help-enabled").control,
            Control::Switch { on: false }
        );
    }
}
