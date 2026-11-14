//! Every Settings control event becomes store writes and effects (M7 Task 25).
//! Source: PT `AppearancePage.tsx`, `TerminalPage.tsx`, `AgentsPage.tsx` (every `onChange`), the core
//! sections, `ThemeWall.tsx` (`land`, `pick`, the switch question, the editor's save) and `useTermSetup`.
//!
//! One writer per key: every write goes through the setter of the module that owns the key. This file
//! defines none.

use std::path::PathBuf;

use flare_theme::ansi::ANSI_KEYS;
use flare_theme::colour::{canonical_stored, opaque, parse_colour, to_stored};

use crate::agent_prefs;
use crate::custom_theme::{palette_of, with_ground_alpha};
use crate::help_prefs;
use crate::kv::PrefStore;
use crate::look::{AgentColourState, AgentIndicator, LookStore};
use crate::new_tab;
use crate::page::appearance;
use crate::tab_layout;
use crate::tab_switch;
use crate::term_prefs;
use crate::verb::DiagControlApi;
use crate::window_colours::{
    ACCENT, BACKGROUND, ColourPref, SECONDARY, read_window_colours, set_custom_window_colours,
    set_window_edges_word,
};

/// The most see-through the slider stores.
const SEE_THROUGH_TOP: u32 = 70;

/// What the user did to a control.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Switch { row: String, on: bool },
    Choose { row: String, option: String },
    Slide { row: String, value: u32 },
    Button { row: String, button: String },
    Reset { row: String },
    ColourCommit { row: String, stored: String },
    ColourRevert { row: String, to: Option<String> },
    SaveChanges { from: SaveFrom },
    WallPick { id: String },
    WallEdit { id: String },
    EditorSet { key: String, value: Option<String> },
    EditorSave,
    EditorCancel,
    Ask { answer: AskAnswer },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveFrom {
    Theme,
    MarkColours,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AskAnswer {
    SaveAsCustom,
    Discard,
    Cancel,
}

/// What the host must do beyond the store writes.
#[derive(Debug, PartialEq)]
pub enum Effect {
    AskThemeSwitch { to: String },
    PickFolder,
    OpenDiagFolder,
    SetVerb(bool),
    Marked,
    Rebuild,
}

pub struct Ctx<'a> {
    pub prefs: &'a PrefStore,
    pub look: &'a LookStore,
    pub diag: &'a dyn DiagControlApi,
    pub pending_pick: &'a std::cell::RefCell<Option<String>>,
}

fn window_pref(row: &str) -> Option<&'static ColourPref> {
    match row {
        "window-background" => Some(&BACKGROUND),
        "window-secondary" => Some(&SECONDARY),
        "window-accent" => Some(&ACCENT),
        _ => None,
    }
}

fn agent_state(row: &str) -> Option<AgentColourState> {
    match row {
        "agent-color" => Some(AgentColourState::Working),
        "agent-done-color" => Some(AgentColourState::Finished),
        "agent-question-color" => Some(AgentColourState::Question),
        _ => None,
    }
}

/// Applies one event. Returns what the host must still do.
pub fn apply(ctx: &Ctx, ev: Event) -> Vec<Effect> {
    match ev {
        Event::Switch { row, on } => switch(ctx, &row, on),
        Event::Choose { row, option } => choose(ctx, &row, &option),
        Event::Slide { row, value } => {
            slide(ctx, &row, value);
            Vec::new()
        }
        Event::Button { row, button } => button_pressed(ctx, &row, &button),
        Event::Reset { row } => {
            colour_write(ctx, &row, None);
            Vec::new()
        }
        Event::ColourCommit { row, stored } => {
            colour_write(ctx, &row, Some(&stored));
            Vec::new()
        }
        Event::ColourRevert { row, to } => {
            colour_write(ctx, &row, to.as_deref());
            Vec::new()
        }
        Event::SaveChanges { from } => {
            match from {
                SaveFrom::Theme => save_theme(ctx),
                SaveFrom::MarkColours => ctx.look.save_term_setup(),
            }
            Vec::new()
        }
        Event::WallPick { id } => wall_pick(ctx, id),
        Event::WallEdit { id } => {
            let custom = ctx.look.custom();
            let palette = palette_of(&id, custom.as_ref(), &ctx.look.theme_id());
            ctx.look.set_draft(Some(with_ground_alpha(&palette)));
            Vec::new()
        }
        Event::EditorSet { key, value } => {
            editor_set(ctx, &key, value.as_deref());
            Vec::new()
        }
        Event::EditorSave => {
            if let Some(draft) = ctx.look.draft() {
                ctx.look.save_as_custom(&draft);
                ctx.look.set_draft(None);
                ctx.look.land_theme("custom", true);
            }
            Vec::new()
        }
        Event::EditorCancel => {
            ctx.look.set_draft(None);
            Vec::new()
        }
        Event::Ask { answer } => {
            let pending = ctx.pending_pick.borrow_mut().take();
            match (answer, pending) {
                (AskAnswer::SaveAsCustom, Some(id)) => {
                    // Switching TO the saved Custom: saving the theme in force would write ITS palette over
                    // the Custom and the switch would do nothing. Keep the Custom's own palette and save
                    // the setup in force beside it (review #72).
                    match (id == "custom").then(|| ctx.look.custom()).flatten() {
                        Some(c) => {
                            ctx.look.save_as_custom(&c.palette);
                            set_custom_window_colours(
                                ctx.prefs,
                                Some(&read_window_colours(ctx.prefs)),
                            );
                        }
                        None => save_theme(ctx),
                    }
                    ctx.look.land_theme(&id, false);
                }
                (AskAnswer::Discard, Some(id)) => ctx.look.land_theme(&id, false),
                _ => {}
            }
            Vec::new()
        }
    }
}

/// A folder the picker returned: the new-tab folder. Nothing chosen writes nothing.
pub fn folder_picked(prefs: &PrefStore, dir: Option<PathBuf>) {
    if let Some(dir) = dir {
        new_tab::set_new_tab_folder(prefs, &dir.to_string_lossy());
    }
}

fn switch(ctx: &Ctx, row: &str, on: bool) -> Vec<Effect> {
    let p = ctx.prefs;
    match row {
        "explorer-verb" => return vec![Effect::SetVerb(on)],
        "diag-verbose" => {
            ctx.diag.set_verbose(on);
            return vec![Effect::Rebuild];
        }
        "agent-done-on" => agent_prefs::set_agent_done_on(p, on),
        "agent-question-on" => agent_prefs::set_agent_question_on(p, on),
        "agent-failed-on" => agent_prefs::set_agent_failed_on(p, on),
        "agent-rainbow" => agent_prefs::set_agent_rainbow(p, on),
        "agent-hooks" => agent_prefs::set_agent_hooks_on(p, on),
        "taskbar-badge" => agent_prefs::set_taskbar_badge_on(p, on),
        "agent-guard" => agent_prefs::set_agent_guard(p, on),
        "agent-keys" => term_prefs::set_agent_keys(p, on),
        "right-click-reports" => term_prefs::set_right_click_reports(p, on),
        "open-previews" => term_prefs::set_open_previews(p, on),
        "wrap-previews" => term_prefs::set_wrap_previews(p, on),
        "hover-thumbs" => term_prefs::set_hover_thumbs(p, on),
        "agent-eval" => term_prefs::set_agent_eval(p, on),
        "app-network" => term_prefs::set_app_network(p, on),
        "dev-preview" => term_prefs::set_dev_preview(p, on),
        "notify-ask" => agent_prefs::set_notify_ask_on(p, on),
        "help-enabled" => help_prefs::set_help_enabled(p, on),
        "term-caret-blink" => ctx.look.set_caret_blink(on),
        _ => {}
    }
    Vec::new()
}

fn choose(ctx: &Ctx, row: &str, option: &str) -> Vec<Effect> {
    let p = ctx.prefs;
    match row {
        "tab-layout" => tab_layout::set_tab_layout_word(p, option),
        "tab-switch" => tab_switch::set_tab_switch_word(p, option),
        "window-edges" => set_window_edges_word(p, option),
        "app-icon" => crate::app_icon::set_app_icon_word(p, option),
        "term-shell" => term_prefs::save_shell_id(p, option),
        "term-font-family" => ctx.look.set_font_id(option),
        "term-caret" => ctx.look.set_caret_shape(option),
        "term-font" => {
            if let Ok(pct) = option.trim().parse::<u32>() {
                ctx.look.set_font_pct(pct);
            }
        }
        "agent-indicator" => {
            let v = match option {
                "off" => AgentIndicator::Off,
                "minimal" => AgentIndicator::Minimal,
                "full" => AgentIndicator::Full,
                _ => AgentIndicator::Ring,
            };
            ctx.look.set_agent_indicator(v);
        }
        _ => {}
    }
    Vec::new()
}

fn slide(ctx: &Ctx, row: &str, value: u32) {
    match row {
        "term-acrylic" => ctx.look.set_see_through(value.min(SEE_THROUGH_TOP) as u8),
        "term-font" => ctx.look.set_font_pct(value),
        _ => {}
    }
}

fn button_pressed(ctx: &Ctx, row: &str, button: &str) -> Vec<Effect> {
    match (row, button) {
        ("newtab-mode", "use-home") => {
            new_tab::set_new_tab_folder(ctx.prefs, "");
            Vec::new()
        }
        ("newtab-mode", "choose-folder") => vec![Effect::PickFolder],
        ("diag-folder", "open") => vec![Effect::OpenDiagFolder],
        ("diag-mark", "mark") => {
            ctx.diag.mark();
            vec![Effect::Marked]
        }
        _ => Vec::new(),
    }
}

/// A colour row's write: `Some(colour)` stores it, `None` clears it (Reset, or a revert to nothing).
/// Junk and a write that changes nothing are skipped, so nobody hears about them.
fn colour_write(ctx: &Ctx, row: &str, stored: Option<&str>) {
    let wanted = stored.and_then(|s| canonical_stored(Some(s)));
    if stored.is_some() && wanted.is_none() {
        return;
    }
    if let Some(pref) = window_pref(row) {
        let wanted = if row == "window-background" {
            wanted.map(|c| opaque(&c, &c))
        } else {
            wanted
        };
        if wanted == pref.get(ctx.prefs) {
            return;
        }
        pref.set(ctx.prefs, wanted.as_deref());
    } else if let Some(state) = agent_state(row) {
        if wanted.clone().unwrap_or_default() == ctx.look.agent_colour_choice(state) {
            return;
        }
        ctx.look
            .set_agent_colour(state, wanted.as_deref().unwrap_or(""));
    }
}

/// Save changes from the theme section: the setup as Custom, with the window colours Custom keeps.
fn save_theme(ctx: &Ctx) {
    ctx.look.save_term_setup();
    set_custom_window_colours(ctx.prefs, Some(&read_window_colours(ctx.prefs)));
}

fn wall_pick(ctx: &Ctx, id: String) -> Vec<Effect> {
    if ctx.look.pick_needs_ask() || appearance::extra_dirty(ctx.prefs, ctx.look) {
        *ctx.pending_pick.borrow_mut() = Some(id.clone());
        return vec![Effect::AskThemeSwitch { to: id }];
    }
    ctx.look.land_theme(&id, false);
    Vec::new()
}

fn editor_set(ctx: &Ctx, key: &str, value: Option<&str>) {
    // Read at call time: two picks in one frame must not drop the first.
    let Some(mut draft) = ctx.look.draft() else {
        return;
    };
    let colour = match value {
        Some(v) => match parse_colour(v) {
            Some(c) => Some(to_stored(&c)),
            None => return,
        },
        None => None,
    };
    match key {
        "bg" => draft.bg = colour.unwrap_or(draft.bg),
        "fg" => draft.fg = colour.unwrap_or(draft.fg),
        "cursor" => draft.cursor = colour.unwrap_or(draft.cursor),
        "selection" => draft.selection = colour,
        k => {
            let Some(i) = ANSI_KEYS.iter().position(|a| *a == k) else {
                return;
            };
            draft.ansi[i] = Some(colour.unwrap_or_else(|| "#888888".to_string()));
        }
    }
    ctx.look.set_draft(Some(draft));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::picker::CodeField;
    use crate::rows::{
        APP_OPTIONS, DIAGNOSTICS_OPTIONS, DICTATION_OPTIONS, HELP_OPTIONS, MARK_OPTIONS, RowKind,
        TERMINAL_OPTIONS,
    };
    use crate::verb::FakeDiag;
    use crate::window_colours::{NO_COLOURS, custom_window_colours};
    use flare_theme::colour_contrast::AlphaRange;
    use std::cell::RefCell;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Rig {
        prefs: PrefStore,
        look: LookStore,
        diag: FakeDiag,
        pending: RefCell<Option<String>>,
    }

    impl Rig {
        fn new() -> Self {
            let prefs = PrefStore::memory();
            Rig {
                look: LookStore::new(prefs.clone()),
                prefs,
                diag: FakeDiag::new("C:/diag", false),
                pending: RefCell::new(None),
            }
        }

        fn go(&self, ev: Event) -> Vec<Effect> {
            apply(
                &Ctx {
                    prefs: &self.prefs,
                    look: &self.look,
                    diag: &self.diag,
                    pending_pick: &self.pending,
                },
                ev,
            )
        }

        fn slide(&self, row: &str, value: u32) {
            self.go(Event::Slide {
                row: row.into(),
                value,
            });
        }

        fn commit(&self, row: &str, stored: &str) {
            self.go(Event::ColourCommit {
                row: row.into(),
                stored: stored.into(),
            });
        }

        fn pick(&self, id: &str) -> Vec<Effect> {
            self.go(Event::WallPick { id: id.into() })
        }

        fn save(&self) {
            self.go(Event::SaveChanges {
                from: SaveFrom::Theme,
            });
        }

        fn dirty(&self) -> bool {
            appearance::save_dirty(&self.prefs, &self.look)
        }
    }

    const CUSTOM_JSON: &str = r##"{"bg":"#11111180","fg":"#eeeeee","cursor":"#ffffff","ansi":{}}"##;

    #[test]
    fn a_moved_slider_is_a_change_to_save_and_back_is_clean() {
        let r = Rig::new();
        assert!(!r.dirty());
        r.slide("term-acrylic", 20);
        assert!(r.dirty());
        r.slide("term-acrylic", 0);
        assert!(!r.dirty());
    }

    #[test]
    fn a_picked_background_never_lights_save() {
        let r = Rig::new();
        assert!(!r.look.setup_state().dirty);
        r.commit("window-background", "#223344");
        assert_eq!(BACKGROUND.get(&r.prefs).as_deref(), Some("#223344"));
        assert!(!r.look.setup_state().dirty);
    }

    #[test]
    fn a_save_keeps_the_level_with_custom_on_an_opaque_ground_and_is_clean_after() {
        let r = Rig::new();
        r.slide("term-acrylic", 35);
        r.save();
        let c = r.look.custom().expect("saved");
        assert_eq!(c.see_through, Some(35));
        assert_eq!(c.palette.bg.len(), 7);
        assert!(!r.dirty());
        r.slide("term-acrylic", 20);
        assert!(r.dirty());
    }

    #[test]
    fn the_editor_opens_on_an_opaque_ground() {
        let r = Rig::new();
        r.prefs.set(crate::custom_theme::CUSTOM_KEY, CUSTOM_JSON);
        r.look.set_theme_id("custom");
        r.go(Event::WallEdit {
            id: "custom".into(),
        });
        assert_eq!(r.look.draft().expect("draft").bg, "#111111");
    }

    #[test]
    fn colour_commit_rules_through_the_action() {
        let r = Rig::new();
        let range = AlphaRange::default();
        let counter = Arc::new(AtomicUsize::new(0));
        let c2 = Arc::clone(&counter);
        let _sub = ACCENT.on_change(&r.prefs, move || {
            c2.fetch_add(1, Ordering::SeqCst);
        });
        let typed = |text: Option<&str>| {
            let mut f = CodeField::default();
            if let Some(t) = text {
                f.edit(t);
            }
            f.commit("#336699", &range)
        };
        // Nothing typed, the same colour, junk: the field commits nothing, so nothing is written.
        for draft in [None, Some("#336699"), Some("zzz")] {
            if let Some(s) = typed(draft) {
                r.commit("window-accent", &s);
            }
        }
        r.commit("window-accent", "zzz");
        assert_eq!(ACCENT.get(&r.prefs), None);
        assert_eq!(counter.load(Ordering::SeqCst), 0);
        let s = typed(Some("123")).expect("a new colour");
        r.commit("window-accent", &s);
        assert_eq!(s, "#112233");
        assert_eq!(ACCENT.get(&r.prefs).as_deref(), Some("#112233"));
    }

    #[test]
    fn committing_what_is_already_stored_writes_and_notifies_nothing() {
        let r = Rig::new();
        r.commit("window-secondary", "#445566");
        let n = Arc::new(AtomicUsize::new(0));
        let n2 = Arc::clone(&n);
        let _s = SECONDARY.on_change(&r.prefs, move || {
            n2.fetch_add(1, Ordering::SeqCst);
        });
        let n3 = Arc::clone(&n);
        let _a = ACCENT.on_change(&r.prefs, move || {
            n3.fetch_add(1, Ordering::SeqCst);
        });
        r.commit("window-secondary", "#445566");
        r.go(Event::ColourRevert {
            row: "window-accent".into(),
            to: None,
        });
        r.go(Event::Reset {
            row: "window-accent".into(),
        });
        assert_eq!(n.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_clean_pick_lands_and_forgets_the_picked_window_colours() {
        let r = Rig::new();
        r.commit("window-accent", "#ff0000");
        r.save();
        assert!(!r.dirty());
        assert!(r.pick("dracula").is_empty());
        assert_eq!(r.look.theme_id(), "dracula");
        assert_eq!(ACCENT.get(&r.prefs), None);
    }

    #[test]
    fn a_dirty_pick_asks_and_cancel_changes_nothing() {
        let r = Rig::new();
        r.slide("term-acrylic", 20);
        let before = r.look.theme_id();
        assert_eq!(
            r.pick("dracula"),
            vec![Effect::AskThemeSwitch {
                to: "dracula".into()
            }]
        );
        r.go(Event::Ask {
            answer: AskAnswer::Cancel,
        });
        assert_eq!(r.look.theme_id(), before);
        assert!(r.pending.borrow().is_none());
        assert_eq!(r.look.see_through(), 20);
    }

    #[test]
    fn ask_save_as_custom_saves_then_lands() {
        let r = Rig::new();
        r.slide("term-acrylic", 20);
        r.pick("dracula");
        r.go(Event::Ask {
            answer: AskAnswer::SaveAsCustom,
        });
        assert_eq!(r.look.custom().expect("saved").see_through, Some(20));
        assert_eq!(r.look.theme_id(), "dracula");
    }

    /// Review #72: "save as Custom" on the way TO Custom keeps the Custom's palette.
    #[test]
    fn ask_save_as_custom_on_the_way_to_custom_keeps_the_saved_palette() {
        let r = Rig::new();
        r.pick("dracula");
        r.save();
        let saved = r.look.custom().expect("saved").palette.bg;
        r.pick("nord");
        r.slide("term-acrylic", 20);
        r.pick("custom");
        r.go(Event::Ask {
            answer: AskAnswer::SaveAsCustom,
        });
        let c = r.look.custom().expect("still saved");
        assert_eq!(
            c.palette.bg, saved,
            "the Custom's palette is not overwritten by nord's"
        );
        assert_eq!(
            c.see_through,
            Some(20),
            "the setup in force is saved with it"
        );
        assert_eq!(r.look.theme_id(), "custom");
    }

    #[test]
    fn ask_discard_drops_the_unsaved_changes_and_lands() {
        let r = Rig::new();
        r.slide("term-acrylic", 20);
        r.pick("dracula");
        r.go(Event::Ask {
            answer: AskAnswer::Discard,
        });
        assert_eq!(r.look.theme_id(), "dracula");
        assert_eq!(r.look.see_through(), 0);
        assert!(r.look.custom().is_none());
    }

    #[test]
    fn picking_custom_brings_back_its_kept_window_colours() {
        let r = Rig::new();
        r.commit("window-accent", "#ff0000");
        r.save();
        r.pick("dracula");
        assert_eq!(ACCENT.get(&r.prefs), None);
        r.pick("custom");
        assert_eq!(r.look.theme_id(), "custom");
        assert_eq!(ACCENT.get(&r.prefs).as_deref(), Some("#ff0000"));
    }

    #[test]
    fn the_editor_save_is_a_theme_pick_that_forgets_the_kept_colours() {
        let r = Rig::new();
        r.commit("window-accent", "#ff0000");
        r.save();
        r.go(Event::WallEdit {
            id: "custom".into(),
        });
        r.go(Event::EditorSet {
            key: "bg".into(),
            value: Some("#101010".into()),
        });
        r.go(Event::EditorSave);
        assert_eq!(r.look.theme_id(), "custom");
        assert_eq!(r.look.custom().expect("saved").palette.bg, "#101010");
        assert!(r.look.draft().is_none());
        assert_eq!(read_window_colours(&r.prefs), NO_COLOURS);
        assert_eq!(custom_window_colours(&r.prefs), NO_COLOURS);
    }

    #[test]
    fn two_editor_picks_in_one_frame_keep_both() {
        let r = Rig::new();
        r.go(Event::WallEdit {
            id: "dracula".into(),
        });
        r.go(Event::EditorSet {
            key: "bg".into(),
            value: Some("#010203".into()),
        });
        r.go(Event::EditorSet {
            key: "fg".into(),
            value: Some("#fafafa".into()),
        });
        r.go(Event::EditorSet {
            key: ANSI_KEYS[1].into(),
            value: None,
        });
        let d = r.look.draft().expect("draft");
        assert_eq!(d.bg, "#010203");
        assert_eq!(d.fg, "#fafafa");
        assert_eq!(d.ansi[1].as_deref(), Some("#888888"));
        r.go(Event::EditorCancel);
        assert!(r.look.draft().is_none());
    }

    #[test]
    fn help_off_stores_zero() {
        let r = Rig::new();
        r.go(Event::Switch {
            row: "help-enabled".into(),
            on: false,
        });
        assert_eq!(r.prefs.get(help_prefs::ENABLED_KEY).as_deref(), Some("0"));
        assert!(!help_prefs::help_enabled(&r.prefs));
    }

    #[test]
    fn the_caret_list_and_switch_write_their_keys() {
        let r = Rig::new();
        r.go(Event::Choose {
            row: "term-caret".into(),
            option: "bar".into(),
        });
        assert_eq!(r.prefs.get("flare.term.caretShape").as_deref(), Some("bar"));
        r.go(Event::Choose {
            row: "term-caret".into(),
            option: "beam".into(),
        });
        assert_eq!(
            r.prefs.get("flare.term.caretShape").as_deref(),
            Some("block")
        );
        r.go(Event::Switch {
            row: "term-caret-blink".into(),
            on: false,
        });
        assert_eq!(r.prefs.get("flare.term.caretBlink").as_deref(), Some("0"));
    }

    #[test]
    fn every_switch_row_id_has_a_writer() {
        let r = Rig::new();
        let tables = [
            &TERMINAL_OPTIONS[..],
            &MARK_OPTIONS[..],
            &DICTATION_OPTIONS[..],
            &HELP_OPTIONS[..],
            &DIAGNOSTICS_OPTIONS[..],
            &APP_OPTIONS[..],
        ];
        let mut seen = 0;
        for def in tables
            .into_iter()
            .flatten()
            .filter(|d| d.kind == RowKind::Switch)
        {
            // The Computer sharing switch is the app's (rule 1252): it is saved once the OS setup finished.
            if def.id.starts_with("dictation-")
                || def.id == "explorer-verb"
                || def.id == "diag-verbose"
                || def.id == "machines-sharing"
            {
                continue;
            }
            seen += 1;
            r.go(Event::Switch {
                row: def.id.into(),
                on: false,
            });
            assert_eq!(r.prefs.get(def.keys[0]).as_deref(), Some("0"), "{}", def.id);
        }
        assert!(seen >= 8);
        assert_eq!(
            r.go(Event::Switch {
                row: "explorer-verb".into(),
                on: false
            }),
            vec![Effect::SetVerb(false)]
        );
    }

    #[test]
    fn use_home_clears_the_folder_and_choose_asks_for_a_picker() {
        let r = Rig::new();
        folder_picked(&r.prefs, Some(PathBuf::from("C:/work")));
        assert_eq!(new_tab::new_tab_folder(&r.prefs), "C:/work");
        folder_picked(&r.prefs, None);
        assert_eq!(new_tab::new_tab_folder(&r.prefs), "C:/work");
        r.go(Event::Button {
            row: "newtab-mode".into(),
            button: "use-home".into(),
        });
        assert_eq!(new_tab::new_tab_folder(&r.prefs), "");
        assert_eq!(
            r.go(Event::Button {
                row: "newtab-mode".into(),
                button: "choose-folder".into()
            }),
            vec![Effect::PickFolder]
        );
    }

    #[test]
    fn verbose_shows_the_value_the_backend_returns() {
        let r = Rig::new();
        let fx = r.go(Event::Switch {
            row: "diag-verbose".into(),
            on: true,
        });
        assert_eq!(fx, vec![Effect::Rebuild]);
        assert!(r.diag.verbose());
        assert_eq!(
            r.go(Event::Button {
                row: "diag-folder".into(),
                button: "open".into()
            }),
            vec![Effect::OpenDiagFolder]
        );
    }

    #[test]
    fn mark_asks_the_log_once() {
        let r = Rig::new();
        let fx = r.go(Event::Button {
            row: "diag-mark".into(),
            button: "mark".into(),
        });
        assert_eq!(fx, vec![Effect::Marked]);
        assert_eq!(r.diag.marks(), 1);
    }

    #[test]
    fn this_file_defines_no_setter_of_its_own() {
        let src = include_str!("actions.rs");
        let code = src.split("#[cfg(test)]").next().expect("code part");
        for bad in [
            "prefs.set(",
            "prefs.remove(",
            "s.set(",
            ".set(\"flare.",
            ".remove(\"flare.",
            "\"flare.",
        ] {
            assert!(!code.contains(bad), "found {bad}");
        }
    }
}
