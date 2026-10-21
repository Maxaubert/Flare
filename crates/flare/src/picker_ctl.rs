//! PickerCtl: the colour popover and the eyedropper (M7 Task 42).
//!
//! Every rule lives in `flare_settings::picker` (HSV, coalesced writes, revert, placement, the kept
//! picker, the code field); this file is the session around it. It owns the open popover, the typed
//! codes of the colour rows, the kept picker and the dropper's health. It never touches a store:
//! what the picker wants written comes out as `actions::Event`s (`take_events`), which
//! `SettingsCtl` applies. The popover's writes are coalesced to one per frame (`frame`), a drag
//! queues and the 16 ms timer that `SettingsCtl` runs only while `needs_frame` is true flushes.

use std::collections::HashMap;

use flare_settings::actions::Event;
use flare_settings::colour_format::{colour_format, cycle_colour_format};
use flare_settings::kv::PrefStore;
use flare_settings::picker::{
    Closed, CodeField, KeptPicker, POPOVER_W, Picker, Rect, SliderKey, picker_key, place_popover_at,
};
use flare_theme::colour::{ColourFormat, Hsva, Rgba, hsv_to_rgb, parse_colour};
use flare_theme::colour_contrast::AlphaRange;
use flare_win::eyedrop::{PickResult, pick_colour};

use crate::appctl::{Post, UiMsg};
use crate::settings_ctl::SettingsMsg;

/// The popover's height with and without the alpha bar (colour.slint `pop-h`).
const POP_H_ALPHA: f64 = 254.0;
const POP_H_PLAIN: f64 = 230.0;

/// What a colour row (or a theme editor well) says about itself at this moment.
#[derive(Clone, Debug, PartialEq)]
pub struct RowColour {
    pub label: String,
    /// The stored form in force (the picked one, else the theme's).
    pub value: String,
    /// What is stored: `None` when the row follows the theme.
    pub stored: Option<String>,
    pub range: AlphaRange,
    pub alpha_disabled: bool,
}

/// The world the controller needs and does not own.
pub struct PickerCtx<'a> {
    pub prefs: &'a PrefStore,
    /// The window in logical px, for the placement.
    pub window: (f64, f64),
    /// The device scale, so the popover lands on the pixel grid.
    pub scale: f64,
    /// `(row or well key, in the theme editor)` to what it holds now.
    pub lookup: &'a dyn Fn(&str, bool) -> Option<RowColour>,
    /// The value of `FLARE_E2E_PICK_COLOUR` (None unless the e2e flag is on).
    pub seam: Option<String>,
    pub post: &'a Post,
}

pub struct PickerSession {
    row: String,
    in_editor: bool,
    picker: Picker,
    /// The row's STORED state at the first opening (`None`: it followed the theme).
    before: Option<String>,
    anchor: Rect,
    range: AlphaRange,
    label: String,
    pos: (f64, f64),
    alpha_disabled: bool,
}

/// A picker left open by a tab switch.
struct Kept {
    row: String,
    in_editor: bool,
    before: Option<String>,
    anchor: Rect,
    opened: String,
}

/// Everything the popover draws, as plain values.
#[derive(Clone, Debug, PartialEq)]
pub struct PickerView {
    pub x: f64,
    pub y: f64,
    pub label: String,
    pub hue: f64,
    pub sat: f64,
    pub val: f64,
    pub alpha: f64,
    pub solid: Rgba,
    pub start: Rgba,
    pub now: Rgba,
    pub hue_colour: Rgba,
    pub alpha_on: bool,
    pub alpha_disabled: bool,
    pub dropper_on: bool,
    pub format: &'static str,
    pub sv_text: String,
    pub hue_text: String,
    pub alpha_text: String,
    pub sv_now: i32,
    pub hue_now: i32,
    pub alpha_now: i32,
}

pub struct PickerCtl {
    session: Option<PickerSession>,
    kept: KeptPicker,
    kept_session: Option<Kept>,
    dropper_ok: bool,
    fields: HashMap<String, CodeField>,
    out: Vec<Event>,
    queued: bool,
}

impl Default for PickerCtl {
    fn default() -> Self {
        PickerCtl {
            session: None,
            kept: KeptPicker::default(),
            kept_session: None,
            dropper_ok: true,
            fields: HashMap::new(),
            out: Vec::new(),
            queued: false,
        }
    }
}

fn slider_key(name: &str) -> Option<SliderKey> {
    Some(match name {
        "Left" => SliderKey::Left,
        "Right" => SliderKey::Right,
        "Up" => SliderKey::Up,
        "Down" => SliderKey::Down,
        "Home" => SliderKey::Home,
        "End" => SliderKey::End,
        _ => return None,
    })
}

fn format_word(f: ColourFormat) -> &'static str {
    match f {
        ColourFormat::Hex => "HEX",
        ColourFormat::Rgba => "RGBA",
        ColourFormat::Hsla => "HSLA",
    }
}

impl PickerCtl {
    /// The typed codes of the colour rows, for the page to draw.
    pub fn fields(&self) -> &HashMap<String, CodeField> {
        &self.fields
    }

    /// Events the picker wants applied, oldest first.
    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.out)
    }

    pub fn is_open(&self) -> bool {
        self.session.is_some()
    }

    /// Whether the picker is open on this row or well.
    pub fn is_open_for(&self, row: &str, in_editor: bool) -> bool {
        self.session
            .as_ref()
            .is_some_and(|s| s.row == row && s.in_editor == in_editor)
    }

    /// A write is queued: the 16 ms frame timer must run.
    pub fn needs_frame(&self) -> bool {
        self.queued
    }

    #[cfg(test)]
    pub fn position(&self) -> Option<(f64, f64)> {
        self.session.as_ref().map(|s| s.pos)
    }

    fn emit(&mut self, stored: String) {
        let Some(s) = &self.session else {
            return;
        };
        self.out.push(if s.in_editor {
            Event::EditorSet {
                key: s.row.clone(),
                value: Some(stored),
            }
        } else {
            Event::ColourCommit {
                row: s.row.clone(),
                stored,
            }
        });
    }

    fn start(
        &mut self,
        row: &str,
        in_editor: bool,
        anchor: Rect,
        ctx: &PickerCtx,
        before: Option<Option<String>>,
        opened: Option<&str>,
    ) {
        let Some(info) = (ctx.lookup)(row, in_editor) else {
            return;
        };
        let (opened, now) = match opened {
            Some(o) => (o.to_string(), Some(info.value.as_str())),
            None => (info.value.clone(), None),
        };
        let mut picker = Picker::open(&opened, now, info.range);
        picker.set_alpha_disabled(info.alpha_disabled);
        let h = if info.range.alpha {
            POP_H_ALPHA
        } else {
            POP_H_PLAIN
        };
        let pos = place_popover_at(anchor, (POPOVER_W, h), ctx.window, ctx.scale);
        self.session = Some(PickerSession {
            row: row.to_string(),
            in_editor,
            picker,
            before: before.unwrap_or(info.stored),
            anchor,
            range: info.range,
            label: info.label,
            pos,
            alpha_disabled: info.alpha_disabled,
        });
    }

    /// The swatch was pressed: toggles. A second press closes and forgets the kept picker; any
    /// other open picker closes first and keeps its colour.
    pub fn open(&mut self, row: &str, in_editor: bool, anchor: Rect, ctx: &PickerCtx) {
        if self.is_open_for(row, in_editor) {
            self.close_keep();
            return;
        }
        self.close_keep();
        self.start(row, in_editor, anchor, ctx, None, None);
    }

    /// The open picker's swatch moved (the format toggle widens the code fields): the popover is
    /// placed again against the swatch's new box. Anything but the open picker's own swatch is ignored.
    pub fn reanchor(&mut self, row: &str, in_editor: bool, anchor: Rect, ctx: &PickerCtx) {
        if !self.is_open_for(row, in_editor) {
            return;
        }
        if let Some(s) = self.session.as_mut() {
            let h = if s.range.alpha {
                POP_H_ALPHA
            } else {
                POP_H_PLAIN
            };
            s.anchor = anchor;
            s.pos = place_popover_at(anchor, (POPOVER_W, h), ctx.window, ctx.scale);
        }
    }

    pub fn on_sv(&mut self, x: f64, y: f64, key: bool) {
        if let Some(s) = &mut self.session {
            s.picker.drag_sv(x, y);
            self.queued = true;
        }
        if key {
            self.flush();
        }
    }

    pub fn on_hue(&mut self, x: f64, key: bool) {
        if let Some(s) = &mut self.session {
            s.picker.drag_hue(x);
            self.queued = true;
        }
        if key {
            self.flush();
        }
    }

    pub fn on_alpha(&mut self, x: f64, key: bool) {
        if let Some(s) = &mut self.session {
            s.picker.drag_alpha(x);
            self.queued = true;
        }
        if key {
            self.flush();
        }
    }

    /// A key on one of the sliders: `which` 0 the square, 1 the hue bar, 2 the alpha bar.
    pub fn on_key(&mut self, which: i32, name: &str, shift: bool) {
        let Some(key) = slider_key(name) else {
            return;
        };
        let Some(s) = &mut self.session else {
            return;
        };
        let stored = match which {
            0 => s.picker.key_sv(key, shift),
            1 => s.picker.key_hue(key, shift),
            _ => s.picker.key_alpha(key, shift),
        };
        if let Some(stored) = stored {
            self.emit(stored);
        }
    }

    /// Once per frame: the queued colour, when it differs from the last one sent.
    pub fn frame(&mut self) {
        self.flush();
    }

    fn flush(&mut self) {
        self.queued = false;
        let stored = self.session.as_mut().and_then(|s| s.picker.frame());
        if let Some(stored) = stored {
            self.emit(stored);
        }
    }

    /// The format toggle: HEX, RGBA, HSLA and round again; remembered in the pref.
    pub fn format_clicked(&mut self, ctx: &PickerCtx) {
        cycle_colour_format(ctx.prefs);
    }

    /// Closes keeping the colour (a press outside, focus away, a second swatch press).
    fn close_keep(&mut self) {
        if self.session.is_some() {
            self.flush();
            if let Some(s) = self.session.as_mut() {
                let _ = s.picker.close();
            }
            self.session = None;
        }
        self.kept.forget();
        self.kept_session = None;
    }

    pub fn outside(&mut self) {
        self.close_keep();
    }

    pub fn focus_left(&mut self) {
        self.close_keep();
    }

    /// Escape: consumed when open. The picker's own writes are put back exactly; a row that followed
    /// the theme goes back to following it.
    pub fn escape(&mut self) -> bool {
        self.queued = false;
        let Some(mut s) = self.session.take() else {
            return false;
        };
        self.kept.forget();
        self.kept_session = None;
        if let (Closed::Reverted, Some(_)) = s.picker.cancel() {
            self.out.push(if s.in_editor {
                Event::EditorSet {
                    key: s.row.clone(),
                    value: s.before.clone(),
                }
            } else {
                Event::ColourRevert {
                    row: s.row.clone(),
                    to: s.before.clone(),
                }
            });
        }
        true
    }

    /// Settings left the front: the overlay closes, the picker is remembered.
    pub fn hide_for_tab_switch(&mut self) {
        if self.session.is_none() {
            return;
        }
        self.flush();
        let Some(s2) = self.session.take() else {
            return;
        };
        let key = picker_key(s2.in_editor, &s2.label);
        let opened = s2.picker.opened().to_string();
        self.kept.keep(&key, &opened);
        self.kept_session = Some(Kept {
            row: s2.row,
            in_editor: s2.in_editor,
            before: s2.before,
            anchor: s2.anchor,
            opened,
        });
    }

    /// Settings came back: the kept picker reopens, showing the colour in force; Escape still
    /// reverts to the colour it first opened with.
    pub fn recall(&mut self, ctx: &PickerCtx) {
        let Some(k) = self.kept_session.take() else {
            return;
        };
        let Some(info) = (ctx.lookup)(&k.row, k.in_editor) else {
            self.kept.forget();
            return;
        };
        let key = picker_key(k.in_editor, &info.label);
        if self.kept.recall(&key).is_none() {
            return;
        }
        self.start(
            &k.row,
            k.in_editor,
            k.anchor,
            ctx,
            Some(k.before.clone()),
            Some(&k.opened),
        );
        // It stays kept until the user closes it.
        self.kept.keep(&key, &k.opened);
        self.kept_session = Some(k);
    }

    /// The eyedropper: asks for a colour; the answer comes back as `SettingsMsg::Dropper`.
    pub fn dropper(&mut self, ctx: &PickerCtx) {
        if !self.dropper_ok || self.session.is_none() {
            return;
        }
        let post = ctx.post.clone();
        pick_colour(ctx.seam.clone(), move |r| {
            post(UiMsg::Settings(SettingsMsg::Dropper(r)));
        });
    }

    /// The dropper's answer: the RGB is the screen's, the alpha stays the colour's own.
    pub fn dropper_done(&mut self, result: PickResult) {
        match result {
            PickResult::Picked(hex) => {
                let Some(c) = parse_colour(&hex) else {
                    return;
                };
                let stored = self
                    .session
                    .as_mut()
                    .and_then(|s| s.picker.set_rgb_keep_alpha(c));
                if let Some(stored) = stored {
                    self.emit(stored);
                }
            }
            PickResult::Cancelled => {}
            PickResult::Failed(why) => {
                log::warn!("eyedrop failed: {why}");
                self.dropper_ok = false;
            }
        }
    }

    // ---- the typed codes of the colour rows -----------------------------------------------------------

    pub fn field_edited(&mut self, row: &str, text: &str) {
        self.fields.entry(row.to_string()).or_default().edit(text);
    }

    /// Enter or blur: a colour is stored only when the typed text differs from what is shown.
    /// Returns true when a draft existed (the page redraws).
    pub fn field_committed(&mut self, row: &str, ctx: &PickerCtx) -> bool {
        let draft = self.fields.remove(row);
        let Some(mut field) = draft else {
            return false;
        };
        let Some(info) = (ctx.lookup)(row, false) else {
            return true;
        };
        if let Some(stored) = field.commit(&info.value, &info.range) {
            self.out.push(Event::ColourCommit {
                row: row.to_string(),
                stored,
            });
        }
        true
    }

    /// A draft is dropped and Escape consumed; with none, Escape is the host's.
    pub fn field_escape(&mut self, row: &str) -> bool {
        if self.fields.get_mut(row).is_some_and(CodeField::escape) {
            self.fields.remove(row);
            return true;
        }
        false
    }

    /// The code field of a row as it is shown, in the format in force.
    #[cfg(test)]
    pub fn shown(&self, row: &str, info: &RowColour, ctx: &PickerCtx) -> String {
        let f = colour_format(ctx.prefs);
        match self.fields.get(row) {
            Some(field) => field.shown(&info.value, &info.range, f),
            None => CodeField::default().shown(&info.value, &info.range, f),
        }
    }

    // ---- what the popover draws -----------------------------------------------------------------------

    pub fn view(&self, prefs: &PrefStore) -> Option<PickerView> {
        let s = self.session.as_ref()?;
        let hsv = s.picker.hsv();
        let aria = s.picker.aria();
        let solid = hsv_to_rgb(&Hsva { a: 1.0, ..hsv });
        let hue_colour = hsv_to_rgb(&Hsva {
            h: hsv.h,
            s: 100.0,
            v: 100.0,
            a: 1.0,
        });
        Some(PickerView {
            x: s.pos.0,
            y: s.pos.1,
            label: s.label.clone(),
            hue: hsv.h,
            sat: hsv.s,
            val: hsv.v,
            alpha: hsv.a,
            solid,
            start: s.picker.start(),
            now: s.picker.rgba(),
            hue_colour,
            alpha_on: s.range.alpha,
            alpha_disabled: s.alpha_disabled,
            dropper_on: self.dropper_ok,
            format: format_word(colour_format(prefs)),
            sv_text: aria.sv.text,
            hue_text: aria.hue.text,
            alpha_text: aria.alpha.text,
            sv_now: aria.sv.now as i32,
            hue_now: aria.hue.now as i32,
            alpha_now: aria.alpha.now as i32,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_settings::colour_format::COLOUR_FORMAT_KEY;
    use flare_theme::colour::{alpha_pct, format, to_stored};
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};

    const ALL: AlphaRange = AlphaRange {
        alpha: true,
        alpha_min: 0.0,
        alpha_max: 1.0,
    };

    /// A fake page: row id to (label, value in force, stored).
    type FakeRow = (String, String, Option<String>);

    #[derive(Default)]
    struct World {
        rows: RefCell<HashMap<String, FakeRow>>,
    }

    impl World {
        fn put(&self, id: &str, label: &str, value: &str, stored: Option<&str>) {
            self.rows.borrow_mut().insert(
                id.into(),
                (label.into(), value.into(), stored.map(String::from)),
            );
        }
        /// Applies events the way the actions would: a commit or revert changes what is in force.
        fn apply(&self, events: &[Event], theme: &str) {
            for e in events {
                match e {
                    Event::ColourCommit { row, stored } => {
                        let mut r = self.rows.borrow_mut();
                        let e = r.get_mut(row).unwrap();
                        e.1 = stored.clone();
                        e.2 = Some(stored.clone());
                    }
                    Event::ColourRevert { row, to } => {
                        let mut r = self.rows.borrow_mut();
                        let e = r.get_mut(row).unwrap();
                        e.2.clone_from(to);
                        e.1 = to.clone().unwrap_or_else(|| theme.to_string());
                    }
                    _ => {}
                }
            }
        }
    }

    struct Fx {
        world: Rc<World>,
        prefs: PrefStore,
        post: Post,
        posted: Arc<Mutex<Vec<PickResult>>>,
        ctl: PickerCtl,
        window: (f64, f64),
        seam: Option<String>,
    }

    fn fx() -> Fx {
        let world = Rc::new(World::default());
        world.put("window-accent", "Accent", "#336699", None);
        world.put("window-secondary", "Secondary", "#112233", Some("#112233"));
        let posted: Arc<Mutex<Vec<PickResult>>> = Arc::default();
        let p2 = Arc::clone(&posted);
        let post: Post = Arc::new(move |m| {
            if let UiMsg::Settings(SettingsMsg::Dropper(r)) = m {
                p2.lock().unwrap().push(r);
            }
        });
        Fx {
            world,
            prefs: PrefStore::memory(),
            post,
            posted,
            ctl: PickerCtl::default(),
            window: (800.0, 600.0),
            seam: None,
        }
    }

    impl Fx {
        fn with<R>(&mut self, f: impl FnOnce(&mut PickerCtl, &PickerCtx) -> R) -> R {
            let world = Rc::clone(&self.world);
            let lookup = move |id: &str, _ed: bool| -> Option<RowColour> {
                let r = world.rows.borrow();
                let (label, value, stored) = r.get(id)?.clone();
                Some(RowColour {
                    label,
                    value,
                    stored,
                    range: ALL,
                    alpha_disabled: false,
                })
            };
            let ctx = PickerCtx {
                prefs: &self.prefs,
                window: self.window,
                scale: 0.0,
                lookup: &lookup,
                seam: self.seam.clone(),
                post: &self.post,
            };
            f(&mut self.ctl, &ctx)
        }

        fn open(&mut self, row: &str) {
            self.with(|c, ctx| {
                c.open(
                    row,
                    false,
                    Rect {
                        x: 300.0,
                        y: 100.0,
                        w: 36.0,
                        h: 24.0,
                    },
                    ctx,
                );
            });
        }

        /// One frame: flush, apply what came out.
        fn frame(&mut self) -> Vec<Event> {
            self.ctl.frame();
            let ev = self.ctl.take_events();
            self.world.apply(&ev, "#336699");
            ev
        }
    }

    #[test]
    fn escape_stores_nothing_when_nothing_was_changed() {
        let mut f = fx();
        f.open("window-secondary");
        assert!(f.ctl.escape());
        assert!(f.ctl.take_events().is_empty());
        assert!(!f.ctl.is_open());
        // Escape with nothing open belongs to the host.
        assert!(!f.ctl.escape());
    }

    #[test]
    fn escape_after_a_pick_puts_back_exactly_the_opening_state_even_when_it_followed_the_theme() {
        let mut f = fx();
        f.open("window-accent");
        f.ctl.on_hue(0.9, false);
        let ev = f.frame();
        assert!(matches!(ev.as_slice(), [Event::ColourCommit { .. }]));
        assert!(f.ctl.escape());
        let ev = f.ctl.take_events();
        assert_eq!(
            ev,
            vec![Event::ColourRevert {
                row: "window-accent".into(),
                to: None
            }]
        );
        f.world.apply(&ev, "#336699");
        assert_eq!(f.world.rows.borrow()["window-accent"].2, None);
    }

    #[test]
    fn escape_after_a_pick_restores_a_stored_colour() {
        let mut f = fx();
        f.open("window-secondary");
        f.ctl.on_sv(0.8, 0.2, false);
        f.frame();
        f.ctl.escape();
        assert_eq!(
            f.ctl.take_events(),
            vec![Event::ColourRevert {
                row: "window-secondary".into(),
                to: Some("#112233".into())
            }]
        );
    }

    #[test]
    fn a_press_outside_keeps_the_colour() {
        let mut f = fx();
        f.open("window-secondary");
        f.ctl.on_hue(0.5, false);
        f.ctl.outside();
        let ev = f.ctl.take_events();
        assert!(matches!(ev.as_slice(), [Event::ColourCommit { .. }]));
        assert!(!f.ctl.is_open());
        // Nothing comes back after a tab switch: the user closed it.
        f.ctl.hide_for_tab_switch();
        f.with(|c, ctx| c.recall(ctx));
        assert!(!f.ctl.is_open());
    }

    #[test]
    fn the_picker_left_open_comes_back_after_a_tab_switch_and_escape_still_reverts_to_the_first_colour()
     {
        let mut f = fx();
        f.open("window-secondary");
        f.ctl.on_hue(0.5, false);
        f.frame();
        let in_force = f.world.rows.borrow()["window-secondary"].1.clone();
        f.ctl.hide_for_tab_switch();
        assert!(!f.ctl.is_open());
        f.with(|c, ctx| c.recall(ctx));
        assert!(f.ctl.is_open());
        let v = f.ctl.view(&f.prefs).unwrap();
        assert_eq!(to_stored(&v.now), in_force, "it shows the colour in force");
        assert!(f.ctl.escape());
        assert_eq!(
            f.ctl.take_events(),
            vec![Event::ColourRevert {
                row: "window-secondary".into(),
                to: Some("#112233".into())
            }]
        );
    }

    #[test]
    fn a_user_closed_picker_stays_closed() {
        let mut f = fx();
        f.open("window-secondary");
        // The second press on the swatch closes it and forgets it.
        f.open("window-secondary");
        assert!(!f.ctl.is_open());
        f.ctl.hide_for_tab_switch();
        f.with(|c, ctx| c.recall(ctx));
        assert!(!f.ctl.is_open());
        // Escape forgets it too.
        f.open("window-secondary");
        f.ctl.escape();
        f.with(|c, ctx| c.recall(ctx));
        assert!(!f.ctl.is_open());
    }

    #[test]
    fn a_drag_writes_once_per_frame() {
        let mut f = fx();
        f.open("window-secondary");
        assert!(!f.ctl.needs_frame());
        for i in 1..=8 {
            f.ctl.on_sv(f64::from(i) / 10.0, 0.3, false);
        }
        assert!(f.ctl.needs_frame());
        assert!(f.ctl.take_events().is_empty(), "nothing before the frame");
        let ev = f.frame();
        assert_eq!(ev.len(), 1);
        assert!(!f.ctl.needs_frame());
        assert!(f.frame().is_empty());
    }

    #[test]
    fn the_format_toggle_cycles_hex_rgba_hsla_and_remembers() {
        let mut f = fx();
        f.open("window-secondary");
        let info = RowColour {
            label: "Secondary".into(),
            value: "#336699cc".into(),
            stored: None,
            range: ALL,
            alpha_disabled: false,
        };
        let c = parse_colour("#336699cc").unwrap();
        let mut seen = Vec::new();
        for _ in 0..3 {
            f.with(|ctl, ctx| ctl.format_clicked(ctx));
            let text = f.with(|ctl, ctx| ctl.shown("window-secondary", &info, ctx));
            seen.push((f.ctl.view(&f.prefs).unwrap().format, text));
        }
        assert_eq!(seen[0].0, "RGBA");
        assert!(seen[0].1.starts_with("rgba("), "{}", seen[0].1);
        assert_eq!(seen[1].0, "HSLA");
        assert!(seen[1].1.starts_with("hsla("), "{}", seen[1].1);
        assert_eq!(seen[2].0, "HEX");
        assert_eq!(f.prefs.get(COLOUR_FORMAT_KEY).as_deref(), Some("hex"));
        // A typed hsla stores the same hex8.
        f.world.put("window-accent", "Accent", "#000000", None);
        let typed = format(&c, ColourFormat::Hsla);
        f.ctl.field_edited("window-accent", &typed);
        assert!(f.with(|ctl, ctx| ctl.field_committed("window-accent", ctx)));
        let ev = f.ctl.take_events();
        assert_eq!(
            ev,
            vec![Event::ColourCommit {
                row: "window-accent".into(),
                stored: "#336699cc".into()
            }]
        );
    }

    #[test]
    fn shift_left_walks_alpha_in_steps() {
        // Flare sets no host alpha table (that is the host table of Prism Terminal's glass levels), so Shift walks by
        // ten percent and a plain press by one.
        let mut f = fx();
        f.world
            .put("window-secondary", "Secondary", "#112233", Some("#112233"));
        f.open("window-secondary");
        let step = |f: &mut Fx, shift: bool| -> u32 {
            f.ctl.on_key(2, "Left", shift);
            let ev = f.ctl.take_events();
            let [Event::ColourCommit { stored, .. }] = ev.as_slice() else {
                panic!("{ev:?}");
            };
            alpha_pct(parse_colour(stored).unwrap().a)
        };
        assert_eq!(step(&mut f, true), 90);
        assert_eq!(step(&mut f, true), 80);
        assert_eq!(step(&mut f, false), 79);
    }

    #[test]
    fn tabbing_through_a_following_colour_row_pins_nothing() {
        let mut f = fx();
        // Focus enters (no edit) and leaves (blur commit): no write, nothing stored.
        let drew = f.with(|c, ctx| c.field_committed("window-accent", ctx));
        assert!(!drew);
        assert!(f.ctl.take_events().is_empty());
        // Typing the colour it already shows pins nothing either.
        f.ctl.field_edited("window-accent", "#336699");
        f.with(|c, ctx| c.field_committed("window-accent", ctx));
        assert!(f.ctl.take_events().is_empty());
    }

    #[test]
    fn typing_a_colour_commits_on_enter_and_on_blur_only_when_it_differs() {
        let mut f = fx();
        f.ctl.field_edited("window-secondary", "#112233");
        f.with(|c, ctx| c.field_committed("window-secondary", ctx));
        assert!(f.ctl.take_events().is_empty(), "same colour: no write");
        f.ctl.field_edited("window-secondary", "#445566");
        f.with(|c, ctx| c.field_committed("window-secondary", ctx));
        assert_eq!(
            f.ctl.take_events(),
            vec![Event::ColourCommit {
                row: "window-secondary".into(),
                stored: "#445566".into()
            }]
        );
        // Escape drops a draft; without one Escape is the host's.
        f.ctl.field_edited("window-secondary", "junk");
        assert!(f.ctl.field_escape("window-secondary"));
        assert!(!f.ctl.field_escape("window-secondary"));
    }

    #[test]
    fn the_dropper_seam_answers_without_a_window() {
        let mut f = fx();
        f.seam = Some("#00ff00".into());
        f.world.put(
            "window-secondary",
            "Secondary",
            "#11223380",
            Some("#11223380"),
        );
        f.open("window-secondary");
        f.with(|c, ctx| c.dropper(ctx));
        let got = f.posted.lock().unwrap().pop().expect("answered at once");
        assert_eq!(got, PickResult::Picked("#00ff00".into()));
        f.ctl.dropper_done(got);
        let ev = f.ctl.take_events();
        let [Event::ColourCommit { stored, .. }] = ev.as_slice() else {
            panic!("{ev:?}");
        };
        let c = parse_colour(stored).unwrap();
        assert_eq!(to_stored(&Rgba { a: 1.0, ..c }), "#00ff00");
        assert_eq!(alpha_pct(c.a), 50, "the alpha stays");
        // Cancel changes nothing.
        f.seam = Some("cancel".into());
        f.with(|c, ctx| c.dropper(ctx));
        let got = f.posted.lock().unwrap().pop().unwrap();
        assert_eq!(got, PickResult::Cancelled);
        f.ctl.dropper_done(got);
        assert!(f.ctl.take_events().is_empty());
    }

    #[test]
    fn a_failed_dropper_hides_its_button_for_the_session() {
        let mut f = fx();
        f.open("window-secondary");
        assert!(f.ctl.view(&f.prefs).unwrap().dropper_on);
        f.ctl.dropper_done(PickResult::Failed("no screen".into()));
        assert!(!f.ctl.view(&f.prefs).unwrap().dropper_on);
        f.ctl.outside();
        f.open("window-secondary");
        assert!(!f.ctl.view(&f.prefs).unwrap().dropper_on, "for the session");
    }

    #[test]
    fn a_second_picker_closes_the_first() {
        let mut f = fx();
        f.open("window-secondary");
        f.ctl.on_hue(0.4, false);
        f.open("window-accent");
        let ev = f.ctl.take_events();
        assert!(
            matches!(ev.as_slice(), [Event::ColourCommit { row, .. }] if row == "window-secondary"),
            "the first keeps its colour: {ev:?}"
        );
        assert!(f.ctl.is_open_for("window-accent", false));
        assert!(!f.ctl.is_open_for("window-secondary", false));
    }

    #[test]
    fn the_popover_follows_its_swatch_when_the_swatch_moves() {
        let mut f = fx();
        f.window = (800.0, 600.0);
        let at = |x: f64| Rect {
            x,
            y: 100.0,
            w: 36.0,
            h: 24.0,
        };
        f.with(|c, ctx| c.open("window-secondary", false, at(200.0), ctx));
        let first = f.ctl.position().unwrap();
        f.with(|c, ctx| c.reanchor("window-secondary", false, at(296.0), ctx));
        let moved = f.ctl.position().unwrap();
        assert!(f.ctl.is_open(), "moving the swatch keeps the picker open");
        assert!(
            (moved.0 - first.0 - 96.0).abs() < 0.5,
            "{first:?} to {moved:?}"
        );
        // Another row's swatch moving is not this picker's business.
        f.with(|c, ctx| c.reanchor("window-accent", false, at(500.0), ctx));
        assert_eq!(f.ctl.position().unwrap(), moved);
    }

    #[test]
    fn the_popover_is_placed_by_the_reference_rules() {
        let mut f = fx();
        f.window = (800.0, 600.0);
        let open_at = |f: &mut Fx, x: f64, y: f64| {
            f.with(|c, ctx| {
                c.open(
                    "window-secondary",
                    false,
                    Rect {
                        x,
                        y,
                        w: 36.0,
                        h: 24.0,
                    },
                    ctx,
                );
            });
            let p = f.ctl.position().unwrap();
            f.ctl.outside();
            p
        };
        // Top left: under the swatch, clamped to 8 from the left.
        let (x, y) = open_at(&mut f, 0.0, 0.0);
        assert_eq!((x, y), (8.0, 30.0));
        // Bottom right: above the swatch, inside the window.
        let (x, y) = open_at(&mut f, 780.0, 580.0);
        assert_eq!(x, 800.0 - POPOVER_W - 8.0);
        assert_eq!(y, 580.0 - POP_H_ALPHA - 6.0);
        // Top right.
        let (x, y) = open_at(&mut f, 780.0, 0.0);
        assert_eq!((x, y), (800.0 - POPOVER_W - 8.0, 30.0));
        // Bottom left.
        let (x, y) = open_at(&mut f, 0.0, 580.0);
        assert_eq!((x, y), (8.0, 580.0 - POP_H_ALPHA - 6.0));
    }
}
