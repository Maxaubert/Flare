//! The Agents page model (M7 Task 22).
//!
//! Three sections in order: the tab marks (the indicator, the finished, rainbow, question and
//! failed switches, then the taskbar count), Claude Code (the exact status switch), and the mark
//! colours (three colour rows under one Save changes heading action).

use super::{
    BlockModel, Control, HeadingAction, PageEnv, PageModel, RowModel, SectionModel, opts,
    plain_row, section, switch_row,
};
use crate::agent_prefs::{
    agent_done_on, agent_failed_on, agent_hooks_on, agent_question_on, agent_rainbow,
    notify_ask_on, taskbar_badge_on,
};
use crate::look::{AgentColourState, AgentIndicator};
use crate::rows::{RowDef, row};
use flare_theme::colour_contrast::AlphaRange;

/// The heading's Save words, as the Appearance page's Save (`saveSetupTitle` in PT).
const SAVE_TITLE: &str = "Saves the theme, agent colours and see-through window as Custom";
const SAVE_CLEAN: &str = "Nothing to save yet";

/// The subtext of each indicator choice (PT's `INDICATOR_SUB`, Ring and Bar).
const RING_SUB: &str = "One small mark at the end of the tab, in its ink.";
const BAR_SUB: &str = "A line under the tab while it works.";

/// A mark colour that is picked (PT's `OWN`).
const OWN_COLOUR: &str = "Your own colour.";

/// The Agents page id.
const PAGE_ID: &str = "agents";

/// The Agents page: marks, Claude Code and the mark colours.
pub fn build(env: &PageEnv<'_>) -> PageModel {
    PageModel {
        id: PAGE_ID,
        label: "Agents",
        sections: vec![
            marks_section(env),
            claude_section(env),
            colours_section(env),
            notify_section(env),
        ],
    }
}

/// The table entry of a row this page draws. Every id passed here is in the tables.
fn def(id: &str) -> &'static RowDef {
    row(id).expect("the Agents page draws only rows of the tables")
}

fn marks_section(env: &PageEnv<'_>) -> SectionModel {
    let (value, sub) = match env.look.agent_indicator() {
        AgentIndicator::Minimal => ("minimal", BAR_SUB),
        _ => ("ring", RING_SUB),
    };
    let indicator = RowModel {
        sub: sub.to_string(),
        ..plain_row(
            def("agent-indicator"),
            Control::Segmented {
                value: value.to_string(),
                options: opts(&[("ring", "Ring"), ("minimal", "Bar")]),
            },
        )
    };
    let blocks = vec![
        BlockModel::Row(indicator),
        BlockModel::Row(switch_row(def("agent-done-on"), agent_done_on(env.prefs))),
        BlockModel::Row(switch_row(def("agent-rainbow"), agent_rainbow(env.prefs))),
        BlockModel::Row(switch_row(
            def("agent-question-on"),
            agent_question_on(env.prefs),
        )),
        BlockModel::Row(switch_row(
            def("agent-failed-on"),
            agent_failed_on(env.prefs),
        )),
        BlockModel::Row(switch_row(
            def("taskbar-badge"),
            taskbar_badge_on(env.prefs),
        )),
    ];
    section("marks", blocks)
}

fn claude_section(env: &PageEnv<'_>) -> SectionModel {
    section(
        "claude",
        vec![BlockModel::Row(switch_row(
            def("agent-hooks"),
            agent_hooks_on(env.prefs),
        ))],
    )
}

/// The Notifications section (Surfaces stage 7): the switch for the Windows notification of a question.
fn notify_section(env: &PageEnv<'_>) -> SectionModel {
    section(
        "notify",
        vec![BlockModel::Row(switch_row(
            def("notify-ask"),
            notify_ask_on(env.prefs),
        ))],
    )
}

fn colours_section(env: &PageEnv<'_>) -> SectionModel {
    let dirty = env.look.setup_state().dirty;
    let blocks = vec![
        BlockModel::Row(colour_row(env, "agent-color", AgentColourState::Working, 0)),
        BlockModel::Row(colour_row(
            env,
            "agent-done-color",
            AgentColourState::Finished,
            1,
        )),
        BlockModel::Row(colour_row(
            env,
            "agent-question-color",
            AgentColourState::Question,
            2,
        )),
    ];
    SectionModel {
        action: Some(HeadingAction {
            dirty,
            title: (if dirty { SAVE_TITLE } else { SAVE_CLEAN }).to_string(),
        }),
        ..section("colours", blocks)
    }
}

/// One mark's colour: the colour in force, and Reset while one is picked. The colour follows
/// the theme until it is picked; `index` is the position in `env.agent_colours`.
fn colour_row(env: &PageEnv<'_>, id: &str, state: AgentColourState, index: usize) -> RowModel {
    let def = def(id);
    let picked = !env.look.agent_colour_choice(state).is_empty();
    let sub = if picked { OWN_COLOUR } else { def.sub };
    RowModel {
        sub: sub.to_string(),
        reset: picked,
        ..plain_row(
            def,
            Control::Colour {
                value: env.agent_colours[index].clone(),
                range: AlphaRange::default(),
                alpha_disabled: false,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kv::PrefStore;
    use crate::look::LookStore;
    use crate::page::{DiagInfo, GlassSupport, ThemeColours, all_row_ids, row_of};
    use crate::verb::VerbState;

    fn face(_: &str) -> String {
        String::new()
    }

    /// A page environment over the given stores; the agent colours in force are `colours`.
    fn with_env<R>(
        prefs: &PrefStore,
        look: &LookStore,
        colours: [&str; 3],
        f: impl FnOnce(&PageEnv<'_>) -> R,
    ) -> R {
        let face_of: &dyn Fn(&str) -> String = &face;
        let env = PageEnv {
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
            agent_colours: colours.map(str::to_string),
            agent_key_labels: String::new(),
            face_of,
        };
        f(&env)
    }

    fn fresh<R>(f: impl FnOnce(&PageEnv<'_>) -> R) -> R {
        let prefs = PrefStore::memory();
        let look = LookStore::new(PrefStore::memory());
        with_env(&prefs, &look, ["#ff0000", "#00ff00", "#0000ff"], f)
    }

    /// The row with this id on the built page.
    fn the_row(page: &PageModel, id: &str) -> RowModel {
        row_of(page, id)
            .unwrap_or_else(|| panic!("row {id} is on the page"))
            .clone()
    }

    fn section_of<'p>(page: &'p PageModel, id: &str) -> &'p SectionModel {
        page.sections
            .iter()
            .find(|s| s.id == id)
            .unwrap_or_else(|| panic!("section {id} is on the page"))
    }

    #[test]
    fn the_marks_rows_are_in_order_with_the_rainbow_after_finished_and_the_badge_last() {
        fresh(|env| {
            let page = build(env);
            let ids: Vec<&str> = page.sections.iter().map(|s| s.id).collect();
            assert_eq!(ids, ["marks", "claude", "colours", "notify"]);
            assert_eq!(
                all_row_ids(&page),
                [
                    "agent-indicator",
                    "agent-done-on",
                    "agent-rainbow",
                    "agent-question-on",
                    "agent-failed-on",
                    "taskbar-badge",
                    "agent-hooks",
                    "agent-color",
                    "agent-done-color",
                    "agent-question-color",
                    "notify-ask",
                ]
            );
        });
    }

    #[test]
    fn the_indicator_offers_ring_and_bar_and_stores_bar_as_minimal() {
        let prefs = PrefStore::memory();
        let look = LookStore::new(PrefStore::memory());
        with_env(&prefs, &look, ["", "", ""], |env| {
            let page = build(env);
            let row = the_row(&page, "agent-indicator");
            assert_eq!(
                row.control,
                Control::Segmented {
                    value: "ring".to_string(),
                    options: opts(&[("ring", "Ring"), ("minimal", "Bar")]),
                }
            );
        });
        look.set_agent_indicator(AgentIndicator::Minimal);
        with_env(&prefs, &look, ["", "", ""], |env| {
            let page = build(env);
            match the_row(&page, "agent-indicator").control {
                Control::Segmented { value, .. } => assert_eq!(value, "minimal"),
                other => panic!("the indicator is a segmented control, not {other:?}"),
            }
        });
    }

    #[test]
    fn each_indicator_has_its_own_subtext() {
        let prefs = PrefStore::memory();
        let look = LookStore::new(PrefStore::memory());
        with_env(&prefs, &look, ["", "", ""], |env| {
            let page = build(env);
            assert_eq!(
                the_row(&page, "agent-indicator").sub,
                "One small mark at the end of the tab, in its ink."
            );
        });
        look.set_agent_indicator(AgentIndicator::Minimal);
        with_env(&prefs, &look, ["", "", ""], |env| {
            let page = build(env);
            assert_eq!(
                the_row(&page, "agent-indicator").sub,
                "A line under the tab while it works."
            );
        });
    }

    #[test]
    fn every_switch_reads_on_unless_stored_zero() {
        const SWITCHES: [&str; 7] = [
            "agent-done-on",
            "agent-rainbow",
            "agent-question-on",
            "agent-failed-on",
            "agent-hooks",
            "taskbar-badge",
            "notify-ask",
        ];
        for id in SWITCHES {
            let key = def(id).keys[0];
            let prefs = PrefStore::memory();
            let look = LookStore::new(PrefStore::memory());

            with_env(&prefs, &look, ["", "", ""], |env| {
                let page = build(env);
                assert_eq!(
                    the_row(&page, id).control,
                    Control::Switch { on: true },
                    "{id} is on when fresh"
                );
            });

            prefs.set(key, "0");
            with_env(&prefs, &look, ["", "", ""], |env| {
                let page = build(env);
                assert_eq!(
                    the_row(&page, id).control,
                    Control::Switch { on: false },
                    "{id} is off when stored 0"
                );
            });

            prefs.set(key, "x");
            with_env(&prefs, &look, ["", "", ""], |env| {
                let page = build(env);
                assert_eq!(
                    the_row(&page, id).control,
                    Control::Switch { on: true },
                    "{id} reads on for any value other than 0"
                );
            });
        }
    }

    #[test]
    fn exact_status_from_claude_code_is_its_own_section() {
        fresh(|env| {
            let page = build(env);
            let claude = section_of(&page, "claude");
            assert_eq!(claude.title, "Claude Code");
            assert_eq!(claude.blocks.len(), 1);
            let row = the_row(&page, "agent-hooks");
            assert_eq!(row.label, def("agent-hooks").label);
            assert!(row.tap, "a press on the row flips the switch");
        });
    }

    #[test]
    fn the_mark_colours_save_lights_with_the_setup_but_not_with_a_window_colour_alone() {
        // One store under both, as the app has it: the window colour and the look share it.
        let prefs = PrefStore::memory();
        let look = LookStore::new(prefs.clone());
        with_env(&prefs, &look, ["", "", ""], |env| {
            let action = section_of(&build(env), "colours")
                .action
                .clone()
                .expect("the Mark colours heading has a Save");
            assert!(!action.dirty);
            assert_eq!(action.title, SAVE_CLEAN);
        });

        // A picked window Background is not a setup change: the heading stays clean.
        prefs.set("flare.window.background", "#101010");
        with_env(&prefs, &look, ["", "", ""], |env| {
            let action = section_of(&build(env), "colours")
                .action
                .clone()
                .expect("a heading action");
            assert!(
                !action.dirty,
                "a window colour alone does not light this Save"
            );
        });

        // A picked mark colour is a setup change: the heading lights.
        look.set_agent_colour(AgentColourState::Working, "#22c55e");
        with_env(&prefs, &look, ["", "", ""], |env| {
            let action = section_of(&build(env), "colours")
                .action
                .clone()
                .expect("a heading action");
            assert!(action.dirty);
            assert_eq!(action.title, SAVE_TITLE);
        });
    }

    #[test]
    fn a_mark_colour_follows_the_theme_until_picked() {
        let prefs = PrefStore::memory();
        let look = LookStore::new(PrefStore::memory());
        with_env(&prefs, &look, ["", "", ""], |env| {
            let page = build(env);
            let row = the_row(&page, "agent-color");
            assert!(!row.reset, "no Reset before a colour is picked");
            assert_eq!(row.sub, "Follows the accent.");
        });

        look.set_agent_colour(AgentColourState::Working, "#22c55e");
        with_env(&prefs, &look, ["#22c55e", "", ""], |env| {
            let page = build(env);
            let row = the_row(&page, "agent-color");
            assert!(row.reset, "Reset shows once a colour is picked");
            assert_eq!(row.sub, "Your own colour.");
        });
    }

    #[test]
    fn the_three_colour_rows_say_what_they_follow_and_a_picked_one_says_it_is_yours() {
        let prefs = PrefStore::memory();
        let look = LookStore::new(PrefStore::memory());
        let rows = [
            ("agent-color", "Agent working colour", "Follows the accent."),
            (
                "agent-done-color",
                "Agent finished colour",
                "Follows the theme green.",
            ),
            (
                "agent-question-color",
                "Agent question colour",
                "Follows the theme.",
            ),
        ];
        with_env(&prefs, &look, ["", "", ""], |env| {
            let page = build(env);
            for (id, label, sub) in rows {
                let row = the_row(&page, id);
                assert_eq!(row.label, label);
                assert_eq!(row.sub, sub, "{id} follows");
                assert!(!row.reset, "{id}: no Reset before a pick");
            }
        });
        look.set_agent_colour(AgentColourState::Working, "#111111");
        look.set_agent_colour(AgentColourState::Finished, "#222222");
        look.set_agent_colour(AgentColourState::Question, "#333333");
        with_env(&prefs, &look, ["#111111", "#222222", "#333333"], |env| {
            let page = build(env);
            for (id, _, _) in rows {
                let row = the_row(&page, id);
                assert_eq!(row.sub, "Your own colour.", "{id} is picked");
                assert!(row.reset, "{id}: Reset once picked");
            }
        });
    }

    #[test]
    fn the_swatch_shows_the_colour_in_force() {
        let prefs = PrefStore::memory();
        let look = LookStore::new(PrefStore::memory());
        with_env(&prefs, &look, ["#111111", "#222222", "#333333"], |env| {
            let page = build(env);
            let expected = [
                ("agent-color", "#111111"),
                ("agent-done-color", "#222222"),
                ("agent-question-color", "#333333"),
            ];
            for (id, colour) in expected {
                match the_row(&page, id).control {
                    Control::Colour { value, range, .. } => {
                        assert_eq!(value, colour, "{id} shows the colour in force");
                        assert_eq!(range, AlphaRange::default(), "full alpha range");
                    }
                    other => panic!("{id} is a colour control, not {other:?}"),
                }
            }
        });
    }
}
