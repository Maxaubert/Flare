//! HelpCtl: the command help panel, F1, the title button and the menu row (M7 Task 43).
//!
//! Everything with a rule lives in `flare_help::panel` (the rows, the keys, the copy rule); this file
//! only moves data between that model and the `HelpBridge` the panel draws from, and carries out the
//! one effect the panel has: a callback that puts text on the clipboard. The panel shows and copies
//! and nothing more (rule 82), so nothing here holds a handle to a shell.

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use flare_help::entries::is_key_press;
use flare_help::panel::{
    COPIED_MS, EMPTY_TEXT, PLACEHOLDER, PanelKey, PanelOut, PanelState, SEARCH_LABEL, front_shell,
};
use flare_help::shells::HELP_SHELLS;
use flare_help::types::HelpShellChoice;
use flare_settings::help_prefs::{
    HELP_WIDTH, help_enabled, help_shell, help_width, set_help_shell, set_help_width,
};
use flare_settings::kv::PrefStore;
use flare_win::clipboard_port::CLIPBOARD_WRITE_MAX;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::{AppWindow, HelpBridge, HelpChip, HelpRowData};

/// The arrow keys move the grip by this many css px, and by the big step with Shift (PT HelpPanel).
const GRIP_STEP: f64 = 20.0;
const GRIP_STEP_BIG: f64 = 80.0;

static OPEN_SEQ: AtomicU64 = AtomicU64::new(0);

/// The next number in the order the panel and the find bar are opened in: the later one is on top.
pub fn next_open_seq() -> u64 {
    OPEN_SEQ.fetch_add(1, Ordering::SeqCst) + 1
}

pub struct HelpCtl {
    weak: slint::Weak<AppWindow>,
    me: Weak<RefCell<HelpCtl>>,
    prefs: PrefStore,
    /// Made at the first opening: the catalogue is not touched before somebody asks for help.
    panel: Option<PanelState>,
    open: bool,
    opened_seq: u64,
    find_seq: u64,
    copy: Rc<dyn Fn(&str)>,
    front_shell: Rc<dyn Fn() -> Option<String>>,
    still: Rc<dyn Fn() -> bool>,
    clock: Rc<dyn Fn() -> u64>,
    rows: Rc<VecModel<HelpRowData>>,
    copy_timer: slint::Timer,
}

impl HelpCtl {
    pub fn new(
        app: &AppWindow,
        prefs: PrefStore,
        copy: Rc<dyn Fn(&str)>,
        front_shell: Rc<dyn Fn() -> Option<String>>,
        still: Rc<dyn Fn() -> bool>,
    ) -> Rc<RefCell<HelpCtl>> {
        let started = Instant::now();
        let rows: Rc<VecModel<HelpRowData>> = Rc::default();
        let ctl = HelpCtl {
            weak: app.as_weak(),
            me: Weak::new(),
            prefs,
            panel: None,
            open: false,
            opened_seq: 0,
            find_seq: 0,
            copy,
            front_shell,
            still,
            clock: Rc::new(move || started.elapsed().as_millis() as u64),
            rows: Rc::clone(&rows),
            copy_timer: slint::Timer::default(),
        };
        let rc = Rc::new(RefCell::new(ctl));
        rc.borrow_mut().me = Rc::downgrade(&rc);
        {
            let b = app.global::<HelpBridge>();
            b.set_rows(ModelRc::from(rows));
            b.set_placeholder(PLACEHOLDER.into());
            b.set_search_label(SEARCH_LABEL.into());
            b.set_empty_text(EMPTY_TEXT.into());
            b.set_chips(chips_of(HelpShellChoice::Powershell));
            b.set_open(false);
        }
        Self::bind(app, &rc);
        rc.borrow().sync_button();
        rc.borrow().sync_width();
        rc
    }

    /// A clock for the copied check (tests drive their own).
    #[allow(dead_code)] // only the tests drive their own clock
    pub fn set_clock(&mut self, clock: Rc<dyn Fn() -> u64>) {
        self.clock = clock;
    }

    fn now(&self) -> u64 {
        (self.clock)()
    }

    // ---- the public face ---------------------------------------------------------------------------

    /// F1, the title button and the menu row: opens the panel, or closes it when it is open.
    pub fn toggle(&mut self) {
        if self.open {
            self.close();
        } else if self.enabled() {
            self.open_panel();
        }
    }

    /// Escape or the close button. The panel slides out (the view animates it over 220 ms) and is
    /// inert at once; the keyboard goes back to the shell. Opening again restarts the slide.
    pub fn close(&mut self) {
        if !self.open {
            return;
        }
        self.open = false;
        self.copy_timer.stop();
        if let Some(app) = self.weak.upgrade() {
            app.global::<HelpBridge>().set_open(false);
            app.set_help_on_top(false);
            app.invoke_focus_terminal();
        }
    }

    /// Whether command help is switched on at all.
    pub fn enabled(&self) -> bool {
        help_enabled(&self.prefs)
    }

    /// One of the three help settings changed: switched off, the panel closes at once and the
    /// button goes; the width is read again. The F1 chord returns the key to the shell because
    /// the key context says `help` is false.
    pub fn on_pref_change(&mut self) {
        if !self.enabled() {
            self.close();
        }
        self.sync_button();
        self.sync_width();
    }

    #[allow(dead_code)] // read by the tests
    pub fn is_open(&self) -> bool {
        self.open
    }

    #[allow(dead_code)] // read by the tests
    pub fn opened_seq(&self) -> u64 {
        self.opened_seq
    }

    /// The find bar opened (its number comes from `next_open_seq`): whichever of the two was opened
    /// last is on top in the corner.
    pub fn note_find_opened(&mut self, seq: u64) {
        self.find_seq = seq;
        self.sync_on_top();
    }

    // ---- opening -----------------------------------------------------------------------------------

    fn open_panel(&mut self) {
        let shell = front_shell(
            (self.front_shell)().as_deref(),
            help_shell(&self.prefs).as_deref(),
        );
        match &mut self.panel {
            Some(p) => p.open(shell),
            None => self.panel = Some(PanelState::new(shell)),
        }
        self.copy_timer.stop();
        self.open = true;
        self.opened_seq = next_open_seq();
        self.sync_width();
        if let Some(app) = self.weak.upgrade() {
            let b = app.global::<HelpBridge>();
            b.set_query("".into());
            b.set_still((self.still)());
            app.set_help_viewport_y(0.0);
        }
        self.fill();
        if let Some(app) = self.weak.upgrade() {
            // The view focuses the search field 60 ms after the first frame of the slide.
            app.global::<HelpBridge>().set_open(true);
        }
        self.sync_on_top();
    }

    fn sync_on_top(&self) {
        if let Some(app) = self.weak.upgrade() {
            app.set_help_on_top(self.open && self.opened_seq > self.find_seq);
        }
    }

    fn sync_button(&self) {
        if let Some(app) = self.weak.upgrade() {
            app.set_help_visible(self.enabled());
        }
    }

    /// The stored width in css px; the view keeps it inside the container less 120 on its own.
    fn sync_width(&self) {
        if let Some(app) = self.weak.upgrade() {
            app.global::<HelpBridge>()
                .set_panel_w(help_width(&self.prefs) as f32);
        }
    }

    // ---- the bridge ---------------------------------------------------------------------------------

    /// Everything the panel draws, from the model. The rows change in place, so a row that did not
    /// change keeps its widgets (and a selection made in its command well).
    fn fill(&self) {
        let (Some(app), Some(p)) = (self.weak.upgrade(), self.panel.as_ref()) else {
            return;
        };
        let b = app.global::<HelpBridge>();
        let copied = p.copied();
        let mut data: Vec<HelpRowData> = Vec::new();
        let mut caps: Vec<ModelRc<SharedString>> = Vec::new();
        for (i, r) in p.drawn().iter().enumerate() {
            let key = row_key(&r.entry.id, r.n);
            let keys = is_key_press(&r.command);
            data.push(HelpRowData {
                name: r.name.as_str().into(),
                command: r.command.as_str().into(),
                header: p.heading_for(i).unwrap_or("").into(),
                keys,
                danger: r.entry.danger.is_some(),
                active: p.marked() && i == p.active(),
                striped: PanelState::stripe(i),
                copied: copied == Some((key.as_str(), true)),
                failed: copied == Some((key.as_str(), false)),
                key: key.into(),
            });
            let parts: Vec<SharedString> = if keys {
                r.command.split('+').map(SharedString::from).collect()
            } else {
                Vec::new()
            };
            caps.push(ModelRc::new(VecModel::from(parts)));
        }
        replace_rows(&self.rows, data);
        b.set_key_caps(ModelRc::new(VecModel::from(caps)));
        b.set_chips(chips_of(p.chip()));
        b.set_count_text(p.count_text().into());
        b.set_said(p.said().into());
        b.set_empty(p.rows().is_empty());
        b.set_active_index(i32::try_from(p.active()).unwrap_or(0));
    }

    fn bind(app: &AppWindow, rc: &Rc<RefCell<HelpCtl>>) {
        let b = app.global::<HelpBridge>();
        macro_rules! on {
            ($reg:ident, |$c:ident $(, $a:ident : $t:ty)*| $body:expr) => {{
                let weak = Rc::downgrade(rc);
                b.$reg(move |$($a: $t),*| {
                    if let Some(rc) = weak.upgrade()
                        && let Ok(mut $c) = rc.try_borrow_mut()
                    {
                        crate::diag_ctl::timed(stringify!($reg), || {
                            $body;
                        });
                    }
                });
            }};
        }
        on!(on_closed, |c| c.close());
        on!(on_query_edited, |c, q: SharedString| c.query_edited(&q));
        on!(on_chip_picked, |c, id: SharedString| c.chip_picked(&id));
        on!(on_row_pressed, |c, i: i32| c.row_pressed(index(i)));
        on!(on_row_clicked, |c, i: i32, moved: f32| c
            .row_clicked(index(i), f64::from(moved)));
        on!(on_copy_clicked, |c, i: i32| c.row_clicked(index(i), 0.0));
        on!(on_key, |c, k: SharedString, _s: bool, _c: bool| c.key(&k));
        on!(on_pointer_over_list, |c| c.pointer_over_list());
        on!(on_near_end, |c| c.near_end());
        on!(on_grip_dragged, |c, travel: f32| c
            .grip_dragged(f64::from(travel)));
        on!(on_grip_key, |c, k: SharedString, shift: bool| c
            .grip_key(&k, shift));
    }

    // ---- events -------------------------------------------------------------------------------------

    fn query_edited(&mut self, q: &str) {
        let Some(p) = self.panel.as_mut() else { return };
        p.set_query(q);
        self.to_top();
        self.fill();
    }

    fn chip_picked(&mut self, id: &str) {
        let Some(shell) = HELP_SHELLS
            .iter()
            .find(|(c, _)| shell_id(*c) == id)
            .map(|(c, _)| *c)
        else {
            return;
        };
        let Some(p) = self.panel.as_mut() else { return };
        p.pick_chip(shell);
        set_help_shell(&self.prefs, id);
        self.to_top();
        self.fill();
    }

    fn row_pressed(&mut self, i: usize) {
        let Some(p) = self.panel.as_mut() else { return };
        p.pointer_down_on(i);
        self.fill();
    }

    fn row_clicked(&mut self, i: usize, moved: f64) {
        let Some(p) = self.panel.as_mut() else { return };
        let out = p.click_row(i, moved);
        self.handle(out);
    }

    fn key(&mut self, name: &str) {
        let k = match name {
            "Escape" => PanelKey::Escape,
            "Up" => PanelKey::Up,
            "Down" => PanelKey::Down,
            "PageUp" => PanelKey::PageUp,
            "PageDown" => PanelKey::PageDown,
            "Enter" => PanelKey::Enter,
            _ => return,
        };
        let Some(p) = self.panel.as_mut() else { return };
        let out = p.key(k, false);
        self.handle(out);
    }

    fn pointer_over_list(&mut self) {
        let Some(p) = self.panel.as_mut() else { return };
        if p.marked() {
            p.pointer_moved_over_list();
            self.fill();
        }
    }

    fn near_end(&mut self) {
        let Some(p) = self.panel.as_mut() else { return };
        if p.drawn().len() >= p.rows().len() {
            return;
        }
        p.near_end();
        self.fill();
    }

    fn grip_dragged(&mut self, travel: f64) {
        let Some(app) = self.weak.upgrade() else {
            return;
        };
        let from = f64::from(app.global::<HelpBridge>().get_grip_from());
        set_help_width(&self.prefs, from + travel);
        self.sync_width();
    }

    fn grip_key(&mut self, name: &str, shift: bool) {
        let step = if shift { GRIP_STEP_BIG } else { GRIP_STEP };
        let sign = match name {
            "Left" => 1.0,
            "Right" => -1.0,
            _ => return,
        };
        let Some(app) = self.weak.upgrade() else {
            return;
        };
        let shown = f64::from(app.get_help_sheet_w());
        let base = if shown > 0.0 {
            shown
        } else {
            f64::from(HELP_WIDTH.default)
        };
        set_help_width(&self.prefs, base + sign * step);
        self.sync_width();
    }

    fn to_top(&self) {
        if let Some(app) = self.weak.upgrade() {
            app.set_help_viewport_y(0.0);
        }
    }

    fn handle(&mut self, out: PanelOut) {
        match out {
            PanelOut::None | PanelOut::Said(_) => self.fill(),
            PanelOut::Close => self.close(),
            PanelOut::Copy { key, text } => self.do_copy(&key, &text),
        }
    }

    /// The only effect the panel has. Text over the clipboard cap is refused, never cut.
    fn do_copy(&mut self, key: &str, text: &str) {
        let ok = !text.is_empty() && text.chars().count() <= CLIPBOARD_WRITE_MAX;
        if ok {
            (self.copy)(text);
        }
        let now = self.now();
        if let Some(p) = self.panel.as_mut() {
            p.copy_result(key, text, ok, now);
        }
        self.fill();
        self.arm_copy_timer(COPIED_MS);
    }

    fn arm_copy_timer(&mut self, ms: u64) {
        let me = self.me.clone();
        self.copy_timer.start(
            slint::TimerMode::SingleShot,
            std::time::Duration::from_millis(ms),
            move || {
                if let Some(rc) = me.upgrade()
                    && let Ok(mut c) = rc.try_borrow_mut()
                {
                    c.tick();
                }
            },
        );
    }

    fn tick(&mut self) {
        let now = self.now();
        if let Some(p) = self.panel.as_mut() {
            p.tick(now);
        }
        self.fill();
        // The timer counts from the event loop's cached time, so it can fire a millisecond or so
        // before the check is COPIED_MS old by the panel's own clock. The check would then stay
        // for good (the e2e "the answer leaves by itself" timed out in a full run): ask again
        // for the little that is left.
        let left = self
            .panel
            .as_ref()
            .and_then(|p| p.copied_since())
            .map(|at| COPIED_MS.saturating_sub(now.saturating_sub(at)));
        if let Some(ms) = left {
            self.arm_copy_timer(ms.max(1));
        }
    }

    // ---- reads for the tests ------------------------------------------------------------------------

    #[cfg(test)]
    fn panel(&self) -> Option<&PanelState> {
        self.panel.as_ref()
    }
}

fn index(i: i32) -> usize {
    usize::try_from(i).unwrap_or(usize::MAX)
}

fn row_key(id: &str, n: usize) -> String {
    format!("{id}#{n}")
}

fn shell_id(c: HelpShellChoice) -> &'static str {
    match c {
        HelpShellChoice::Powershell => "powershell",
        HelpShellChoice::Cmd => "cmd",
        HelpShellChoice::Bash => "bash",
    }
}

fn chips_of(on: HelpShellChoice) -> ModelRc<HelpChip> {
    ModelRc::new(VecModel::from(
        HELP_SHELLS
            .iter()
            .map(|(c, name)| HelpChip {
                id: shell_id(*c).into(),
                name: (*name).into(),
                on: *c == on,
            })
            .collect::<Vec<_>>(),
    ))
}

/// Brings the model to `new` touching only the rows that differ.
fn replace_rows(model: &VecModel<HelpRowData>, new: Vec<HelpRowData>) {
    let have = model.row_count();
    for (i, row) in new.iter().enumerate() {
        if i < have {
            if model.row_data(i).as_ref() != Some(row) {
                model.set_row_data(i, row.clone());
            }
        } else {
            model.push(row.clone());
        }
    }
    while model.row_count() > new.len() {
        model.remove(model.row_count() - 1);
    }
}

/// A flag the tests use to count what the copy port was asked.
#[cfg(test)]
type Log = Rc<RefCell<Vec<String>>>;

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::ui_harness::Harness;
    use flare_help::panel::{FIRST_PAGE, OUT_MS};

    struct Fx {
        h: Harness<AppWindow>,
        ctl: Rc<RefCell<HelpCtl>>,
        prefs: PrefStore,
        copied: Log,
        front: Rc<RefCell<Option<String>>>,
        clock: Rc<Cell<u64>>,
    }

    fn fx() -> Fx {
        let prefs = PrefStore::memory();
        let copied: Log = Rc::default();
        let front: Rc<RefCell<Option<String>>> = Rc::default();
        let clock = Rc::new(Cell::new(1_000_u64));
        let h = Harness::new(1.0, 1000, 640, || AppWindow::new().unwrap());
        h.comp.set_front_kind(0);
        // The window is the active one, so a focused field reports it.
        h.comp
            .window()
            .dispatch_event(slint::platform::WindowEvent::WindowActiveChanged(true));
        h.advance(0);
        let ctl = HelpCtl::new(
            &h.comp,
            prefs.clone(),
            {
                let copied = Rc::clone(&copied);
                Rc::new(move |t: &str| copied.borrow_mut().push(t.to_string()))
            },
            {
                let front = Rc::clone(&front);
                Rc::new(move || front.borrow().clone())
            },
            Rc::new(|| false),
        );
        ctl.borrow_mut().set_clock({
            let c = Rc::clone(&clock);
            Rc::new(move || c.get())
        });
        Fx {
            h,
            ctl,
            prefs,
            copied,
            front,
            clock,
        }
    }

    impl Fx {
        fn bridge(&self) -> HelpBridge<'_> {
            self.h.comp.global::<HelpBridge>()
        }
        fn open(&self) {
            self.ctl.borrow_mut().toggle();
            self.h.advance(1);
            self.h.advance(900);
        }
        fn chip(&self) -> String {
            self.ctl
                .borrow()
                .panel()
                .map(|p| shell_id(p.chip()).to_string())
                .unwrap_or_default()
        }
        /// The index of the first row whose command is `cmd`.
        fn row_of(&self, cmd: &str) -> usize {
            let n = self.bridge().get_rows().row_count();
            (0..n)
                .find(|i| self.bridge().get_rows().row_data(*i).unwrap().command == cmd)
                .unwrap_or_else(|| panic!("no row with {cmd}"))
        }
    }

    #[test]
    fn opening_starts_on_the_front_tabs_shell_with_an_empty_question() {
        let f = fx();
        for (front, want) in [
            (Some("pwsh"), "powershell"),
            (Some("cmd"), "cmd"),
            (Some("wsl-ubuntu"), "bash"),
        ] {
            *f.front.borrow_mut() = front.map(str::to_string);
            f.open();
            assert_eq!(f.chip(), want, "front {front:?}");
            assert_eq!(f.bridge().get_query(), "");
            assert!(f.bridge().get_open());
            let on: Vec<_> = f
                .bridge()
                .get_chips()
                .iter()
                .filter(|c| c.on)
                .map(|c| c.id.to_string())
                .collect();
            assert_eq!(on, [want]);
            assert!(f.bridge().get_rows().row_count() > 0);
            assert!(f.bridge().get_rows().iter().all(|r| !r.active));
            f.ctl.borrow_mut().close();
        }
        // no tab: the chip picked by hand, else PowerShell
        *f.front.borrow_mut() = None;
        f.open();
        assert_eq!(f.chip(), "powershell");
        f.ctl.borrow_mut().close();
        set_help_shell(&f.prefs, "bash");
        f.open();
        assert_eq!(f.chip(), "bash");
    }

    #[test]
    fn the_panel_stays_open_across_a_tab_switch_and_a_click_into_the_shell() {
        let f = fx();
        f.open();
        // a tab switch changes the front shell and the page; the window state stays
        *f.front.borrow_mut() = Some("cmd".into());
        f.h.comp.set_front_kind(1);
        f.h.advance(50);
        f.h.comp.set_front_kind(0);
        f.h.advance(50);
        assert!(f.ctl.borrow().is_open());
        assert!(f.bridge().get_open());
        // a click into the shell hands the keys to the shell, and the panel stays
        f.h.comp.invoke_focus_terminal();
        f.h.advance(10);
        assert!(!f.h.comp.get_find_input_focused(), "the shell has the keys");
        assert!(f.ctl.borrow().is_open());
        assert!(f.bridge().get_open());
    }

    #[test]
    fn the_search_field_takes_the_keys_after_60_ms_and_the_slide_out_ends_inert() {
        let f = fx();
        f.ctl.borrow_mut().toggle();
        f.h.advance(1);
        assert!(!f.h.comp.get_find_input_focused());
        f.h.advance(80);
        assert!(f.h.comp.get_find_input_focused(), "the field is focused");
        f.ctl.borrow_mut().close();
        f.h.advance(OUT_MS + 20);
        assert!(!f.bridge().get_open());
        assert!(
            !f.h.comp.get_find_input_focused(),
            "the sheet takes no keys once closed"
        );
        // opening again restarts the slide and takes the keys again
        f.ctl.borrow_mut().toggle();
        f.h.advance(1);
        f.h.advance(80);
        assert!(f.bridge().get_open());
        assert!(f.h.comp.get_find_input_focused());
    }

    #[test]
    fn copy_hands_the_exact_text_to_the_clipboard_and_nothing_else() {
        let f = fx();
        f.open();
        let i = f.row_of("Set-Location ");
        f.bridge().invoke_copy_clicked(i as i32);
        assert_eq!(*f.copied.borrow(), vec!["Set-Location ".to_string()]);
        let r = f.bridge().get_rows().row_data(i).unwrap();
        assert!(r.copied && !r.failed);
        assert_eq!(f.bridge().get_said(), "Copied Set-Location ");
        // the check leaves after 1400 ms
        f.clock.set(f.clock.get() + COPIED_MS);
        f.h.advance(COPIED_MS + 10);
        let r = f.bridge().get_rows().row_data(i).unwrap();
        assert!(!r.copied);
        assert_eq!(f.bridge().get_said(), "");
        assert_eq!(f.copied.borrow().len(), 1);
    }

    #[test]
    fn a_timer_that_fires_early_still_clears_the_check() {
        let f = fx();
        f.open();
        let i = f.row_of("Set-Location ");
        f.bridge().invoke_copy_clicked(i as i32);
        // the timer fires when the panel's clock says 1 ms short of COPIED_MS
        f.clock.set(f.clock.get() + COPIED_MS - 1);
        f.h.advance(COPIED_MS + 10);
        f.clock.set(f.clock.get() + 1);
        f.h.advance(20);
        let r = f.bridge().get_rows().row_data(i).unwrap();
        assert!(!r.copied, "the check must leave, not stay for good");
    }

    #[test]
    fn a_click_on_a_row_outside_the_well_copies_and_a_drag_does_not() {
        let f = fx();
        f.open();
        let i = f.row_of("Set-Location ");
        f.bridge().invoke_row_clicked(i as i32, 12.0);
        assert!(f.copied.borrow().is_empty(), "a drag is a selection");
        f.bridge().invoke_row_clicked(i as i32, 1.0);
        assert_eq!(f.copied.borrow().len(), 1);
    }

    #[test]
    fn a_key_row_copies_nothing() {
        let f = fx();
        f.open();
        f.bridge().invoke_query_edited("interrupt".into());
        let keys = f
            .bridge()
            .get_rows()
            .iter()
            .position(|r| r.keys)
            .or_else(|| {
                // the browse list holds the key rows of the shell category
                f.bridge().invoke_query_edited("".into());
                f.bridge().get_rows().iter().position(|r| r.keys)
            });
        // Fall back to the whole catalogue when the first page has none.
        let i = keys.unwrap_or_else(|| {
            let mut c = f.ctl.borrow_mut();
            while c.panel().is_some_and(|p| p.drawn().len() < p.rows().len()) {
                c.near_end();
            }
            drop(c);
            f.bridge()
                .get_rows()
                .iter()
                .position(|r| r.keys)
                .expect("the catalogue has a key row")
        });
        f.bridge().invoke_copy_clicked(i as i32);
        f.bridge().invoke_row_clicked(i as i32, 0.0);
        assert!(f.copied.borrow().is_empty());
        let caps = f.bridge().get_key_caps().row_data(i).unwrap();
        assert!(caps.row_count() >= 1, "the caps are split for the view");
    }

    #[test]
    fn a_long_text_is_refused() {
        let f = fx();
        f.open();
        let long = "x".repeat(CLIPBOARD_WRITE_MAX + 1);
        f.ctl.borrow_mut().do_copy("k#0", &long);
        assert!(f.copied.borrow().is_empty());
        assert_eq!(f.bridge().get_said(), "Could not copy to the clipboard");
        let exact = "y".repeat(CLIPBOARD_WRITE_MAX);
        f.ctl.borrow_mut().do_copy("k#0", &exact);
        assert_eq!(f.copied.borrow().len(), 1);
    }

    #[test]
    fn enter_copies_the_marked_row_and_escape_closes() {
        let f = fx();
        f.open();
        f.bridge().invoke_key("Down".into(), false, false);
        let marked = f
            .bridge()
            .get_rows()
            .iter()
            .position(|r| r.active)
            .expect("a row is marked");
        let cmd = f.bridge().get_rows().row_data(marked).unwrap().command;
        f.bridge().invoke_key("Enter".into(), false, false);
        assert_eq!(
            f.copied.borrow().last().map(String::as_str),
            Some(cmd.as_str())
        );
        f.bridge().invoke_key("Escape".into(), false, false);
        assert!(!f.ctl.borrow().is_open());
        assert!(!f.bridge().get_open());
    }

    #[test]
    fn the_panel_never_writes_to_a_pty() {
        // Rule 82: the only outward path is the copy port; the source names no terminal handle.
        let src = include_str!("help_ctl.rs");
        let cut = src
            .find(&["mod tests", " {"].concat())
            .expect("the test module");
        let code = &src[..cut];
        for word in [
            "TermHandle",
            &["wri", "te"].concat(),
            "send_input",
            "send_key",
        ] {
            assert!(!code.contains(word), "help_ctl.rs names {word}");
        }
        // And a full session of searching, copying and Enter reaches only the copy port.
        let f = fx();
        f.open();
        f.bridge().invoke_query_edited("list files".into());
        f.bridge().invoke_key("Down".into(), false, false);
        f.bridge().invoke_key("Enter".into(), false, false);
        f.bridge().invoke_copy_clicked(0);
        assert!(!f.copied.borrow().is_empty());
    }

    #[test]
    fn the_width_is_clamped_stored_and_never_wider_than_the_container_less_120() {
        let f = fx();
        f.open();
        assert_eq!(f.bridge().get_panel_w(), 620.0);
        // the grip drags 50 px to the left from a press at 620
        f.bridge().set_grip_from(620.0);
        f.bridge().invoke_grip_dragged(50.0);
        assert_eq!(help_width(&f.prefs), 670);
        assert_eq!(f.bridge().get_panel_w(), 670.0);
        // too small and too large are clamped
        f.bridge().set_grip_from(620.0);
        f.bridge().invoke_grip_dragged(-250.0);
        assert_eq!(help_width(&f.prefs), 420);
        f.bridge().invoke_grip_dragged(5000.0);
        assert_eq!(help_width(&f.prefs), 1100);
        // a stored 1100 in a 1000 css px window shows at most 880 wide
        f.h.advance(10);
        let _ = f.h.render();
        let shown = f.h.comp.get_help_sheet_w();
        assert!(shown <= 1000.0 - 120.0 + 0.01, "sheet {shown}");
        // keys: Left grows by 20, Shift+Right shrinks by 80
        f.bridge().set_panel_w(620.0);
        f.prefs.set("flare.help.panelWidth", "620");
        f.h.advance(10);
        let _ = f.h.render();
        f.bridge().invoke_grip_key("Left".into(), false);
        assert_eq!(help_width(&f.prefs), 640);
        f.h.advance(10);
        let _ = f.h.render();
        f.bridge().invoke_grip_key("Right".into(), true);
        assert_eq!(help_width(&f.prefs), 560);
    }

    #[test]
    fn the_chip_pick_is_remembered() {
        let f = fx();
        f.open();
        f.bridge().invoke_chip_picked("cmd".into());
        assert_eq!(f.chip(), "cmd");
        assert_eq!(help_shell(&f.prefs).as_deref(), Some("cmd"));
        // the next opening with no tab in front starts on it
        f.ctl.borrow_mut().close();
        f.open();
        assert_eq!(f.chip(), "cmd");
        // junk is not a chip
        f.bridge().invoke_chip_picked("fish".into());
        assert_eq!(f.chip(), "cmd");
    }

    #[test]
    fn switching_help_off_closes_it_and_removes_the_button() {
        let f = fx();
        assert!(f.h.comp.get_help_visible());
        f.open();
        flare_settings::help_prefs::set_help_enabled(&f.prefs, false);
        f.ctl.borrow_mut().on_pref_change();
        assert!(!f.ctl.borrow().is_open());
        assert!(!f.bridge().get_open());
        assert!(!f.h.comp.get_help_visible());
        // and it does not open again while off
        f.ctl.borrow_mut().toggle();
        assert!(!f.ctl.borrow().is_open());
        flare_settings::help_prefs::set_help_enabled(&f.prefs, true);
        f.ctl.borrow_mut().on_pref_change();
        assert!(f.h.comp.get_help_visible());
    }

    #[test]
    fn the_title_button_shows_only_while_help_is_on_and_widens_the_group() {
        for scale in crate::ui_harness::SCALES {
            let prefs = PrefStore::memory();
            let h = Harness::new(scale, 1000, 640, || AppWindow::new().unwrap());
            h.advance(0);
            let ctl = HelpCtl::new(
                &h.comp,
                prefs.clone(),
                Rc::new(|_: &str| {}),
                Rc::new(|| None),
                Rc::new(|| false),
            );
            h.advance(1);
            let snap = |l: f32| (l * scale).round() / scale;
            assert!(h.comp.get_help_visible());
            assert!(
                (h.comp.get_title_buttons_w() - snap(190.0)).abs() < 0.001,
                "{scale}: {}",
                h.comp.get_title_buttons_w()
            );
            flare_settings::help_prefs::set_help_enabled(&prefs, false);
            ctl.borrow_mut().on_pref_change();
            h.advance(1);
            assert!(!h.comp.get_help_visible());
            assert!((h.comp.get_title_buttons_w() - snap(154.0)).abs() < 0.001);
        }
    }

    #[test]
    fn later_opened_wins_the_corner_with_find() {
        let f = fx();
        f.open();
        let help_seq = f.ctl.borrow().opened_seq();
        assert!(help_seq > 0);
        assert!(f.h.comp.get_help_on_top(), "help alone is on top");
        // the find bar opens after the panel: the bar is on top
        f.ctl.borrow_mut().note_find_opened(next_open_seq());
        assert!(!f.h.comp.get_help_on_top());
        // the panel closed and opened again is later than the bar
        f.ctl.borrow_mut().close();
        assert!(!f.h.comp.get_help_on_top());
        f.open();
        assert!(f.ctl.borrow().opened_seq() > help_seq);
        assert!(f.h.comp.get_help_on_top());
    }

    #[test]
    fn a_keystroke_is_answered_in_under_20_ms_once_warm() {
        let f = fx();
        f.open();
        // warm: the index is built by the first question
        f.bridge().invoke_query_edited("find big files".into());
        let mut worst = std::time::Duration::ZERO;
        for q in ["f", "fi", "fin", "find", "find b", "find bi", "find big"] {
            let t = Instant::now();
            f.bridge().invoke_query_edited(q.into());
            worst = worst.max(t.elapsed());
        }
        let budget = if cfg!(debug_assertions) { 400 } else { 20 };
        assert!(
            worst.as_millis() < budget,
            "worst keystroke {worst:?} (budget {budget} ms)"
        );
        assert!(f.bridge().get_rows().row_count() <= FIRST_PAGE.max(50));
    }

    #[test]
    fn a_question_scrolls_the_list_back_to_the_top_and_replaces_rows_in_place() {
        let f = fx();
        f.open();
        f.h.comp.set_help_viewport_y(-300.0);
        f.bridge().invoke_query_edited("delete a folder".into());
        assert_eq!(f.h.comp.get_help_viewport_y(), 0.0);
        let first = f.bridge().get_rows().row_data(0).unwrap();
        assert!(!first.header.is_empty() || f.bridge().get_count_text().contains("result"));
        assert!(f.bridge().get_count_text().contains("result"));
        f.bridge().invoke_query_edited("zzzzqqqqxxxx".into());
        assert!(f.bridge().get_empty());
        assert_eq!(f.bridge().get_rows().row_count(), 0);
        assert_eq!(f.bridge().get_count_text(), "No results");
    }
}
