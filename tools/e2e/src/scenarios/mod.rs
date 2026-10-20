//! The scenario table: one row per scenario of Appendix B, in the order the parent runs them. Task 1
//! writes the rows and one stub per scenario; each port task replaces its own stub body.
pub mod agent_hooks;
pub mod agent_launch;
pub mod agent_misc;
pub mod anywhere;
pub mod chrome;
pub mod closeask;
pub mod diag;
pub mod dictation;
pub mod dpi;
mod driver;
pub mod help;
pub mod indicators;
pub mod input_a;
pub mod input_b;
pub mod input_c;
pub mod input_d;
pub mod lifecycle;
mod marks_px;
pub mod settings_a;
pub mod settings_b;
pub mod settings_c;
pub mod settings_d;
pub mod strip;
pub mod update;
pub mod vertical;
pub mod visual;

use crate::ctx::Scenario;

/// Every scenario the harness knows: the 70 ported ones, hookSmoke, and the two DPI scenarios.
pub const ALL: &[Scenario] = &[
    Scenario {
        name: "spawn",
        group: "lifecycle",
        run: lifecycle::spawn,
        limit_secs: 180,
    },
    Scenario {
        name: "hookSmoke",
        group: "lifecycle",
        run: lifecycle::hook_smoke,
        limit_secs: 180,
    },
    Scenario {
        name: "cwdLabel",
        group: "lifecycle",
        run: lifecycle::cwd_label,
        limit_secs: 180,
    },
    Scenario {
        name: "indicator",
        group: "indicators",
        run: indicators::indicator,
        limit_secs: 180,
    },
    Scenario {
        name: "attention",
        group: "indicators",
        run: indicators::attention,
        limit_secs: 180,
    },
    Scenario {
        name: "agentHooks",
        group: "agent_hooks",
        run: agent_hooks::agent_hooks,
        limit_secs: 180,
    },
    Scenario {
        name: "agentDiag",
        group: "agent_misc",
        run: agent_misc::agent_diag,
        limit_secs: 180,
    },
    Scenario {
        name: "theme",
        group: "chrome",
        run: chrome::theme,
        limit_secs: 180,
    },
    Scenario {
        name: "newTabFolder",
        group: "chrome",
        run: chrome::new_tab_folder,
        limit_secs: 180,
    },
    Scenario {
        name: "caretHold",
        group: "input_a",
        run: input_a::caret_hold,
        limit_secs: 180,
    },
    Scenario {
        name: "mouseReport",
        group: "input_a",
        run: input_a::mouse_report,
        limit_secs: 180,
    },
    Scenario {
        name: "clickCaret",
        group: "input_a",
        run: input_a::click_caret,
        limit_secs: 180,
    },
    Scenario {
        name: "selectionEdit",
        group: "input_b",
        run: input_b::selection_edit,
        limit_secs: 180,
    },
    Scenario {
        name: "findKey",
        group: "input_a",
        run: input_a::find_key,
        limit_secs: 180,
    },
    Scenario {
        name: "scrollbar",
        group: "input_a",
        run: input_a::scrollbar,
        limit_secs: 180,
    },
    Scenario {
        name: "paste",
        group: "input_a",
        run: input_a::paste,
        limit_secs: 180,
    },
    Scenario {
        name: "tabDragFirst",
        group: "strip",
        run: strip::tab_drag_first,
        limit_secs: 180,
    },
    Scenario {
        name: "tabWidth",
        group: "strip",
        run: strip::tab_width,
        limit_secs: 180,
    },
    Scenario {
        name: "tabLooks",
        group: "strip",
        run: strip::tab_looks,
        limit_secs: 180,
    },
    Scenario {
        name: "tabSwitch",
        group: "strip",
        run: strip::tab_switch,
        limit_secs: 180,
    },
    Scenario {
        name: "indicatorStyles",
        group: "indicators",
        run: indicators::indicator_styles,
        limit_secs: 360,
    },
    Scenario {
        name: "verticalTabs",
        group: "vertical",
        run: vertical::vertical_tabs,
        limit_secs: 360,
    },
    Scenario {
        name: "windowColoursSave",
        group: "settings_b",
        run: settings_b::window_colours_save,
        limit_secs: 180,
    },
    Scenario {
        name: "tooltips",
        group: "strip",
        run: strip::tooltips,
        limit_secs: 180,
    },
    Scenario {
        name: "manyTabs",
        group: "strip",
        run: strip::many_tabs,
        limit_secs: 180,
    },
    Scenario {
        name: "backgroundWork",
        group: "agent_misc",
        run: agent_misc::background_work,
        limit_secs: 180,
    },
    Scenario {
        name: "systemMenu",
        group: "chrome",
        run: chrome::system_menu,
        limit_secs: 180,
    },
    Scenario {
        name: "agentKeys",
        group: "agent_misc",
        run: agent_misc::agent_keys,
        limit_secs: 180,
    },
    Scenario {
        name: "cogToggle",
        group: "chrome",
        run: chrome::cog_toggle,
        limit_secs: 180,
    },
    Scenario {
        name: "accentAsk",
        group: "settings_b",
        run: settings_b::accent_ask,
        limit_secs: 180,
    },
    Scenario {
        name: "titleBar",
        group: "chrome",
        run: chrome::title_bar,
        limit_secs: 180,
    },
    Scenario {
        name: "settingsPage",
        group: "chrome",
        run: chrome::settings_page,
        limit_secs: 180,
    },
    Scenario {
        name: "launchSkeleton",
        group: "agent_launch",
        run: agent_launch::launch_skeleton,
        limit_secs: 180,
    },
    Scenario {
        name: "restore",
        group: "lifecycle",
        run: lifecycle::restore,
        limit_secs: 180,
    },
    Scenario {
        name: "handoff",
        group: "lifecycle",
        run: lifecycle::handoff,
        limit_secs: 180,
    },
    Scenario {
        name: "quitManyShells",
        group: "lifecycle",
        run: lifecycle::quit_many_shells,
        limit_secs: 180,
    },
    Scenario {
        name: "lastTab",
        group: "lifecycle",
        run: lifecycle::last_tab,
        limit_secs: 180,
    },
    Scenario {
        name: "closeAsk",
        group: "closeask",
        run: closeask::close_ask,
        limit_secs: 180,
    },
    Scenario {
        name: "pathLinks",
        group: "input_b",
        run: input_b::path_links,
        limit_secs: 180,
    },
    Scenario {
        name: "linkPaintStall",
        group: "input_d",
        run: input_d::link_paint_stall,
        limit_secs: 300,
    },
    Scenario {
        name: "sonaraQuestion",
        group: "agent_launch",
        run: agent_launch::sonara_question,
        limit_secs: 180,
    },
    Scenario {
        name: "termReplies",
        group: "input_c",
        run: input_c::term_replies,
        limit_secs: 180,
    },
    Scenario {
        name: "termClipboard",
        group: "input_c",
        run: input_c::term_clipboard,
        limit_secs: 180,
    },
    Scenario {
        name: "imagePaste",
        group: "agent_launch",
        run: agent_launch::image_paste,
        limit_secs: 180,
    },
    Scenario {
        name: "bell",
        group: "input_c",
        run: input_c::bell,
        limit_secs: 180,
    },
    Scenario {
        name: "hiddenResize",
        group: "input_c",
        run: input_c::hidden_resize,
        limit_secs: 180,
    },
    Scenario {
        name: "links",
        group: "input_b",
        run: input_b::links,
        limit_secs: 180,
    },
    Scenario {
        name: "osc8Links",
        group: "input_b",
        run: input_b::osc8_links,
        limit_secs: 180,
    },
    Scenario {
        name: "reviewKeys",
        group: "input_d",
        run: input_d::review_keys,
        limit_secs: 180,
    },
    Scenario {
        name: "dropAndMenu",
        group: "input_d",
        run: input_d::drop_and_menu,
        limit_secs: 180,
    },
    Scenario {
        name: "helpPanel",
        group: "help",
        run: help::help_panel,
        limit_secs: 300,
    },
    Scenario {
        name: "options",
        group: "settings_a",
        run: settings_a::options,
        limit_secs: 180,
    },
    Scenario {
        name: "diagLog",
        group: "diag",
        run: diag::diag_log,
        limit_secs: 180,
    },
    Scenario {
        name: "settingsLook",
        group: "settings_a",
        run: settings_a::settings_look,
        limit_secs: 180,
    },
    Scenario {
        name: "settingsSearch",
        group: "settings_a",
        run: settings_a::settings_search,
        limit_secs: 180,
    },
    Scenario {
        name: "settingsKeys",
        group: "settings_a",
        run: settings_a::settings_keys,
        limit_secs: 180,
    },
    Scenario {
        name: "themeEditorLive",
        group: "settings_b",
        run: settings_b::theme_editor_live,
        limit_secs: 180,
    },
    Scenario {
        name: "themeSwitch",
        group: "settings_b",
        run: settings_b::theme_switch,
        limit_secs: 180,
    },
    Scenario {
        name: "reviewSettings",
        group: "settings_b",
        run: settings_b::review_settings,
        limit_secs: 180,
    },
    Scenario {
        name: "colourPicker",
        group: "settings_c",
        run: settings_c::colour_picker,
        limit_secs: 180,
    },
    Scenario {
        name: "pickedGround",
        group: "settings_c",
        run: settings_c::picked_ground,
        limit_secs: 180,
    },
    Scenario {
        name: "opacityAlpha",
        group: "settings_c",
        run: settings_c::opacity_alpha,
        limit_secs: 180,
    },
    Scenario {
        name: "seeThrough",
        group: "settings_c",
        run: settings_c::see_through,
        limit_secs: 180,
    },
    Scenario {
        name: "themeCards",
        group: "settings_d",
        run: settings_d::theme_cards,
        limit_secs: 180,
    },
    Scenario {
        name: "edges",
        group: "settings_d",
        run: settings_d::edges,
        limit_secs: 180,
    },
    Scenario {
        name: "accent",
        group: "settings_d",
        run: settings_d::accent,
        limit_secs: 180,
    },
    Scenario {
        name: "updateWindow",
        group: "update",
        run: update::update_window,
        limit_secs: 300,
    },
    Scenario {
        name: "updateGuard",
        group: "update",
        run: update::update_guard,
        limit_secs: 180,
    },
    Scenario {
        name: "updateNotes",
        group: "update",
        run: update::update_notes,
        limit_secs: 180,
    },
    Scenario {
        name: "dictation",
        group: "dictation",
        run: dictation::dictation,
        limit_secs: 240,
    },
    Scenario {
        name: "dictationPage",
        group: "dictation",
        run: dictation::dictation_page,
        limit_secs: 240,
    },
    Scenario {
        name: "dictationParakeet",
        group: "dictation",
        run: dictation::dictation_parakeet,
        limit_secs: 240,
    },
    Scenario {
        name: "exitClosesTab",
        group: "lifecycle",
        run: lifecycle::exit_closes_tab,
        limit_secs: 180,
    },
    Scenario {
        name: "promptLayout",
        group: "lifecycle",
        run: lifecycle::prompt_layout,
        limit_secs: 180,
    },
    Scenario {
        name: "dictateAnywhere",
        group: "anywhere",
        run: anywhere::dictate_anywhere,
        limit_secs: 240,
    },
    Scenario {
        name: "dictateTwoCopies",
        group: "anywhere",
        run: anywhere::dictate_two_copies,
        limit_secs: 240,
    },
    Scenario {
        name: "dpiMatrix",
        group: "dpi",
        run: dpi::dpi_matrix,
        limit_secs: 300,
    },
    Scenario {
        name: "monitorMove",
        group: "dpi",
        run: dpi::monitor_move,
        limit_secs: 300,
    },
];

/// Scenarios that are NOT part of the gate: a plain run never picks them, a filter that names one does.
/// isualTour is the visual comparison capture (audit 2026-10-20); it takes half an hour and writes pictures.
pub const OPT_IN: &[Scenario] = &[Scenario {
    name: "visualTour",
    group: "visual",
    run: visual::visual_tour,
    limit_secs: 3_300,
}];
