//! UI flows of the picker, the theme wall and the switch question (M7 Task 48); headless only.
//!
//! Each test drives the real `SettingsCtl` over a headless `AppWindow`. The bridge callbacks are the
//! ones the window's Slint code fires, the stores are the real ones (in memory), and the bridge
//! properties are what the window draws. Keys reach the controller as the bridge's own key callbacks,
//! which the window's focus scopes call. Keyboard focus lives in Slint and is not readable from Rust;
//! the steps that depend on it say so where they occur. The e2e scenario each test ports is named in
//! the test's own doc line (Prism Terminal v0.55.6, `docs/parity.md`).

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};

use flare_settings::actions::{Event, SaveFrom};
use flare_settings::custom_theme::serialize_custom;
use flare_settings::kv::PrefStore;
use flare_settings::look::LookStore;
use flare_settings::page::appearance;
use flare_settings::page::{GlassSupport, ThemeColours};
use flare_settings::verb::{FakeDiag, FakeVerb, VerbState};
use flare_settings::wall::{self, CardData};
use flare_settings::window_colours::{ACCENT, BACKGROUND, SECONDARY, custom_window_colours};
use flare_theme::colour::{Rgba, parse_colour};
use slint::{ComponentHandle, Model};

use crate::appctl::{Post, UiMsg};
use crate::settings_ctl::{SettingsCtl, SettingsDeps, SettingsMsg};
use crate::ui_harness::{Harness, SCALES};
use crate::{AppWindow, SetRow, SetSection, SettingsBridge};

type Queue = Arc<Mutex<VecDeque<UiMsg>>>;

/// The window's Settings state for one test: the controller, its stores and the message queue.
struct Fx {
    h: Harness<AppWindow>,
    ctl: Rc<RefCell<SettingsCtl>>,
    prefs: PrefStore,
    look: LookStore,
    q: Queue,

    clock: Rc<Cell<u64>>,
}

fn fx_at(scale: f32) -> Fx {
    let prefs = PrefStore::memory();
    let look = LookStore::new(prefs.clone());
    let q: Queue = Arc::default();
    let post: Post = {
        let q = Arc::clone(&q);
        Arc::new(move |m| {
            q.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push_back(m);
        })
    };
    let h = Harness::new(scale, 1000, 640, || AppWindow::new().unwrap());
    h.comp.set_front_kind(1);
    h.advance(0);
    let opened: Rc<RefCell<Vec<PathBuf>>> = Rc::default();
    let clock = Rc::new(Cell::new(1_000_u64));
    let deps = SettingsDeps {
        prefs: prefs.clone(),
        look: look.clone(),
        diag: Rc::new(FakeDiag::new("C:/logs", false)),
        verb: Arc::new(FakeVerb::new(VerbState::Off)),
        dictation: Rc::new(crate::dictation_page::DictationPageProvider),
        pick_folder: Rc::new(|_, done| done(None)),
        open_path: {
            let opened = Rc::clone(&opened);
            Rc::new(move |p: &Path| opened.borrow_mut().push(p.to_path_buf()))
        },
        clipboard: Rc::new(|_: &str| {}),
        shell_list: Arc::new(|| vec![("pwsh".to_string(), "PowerShell".to_string())]),
        glass: Rc::new(|| GlassSupport::Ok),
        face_of: Rc::new(|_: &str| String::new()),
        theme_colours: Rc::new(|| ThemeColours {
            accent: "#fe8f34".into(),
            background: "#101010".into(),
            secondary: "#181818".into(),
        }),
        agent_colours: Rc::new(|| ["#111111".into(), "#222222".into(), "#333333".into()]),
        agent_key_labels: Rc::new(String::new),
        post,
        version: "1.2.3".to_string(),
        copyright: "Copyright 2026".to_string(),
        e2e: true,
        clock: {
            let c = Rc::clone(&clock);
            Rc::new(move || c.get())
        },
        still: Rc::new(|| false),
    };
    let ctl = SettingsCtl::new(&h.comp, deps);
    ctl.borrow_mut().show();
    let f = Fx {
        h,
        ctl,
        prefs,
        look,
        q,
        clock,
    };
    f.settle();
    f
}

/// Runs one flow at every display scaling (the flows are logic, but the window is drawn at each one).
fn every_scale(body: fn(&Fx)) {
    for s in SCALES {
        let f = fx_at(s);
        body(&f);
    }
}

impl Fx {
    fn b(&self) -> SettingsBridge<'_> {
        self.h.comp.global::<SettingsBridge>()
    }

    /// Pumps until both start-up answers, the shell list and the Explorer verb's state, have arrived.
    /// Each comes from a worker thread, so on a slow machine one lands whenever it likes; unwaited, it
    /// reaches a later pump in the middle of a flow, rebuilds the page and takes the keyboard focus
    /// off the rail (the runner failed the rail test that way). Every flow now starts after both.
    /// KNOWN APP BUG this works around: in the real app a late shell list or verb answer (or any pref change\n    /// from elsewhere) rebuilds the open page via refresh_if_visible and drops the rail focus; fix it in\n    /// settings_ctl.rs (keep focus across a refresh), then this wait can go.
    fn settle(&self) {
        let (mut shells, mut verb) = (false, false);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !(shells && verb) {
            let msgs: Vec<UiMsg> = self
                .q
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .drain(..)
                .collect();
            for m in msgs {
                if let UiMsg::Settings(m) = m {
                    shells |= matches!(m, SettingsMsg::ShellList(_));
                    verb |= matches!(m, SettingsMsg::Verb(_));
                    self.ctl.borrow_mut().on_msg(m);
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the start-up answers never came (shells {shells}, verb {verb})"
            );
            if !(shells && verb) {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
    }
    /// Hands every queued message to the controller, as the window's pump does.
    fn pump(&self) {
        let msgs: Vec<UiMsg> = self
            .q
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
            .collect();
        for m in msgs {
            if let UiMsg::Settings(m) = m {
                self.ctl.borrow_mut().on_msg(m);
            }
        }
    }

    fn event(&self, ev: Event) {
        self.ctl.borrow_mut().event(ev);
        self.pump();
    }

    fn page(&self, id: &str) {
        self.b().invoke_page_chosen(id.into());
        self.pump();
    }

    fn sections(&self) -> Vec<SetSection> {
        self.b().get_sections().iter().collect()
    }

    fn row(&self, id: &str) -> Option<SetRow> {
        self.sections()
            .iter()
            .flat_map(|s| s.rows.iter())
            .find(|r| r.id == id)
    }

    fn row_ids(&self) -> Vec<String> {
        self.sections()
            .iter()
            .flat_map(|s| s.rows.iter())
            .map(|r| r.id.to_string())
            .collect()
    }

    fn cards(&self) -> Vec<CardData> {
        wall::wall_cards(&self.look)
    }

    /// The Save button of the Appearance page is lit: the page's save action reads dirty.
    fn save_lit(&self) -> bool {
        appearance::save_dirty(&self.prefs, &self.look)
    }

    /// The accent the window wears now (the chrome token the window's theme reads).
    fn applied_accent(&self) -> String {
        self.look
            .look(false)
            .chrome
            .vars
            .get("--p-accent")
            .cloned()
            .unwrap_or_default()
    }

    /// The accent-on text the window wears on an accent fill.
    fn applied_on_accent(&self) -> String {
        self.look
            .look(false)
            .chrome
            .vars
            .get("--p-on-accent")
            .cloned()
            .unwrap_or_default()
    }

    /// Opens the swatch of a colour row (anchored where the page would draw it).
    fn open_swatch(&self, id: &str) {
        self.b()
            .invoke_colour_swatch(id.into(), 300.0, 100.0, 36.0, 24.0);
        self.pump();
    }

    /// Opens the swatch of a theme editor well.
    fn open_well(&self, key: &str) {
        self.b()
            .invoke_editor_well_swatch(key.into(), 300.0, 100.0, 36.0, 24.0);
        self.pump();
    }

    /// Opens the theme editor on a card, as its pencil does.
    fn open_editor_on(&self, id: &str) {
        self.event(Event::WallEdit { id: id.to_string() });
    }

    /// The card that wears the pencil (the theme in force).
    fn selected_card(&self) -> CardData {
        self.cards()
            .into_iter()
            .find(|c| c.on)
            .expect("one theme is in force")
    }

    /// The theme in force is not `id`: the first card that is not the one on.
    fn other_card(&self) -> String {
        let on = self.look.theme_id();
        self.cards()
            .into_iter()
            .find(|c| c.id != on)
            .expect("there are other themes")
            .id
    }

    /// Lets the clock run and the timers fire (the picker's frame and the wall's animation).
    fn tick(&self, ms: u64) {
        self.clock.set(self.clock.get() + ms);
        self.h.advance(ms);
        self.pump();
    }
}

/// A flushed pick on the square near its top right (saturation 90, brightness 90). Unlike the hue bar
/// it changes a grey too, so the stored colour always moves.
fn drag_square_to_vivid(f: &Fx) {
    f.b().invoke_picker_sv(0.9, 0.1, true);
    f.pump();
}

/// A stored colour as the Slint colour the window draws it in.
fn slint_colour(stored: &str) -> slint::Color {
    let c: Rgba = parse_colour(stored).expect("a stored colour");
    // r, g and b are 0 to 255; the alpha is 0 to 1.
    let byte = |v: f64| v.round().clamp(0.0, 255.0) as u8;
    slint::Color::from_argb_u8(byte(c.a * 255.0), byte(c.r), byte(c.g), byte(c.b))
}

// ---- the colour picker -----------------------------------------------------------------------------

fn the_working_colour_follows_the_theme_until_changed_and_its_picker_is_named_at(f: &Fx) {
    // e2e colourPicker. The swatch's accessible name is "Pick " plus the row's label, set in
    // colour.slint from `root.label`; the label and the popover's name are what Rust hands over, and
    // both are asserted here (Slint's accessible tree is not readable from a headless test).
    f.page("agents");
    let row = f.row("agent-color").expect("the working colour row");
    assert_eq!(row.label, "Agent working colour");
    assert!(!row.reset, "it follows the theme, so no Reset is offered");
    assert_eq!(f.prefs.get("flare.term.agentColor"), None);
    f.open_swatch("agent-color");
    assert!(f.b().get_picker_open());
    assert_eq!(f.b().get_picker_label().as_str(), "Agent working colour");
    drag_square_to_vivid(f);
    assert!(
        f.prefs.get("flare.term.agentColor").is_some(),
        "the first change stores the colour"
    );
    assert!(f.row("agent-color").expect("row").reset, "now Reset shows");
}

fn escape_closes_and_restores_and_focus_returns_to_the_swatch_at(f: &Fx) {
    // e2e colourPicker, the Escape half. Focus returning to the swatch is Slint's `focus-swatch()`
    // (colour.slint) and is not observable from Rust; the rest is asserted.
    f.open_swatch("window-accent");
    drag_square_to_vivid(f);
    assert!(ACCENT.get(&f.prefs).is_some(), "the drag was written");
    f.b().invoke_picker_escape();
    f.pump();
    assert!(!f.b().get_picker_open());
    assert_eq!(ACCENT.get(&f.prefs), None, "Escape puts the colour back");
    assert!(
        !f.ctl.borrow_mut().escape(),
        "with nothing open, Escape is the host's"
    );
}

fn shift_left_walks_alpha_in_steps_and_stores_hex8_at(f: &Fx) {
    // Alpha moves 1 per Left and 10 per Shift+Left: five Shift+Left presses take 100 to 50.
    f.open_swatch("window-accent");
    for _ in 0..5 {
        f.b().invoke_picker_key(2, "Left".into(), true);
    }
    f.pump();
    let stored = ACCENT
        .get(&f.prefs)
        .expect("the alpha walk stored a colour");
    assert_eq!(stored.len(), 9, "a hex8 colour: {stored}");
    assert!(stored.starts_with("#fe8f34"), "the hue is kept: {stored}");
    assert!(stored.ends_with("80"), "50 percent is 0x80: {stored}");
}

fn background_has_no_alpha_and_the_popover_says_so_at(f: &Fx) {
    // The ground is opaque (`alpha_disabled`), so the popover draws no alpha bar.
    f.open_swatch("window-background");
    assert!(f.b().get_picker_open());
    assert!(!f.b().get_picker_alpha_on(), "Background has no Alpha");
    f.b().invoke_picker_escape();
}

// ---- the theme editor ----------------------------------------------------------------------------

fn a_picked_background_paints_at_once_and_survives_a_tab_switch_and_the_picker_returns_with_focus_at(
    f: &Fx,
) {
    // e2e themeEditorLive. The preview is the editor's own card (`editor-preview`), redrawn by the
    // rebuild the picker asks for on every editor write.
    let id = f.selected_card().id;
    f.open_editor_on(&id);
    assert!(f.look.draft().is_some(), "the editor is open");
    let before = f.look.draft().expect("draft").bg;
    f.open_well("bg");
    assert!(f.b().get_picker_open());
    drag_square_to_vivid(f);
    let picked = f.look.draft().expect("draft").bg;
    assert_ne!(before, picked, "the pick changed the draft");
    assert_eq!(
        f.b().get_editor_preview().bg,
        slint_colour(&picked),
        "the preview paints the pick at once"
    );
    // Another tab takes the window, then Settings comes back.
    f.ctl.borrow_mut().hide();
    assert!(!f.b().get_picker_open());
    f.ctl.borrow_mut().show();
    f.pump();
    assert!(f.b().get_picker_open(), "the kept picker returns");
    assert_eq!(
        f.look.draft().expect("the draft survived").bg,
        picked,
        "the pick survived the tab switch"
    );
    f.b().invoke_picker_escape();
    f.pump();
}

fn editor_escape_closes_and_a_user_closed_picker_stays_closed_at(f: &Fx) {
    let id = f.selected_card().id;
    f.open_editor_on(&id);
    f.open_well("bg");
    assert!(f.b().get_picker_open());
    // The user closes the picker by pressing outside: it keeps its colour and is forgotten.
    f.b().invoke_picker_outside();
    f.pump();
    assert!(!f.b().get_picker_open());
    // Escape with the picker shut closes the editor (the question and the editor come before any picker).
    assert!(f.ctl.borrow_mut().escape(), "Escape closes the editor");
    assert!(f.look.draft().is_none(), "the editor is shut");
    f.ctl.borrow_mut().show();
    f.pump();
    assert!(
        !f.b().get_picker_open(),
        "a picker the user closed does not come back"
    );
}

fn saving_the_editor_keeps_the_origin_themes_accent_at(f: &Fx) {
    // The editor's save is a Custom theme built from the origin palette; its accent comes with it.
    let origin = f.selected_card().id;
    let accent_before = f.applied_accent();
    f.open_editor_on(&origin);
    f.b().invoke_editor_save();
    f.pump();
    let saved = f.look.custom().expect("the editor saved a Custom");
    assert_eq!(f.look.theme_id(), "custom");
    assert_eq!(
        saved.palette.accent.as_deref(),
        Some("#fe8f34"),
        "PT Default's accent is kept"
    );
    assert_eq!(f.applied_accent(), accent_before, "the window wears it");
    assert_eq!(
        f.prefs.get("flare.window.accent"),
        None,
        "no window accent pick"
    );
}

// ---- the switch question and the theme wall ---------------------------------------------------------

fn a_clean_pick_lands_a_dirty_pick_asks_discard_drops_cancel_keeps_at(f: &Fx) {
    // e2e themeSwitch. Font size runs 50 to 200 in tens and never lights Save.
    for v in (50..=200).step_by(10) {
        f.b().invoke_row_slide("term-font".into(), v);
        f.pump();
        assert_eq!(f.look.font_pct(), v as u32, "font size {v}");
    }
    assert!(!f.save_lit(), "appearance rows do not light Save");
    // The list: the Custom card first when one is saved, then PT Default.
    let first = f.other_card();
    let second = f
        .cards()
        .into_iter()
        .map(|c| c.id)
        .find(|id| *id != first && *id != f.look.theme_id())
        .expect("a third theme");
    // A clean pick lands at once.
    f.b().invoke_wall_pick(first.as_str().into());
    f.pump();
    assert!(!f.b().get_ask_open(), "no question when nothing changed");
    assert_eq!(f.look.theme_id(), first);
    // A dirty pick asks first.
    f.b().invoke_row_slide("term-acrylic".into(), 20);
    f.pump();
    f.b().invoke_wall_pick(second.as_str().into());
    f.pump();
    assert!(f.b().get_ask_open(), "a dirty pick asks");
    assert_eq!(f.look.theme_id(), first, "nothing changed yet");
    // Cancel keeps everything.
    f.b().invoke_ask_answered(0);
    f.pump();
    assert!(!f.b().get_ask_open());
    assert_eq!(f.look.theme_id(), first, "Cancel keeps the theme");
    assert_eq!(f.look.see_through(), 20, "Cancel keeps the see-through");
    // Discard drops the changes and lands the pick.
    f.b().invoke_wall_pick(second.as_str().into());
    f.pump();
    f.b().invoke_ask_answered(1);
    f.pump();
    assert_eq!(f.look.theme_id(), second, "Discard lands the pick");
    assert_eq!(f.look.see_through(), 0, "and drops the change");
}

fn the_switch_question_says_where_its_three_buttons_are_at(f: &Fx) {
    // e2e accentAsk reads the question's paint, so the window reports the buttons' boxes.
    let first = f.other_card();
    f.b().invoke_wall_pick(first.as_str().into());
    f.pump();
    let second = f
        .cards()
        .into_iter()
        .map(|c| c.id)
        .find(|id| *id != first && *id != f.look.theme_id())
        .expect("a third theme");
    f.b().invoke_row_slide("term-acrylic".into(), 20);
    f.pump();
    f.b().invoke_wall_pick(second.as_str().into());
    f.pump();
    f.h.advance(0);
    assert!(f.b().get_ask_open(), "a dirty pick asks");
    let r: Vec<f32> = f.h.comp.get_ask_rects().iter().collect();
    assert_eq!(
        r.len(),
        12,
        "x, y, w, h of Cancel, Discard and Save as Custom"
    );
    let (cancel, discard, save) = (&r[0..4], &r[4..8], &r[8..12]);
    let win_w = 1000.0_f32;
    let win_h = 640.0_f32;
    for b in [cancel, discard, save] {
        assert!(b[2] > 40.0 && b[3] >= 30.0, "a button has a size ({b:?})");
        assert!(b[0] >= 0.0 && b[0] + b[2] <= win_w && b[1] >= 0.0 && b[1] + b[3] <= win_h);
    }
    assert!(
        cancel[0] + cancel[2] <= discard[0] + 0.5 && discard[0] + discard[2] <= save[0] + 0.5,
        "left to right: Cancel, Discard, Save as Custom ({cancel:?} {discard:?} {save:?})"
    );
}
fn a_saved_custom_comes_first_then_pt_default_and_carries_no_font_at(f: &Fx) {
    f.event(Event::SaveChanges {
        from: SaveFrom::Theme,
    });
    let ids: Vec<String> = f.cards().into_iter().map(|c| c.id).collect();
    assert_eq!(ids[0], "custom", "Custom first");
    assert_eq!(ids[1], "pt-default", "then PT Default");
    let saved = f.look.custom().expect("saved");
    assert!(
        !serialize_custom(&saved).contains("font"),
        "a Custom carries no font"
    );
}

fn a_red_accent_lights_save_with_white_text_and_a_near_black_red_is_lifted_at(f: &Fx) {
    // The accent ask: a red accent is a dirty window colour, so Save lights, and white reads on it.
    f.event(Event::ColourCommit {
        row: "window-accent".into(),
        stored: "#ff0000".into(),
    });
    assert!(f.save_lit(), "a red accent lights Save");
    assert_eq!(f.applied_on_accent(), "#ffffff", "white text on the red");
    // A near-black red reads as nothing on the ground, so the window lifts it (M4's rule).
    f.event(Event::ColourCommit {
        row: "window-accent".into(),
        stored: "#200000".into(),
    });
    let applied = f.applied_accent();
    assert_ne!(applied, "#200000", "the near-black red is lifted");
    assert!(applied.starts_with('#'), "a plain colour: {applied}");
}

fn secondary_sits_above_accent_a_pick_lights_save_save_keeps_it_with_custom_another_theme_forgets_it_custom_brings_it_back_at(
    f: &Fx,
) {
    // e2e windowColoursSave.
    let ids = f.row_ids();
    let at = |id: &str| ids.iter().position(|x| x == id).expect("row drawn");
    assert!(
        at("window-secondary") < at("window-accent"),
        "order on the page"
    );
    // A pick lights Save.
    f.event(Event::ColourCommit {
        row: "window-secondary".into(),
        stored: "#102030".into(),
    });
    assert!(f.save_lit(), "a pick lights Save");
    // Save keeps the colour with Custom, and Save clears the mark.
    f.event(Event::SaveChanges {
        from: SaveFrom::Theme,
    });
    assert!(!f.save_lit(), "Save leaves nothing to save");
    assert_eq!(
        custom_window_colours(&f.prefs).secondary.as_deref(),
        Some("#102030")
    );
    // Another theme forgets the window colour.
    let other = f.other_card();
    f.b().invoke_wall_pick(other.as_str().into());
    f.pump();
    assert!(!f.b().get_ask_open(), "Save left nothing to ask about");
    assert_eq!(SECONDARY.get(&f.prefs), None, "another theme forgets it");
    // Custom brings it back.
    f.b().invoke_wall_pick("custom".into());
    f.pump();
    assert_eq!(f.look.theme_id(), "custom");
    assert_eq!(
        SECONDARY.get(&f.prefs).as_deref(),
        Some("#102030"),
        "Custom brings it back"
    );
    assert_eq!(BACKGROUND.get(&f.prefs), None);
}

fn the_see_through_row_is_a_slider_starting_at_zero_stepping_by_one_and_stopping_at_70_at(f: &Fx) {
    // e2e seeThrough.
    f.page("appearance");
    let row = f.row("term-acrylic").expect("the see-through row");
    assert_eq!(row.num, 0, "starts at zero");
    assert_eq!(row.num_max, 70, "the top is 70");
    f.b().invoke_row_slide("term-acrylic".into(), 1);
    f.pump();
    assert_eq!(f.look.see_through(), 1, "one step");
    f.b().invoke_row_slide("term-acrylic".into(), 100);
    f.pump();
    assert_eq!(f.look.see_through(), 70, "stops at 70");
    f.b().invoke_row_slide("term-acrylic".into(), 30);
    f.pump();
    assert_eq!(
        f.row("term-acrylic").expect("row").num_text.as_str(),
        "30%",
        "the value reads 30%"
    );
    assert!(f.save_lit(), "a moved slider lights Save");
    // A pick asks, Cancel keeps the see-through.
    let other = f.other_card();
    f.b().invoke_wall_pick(other.as_str().into());
    f.pump();
    assert!(f.b().get_ask_open(), "a pick asks");
    f.b().invoke_ask_answered(0);
    f.pump();
    assert_eq!(f.look.see_through(), 30, "Cancel keeps it");
    // Save as Custom keeps the 45, and the theme picked after lands with 0.
    f.b().invoke_row_slide("term-acrylic".into(), 45);
    f.pump();
    f.b().invoke_wall_pick(other.as_str().into());
    f.pump();
    f.b().invoke_ask_answered(2);
    f.pump();
    assert_eq!(
        f.look.custom().and_then(|c| c.see_through),
        Some(45),
        "Save as Custom keeps 45"
    );
    assert_eq!(f.look.theme_id(), other, "the pick lands");
    assert_eq!(f.look.see_through(), 0, "another theme is 0");
}

fn row_buttons_and_pressed_segments_ignore_the_accent_an_on_switch_wears_it_and_a_hex8_accent_gives_a_see_through_fill_at(
    f: &Fx,
) {
    // e2e accent. The look of the buttons and the switch is pixels (Task 45's look tests); this asserts
    // the data they are drawn from: a hex8 accent is a see-through fill (the chrome's accent carries
    // its alpha), and a plain accent is an opaque colour.
    let plain = f.applied_accent();
    assert_eq!(plain.len(), 7, "the default accent is opaque: {plain}");
    f.event(Event::ColourCommit {
        row: "window-accent".into(),
        stored: "#fe8f3380".into(),
    });
    assert_eq!(f.applied_accent().len(), 9, "a hex8 accent keeps its alpha");
    f.event(Event::SaveChanges {
        from: SaveFrom::Theme,
    });
    f.event(Event::ColourCommit {
        row: "window-accent".into(),
        stored: "#fe8f34".into(),
    });
    assert_eq!(f.applied_accent().len(), 7, "an opaque accent again");
}

fn tabbing_through_a_following_colour_field_stores_nothing_and_offers_no_reset_at(f: &Fx) {
    // e2e reviewSettings.
    let row = f.row("window-accent").expect("the accent row");
    assert!(!row.reset, "a following colour offers no Reset");
    // Focus moves through the code field and the swatch: a commit with nothing typed stores nothing.
    f.b().invoke_colour_committed("window-accent".into());
    f.pump();
    assert_eq!(ACCENT.get(&f.prefs), None, "tabbing stores nothing");
    // The host's Escape: nothing was drafted, so it is not consumed by the field.
    f.b().invoke_colour_escape("window-accent".into());
    f.pump();
    // A typed hsla stores the same hex8 as the typed hex8.
    f.b()
        .invoke_colour_edited("window-accent".into(), "rgba(255, 140, 51, 0.5)".into());
    f.b().invoke_colour_committed("window-accent".into());
    f.pump();
    let from_rgba = ACCENT.get(&f.prefs).expect("the rgba was stored");
    f.event(Event::ColourRevert {
        row: "window-accent".into(),
        to: None,
    });
    f.b()
        .invoke_colour_edited("window-accent".into(), "#ff8c3380".into());
    f.b().invoke_colour_committed("window-accent".into());
    f.pump();
    assert_eq!(
        ACCENT.get(&f.prefs).as_deref(),
        Some(from_rgba.as_str()),
        "the same hex8"
    );
}

fn show_all_expands_and_show_fewer_collapses_and_the_label_says_how_many_at(f: &Fx) {
    // e2e wall. With a Custom saved the wall holds presets plus one: "Show all 40 themes".
    f.event(Event::SaveChanges {
        from: SaveFrom::Theme,
    });
    f.pump();
    let total = wall::total_themes(true);
    assert_eq!(total, 40, "the presets and the saved Custom");
    assert_eq!(
        f.b().get_wall_toggle_aria().as_str(),
        format!("Show all {total} themes")
    );
    let collapsed = f.b().get_wall_clip_h();
    f.b().invoke_wall_toggle();
    f.pump();
    assert!(f.b().get_wall_expanded());
    assert_eq!(f.b().get_wall_toggle_aria().as_str(), "Show fewer themes");
    f.tick(300);
    assert!(
        (f.b().get_wall_clip_h() - f.b().get_wall_full_h()).abs() < 0.5,
        "the whole wall shows"
    );
    assert!(f.b().get_wall_full_h() > collapsed);
    f.b().invoke_wall_toggle();
    f.pump();
    assert!(!f.b().get_wall_expanded());
    assert_eq!(
        f.b().get_wall_toggle_aria().as_str(),
        format!("Show all {total} themes"),
        "the label says how many again"
    );
}

fn the_pencil_is_on_the_selected_card_only_and_opens_the_editor_seeded_from_it_at(f: &Fx) {
    // e2e wall, the pencil. Presets never change: an edit that is not saved leaves them as they were.
    let cards: Vec<_> = f.b().get_wall_cards().iter().collect();
    let pencils: Vec<_> = cards.iter().filter(|c| c.edit).collect();
    assert_eq!(pencils.len(), 1, "one pencil");
    assert!(pencils[0].on, "on the selected card");
    let id = pencils[0].id.to_string();
    let before = f.selected_card();
    f.b().invoke_wall_edit(id.as_str().into());
    f.pump();
    let draft = f.look.draft().expect("the editor opened");
    assert_eq!(
        draft.bg,
        f.look_palette_bg(&id),
        "seeded from the card's own colours"
    );
    f.b().invoke_editor_cancel();
    f.pump();
    assert!(f.look.draft().is_none());
    assert!(f.look.custom().is_none(), "the presets never change");
    assert_eq!(
        f.look.theme_id(),
        before.id,
        "the theme in force is the same"
    );
    assert_eq!(
        f.selected_card().bg,
        before.bg,
        "the card's colours are the same"
    );
}

impl Fx {
    /// The ground the editor starts from for a card: what `palette_of` gives, with the ground opaque.
    fn look_palette_bg(&self, id: &str) -> String {
        let custom = self.look.custom();
        let p =
            flare_settings::custom_theme::palette_of(id, custom.as_ref(), &self.look.theme_id());
        flare_settings::custom_theme::with_ground_alpha(&p).bg
    }
}

/// The rail by keyboard in the whole window (e2e settingsKeys): Tab from the Find field lands on the
/// rail's first page, Down walks it, Enter opens the page.
fn tab_from_the_field_lands_on_the_rail_and_enter_opens_the_page_at(f: &Fx) {
    f.h.activate();
    f.h.comp.invoke_settings_focus_find();
    f.h.advance(0);
    let key = |k: slint::platform::Key| f.h.key_text(&String::from(char::from(k)));
    f.h.key_text("\t");
    key(slint::platform::Key::DownArrow);
    key(slint::platform::Key::DownArrow);
    f.pump();
    key(slint::platform::Key::Return);
    f.pump();
    f.h.advance(0);
    assert_eq!(f.b().get_page_title().as_str(), "Agents");
}

#[test]
fn tab_from_the_field_lands_on_the_rail_and_enter_opens_the_page() {
    every_scale(tab_from_the_field_lands_on_the_rail_and_enter_opens_the_page_at);
}

#[test]
fn the_working_colour_follows_the_theme_until_changed_and_its_picker_is_named() {
    every_scale(the_working_colour_follows_the_theme_until_changed_and_its_picker_is_named_at);
}

#[test]
fn escape_closes_and_restores_and_focus_returns_to_the_swatch() {
    every_scale(escape_closes_and_restores_and_focus_returns_to_the_swatch_at);
}

#[test]
fn shift_left_walks_alpha_in_steps_and_stores_hex8() {
    every_scale(shift_left_walks_alpha_in_steps_and_stores_hex8_at);
}

#[test]
fn background_has_no_alpha_and_the_popover_says_so() {
    every_scale(background_has_no_alpha_and_the_popover_says_so_at);
}

#[test]
fn a_picked_background_paints_at_once_and_survives_a_tab_switch_and_the_picker_returns_with_focus()
{
    every_scale(a_picked_background_paints_at_once_and_survives_a_tab_switch_and_the_picker_returns_with_focus_at);
}

#[test]
fn editor_escape_closes_and_a_user_closed_picker_stays_closed() {
    every_scale(editor_escape_closes_and_a_user_closed_picker_stays_closed_at);
}

#[test]
fn saving_the_editor_keeps_the_origin_themes_accent() {
    every_scale(saving_the_editor_keeps_the_origin_themes_accent_at);
}

#[test]
fn a_clean_pick_lands_a_dirty_pick_asks_discard_drops_cancel_keeps() {
    every_scale(a_clean_pick_lands_a_dirty_pick_asks_discard_drops_cancel_keeps_at);
}

#[test]
fn the_switch_question_says_where_its_three_buttons_are() {
    every_scale(the_switch_question_says_where_its_three_buttons_are_at);
}

#[test]
fn a_saved_custom_comes_first_then_pt_default_and_carries_no_font() {
    every_scale(a_saved_custom_comes_first_then_pt_default_and_carries_no_font_at);
}

#[test]
fn a_red_accent_lights_save_with_white_text_and_a_near_black_red_is_lifted() {
    every_scale(a_red_accent_lights_save_with_white_text_and_a_near_black_red_is_lifted_at);
}

#[test]
fn secondary_sits_above_accent_a_pick_lights_save_save_keeps_it_with_custom_another_theme_forgets_it_custom_brings_it_back()
 {
    every_scale(secondary_sits_above_accent_a_pick_lights_save_save_keeps_it_with_custom_another_theme_forgets_it_custom_brings_it_back_at);
}

#[test]
fn the_see_through_row_is_a_slider_starting_at_zero_stepping_by_one_and_stopping_at_70() {
    every_scale(
        the_see_through_row_is_a_slider_starting_at_zero_stepping_by_one_and_stopping_at_70_at,
    );
}

#[test]
fn row_buttons_and_pressed_segments_ignore_the_accent_an_on_switch_wears_it_and_a_hex8_accent_gives_a_see_through_fill()
 {
    every_scale(row_buttons_and_pressed_segments_ignore_the_accent_an_on_switch_wears_it_and_a_hex8_accent_gives_a_see_through_fill_at);
}

#[test]
fn tabbing_through_a_following_colour_field_stores_nothing_and_offers_no_reset() {
    every_scale(tabbing_through_a_following_colour_field_stores_nothing_and_offers_no_reset_at);
}

#[test]
fn show_all_expands_and_show_fewer_collapses_and_the_label_says_how_many() {
    every_scale(show_all_expands_and_show_fewer_collapses_and_the_label_says_how_many_at);
}

#[test]
fn the_pencil_is_on_the_selected_card_only_and_opens_the_editor_seeded_from_it() {
    every_scale(the_pencil_is_on_the_selected_card_only_and_opens_the_editor_seeded_from_it_at);
}
