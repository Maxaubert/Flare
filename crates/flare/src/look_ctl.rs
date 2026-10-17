//! LookCtl: store listeners, startup order, developer flags and chords (Task 26).
//! Source: PT `src/renderer/src/lib/paintChrome.ts` (one paint, again whenever anything changes),
//! `core/renderer/lib/lookApply.ts` (`oncePerFrame`), regression rule 65 (the redraw nudge), decision D13.
//!
//! One place reads the settings and wears them: the Slint chrome and the grid through `chrome_apply`, and
//! the DWM material through `WindowMaterial`, in the order of decision D7 (the backdrop first when glass
//! turns on, the UI first when it turns off). Many changes in one turn are applied once.

// Reached from the window's start-up and the headless scenes; some helpers are for the tests and M7.
#![allow(dead_code)]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use flare_settings::kv::Subscription;
use flare_settings::look::{LookSnapshot, LookStore};
use flare_theme::edges::WindowEdges;
use flare_theme::look::{AppliedLook, apply_look};
use flare_theme::marks::Mode;
use flare_theme::order::wall_ids;
use flare_theme::preset::HOST_DEFAULT_THEME;
use flare_theme::reports::{
    NUDGE_AFTER_MS, NUDGE_HOLD_MS, THEME_REPORTS_OFF, ThemeReports, nudge_widths, theme_push,
};
use flare_theme::retired::live_theme_id;
use flare_theme::see_through::SEE_THROUGH_TOP;

use crate::AppWindow;
use crate::backend::BackendOutcome;
use crate::chrome_apply::{apply_chrome, apply_to_host};
pub use crate::config::LookOverrides;
use crate::host::GridHost;
use crate::material_ctl::{Order, WindowMaterial, order_for};

// ---- the two sides of an apply -----------------------------------------------------------------

/// The UI side of one apply: the Slint chrome and the grid.
pub trait LookUi {
    fn apply(&mut self, look: &AppliedLook);
}

/// The material side: the frost behind a see-through window and the border.
pub trait LookMaterial {
    /// A window handle exists and the material is attached.
    fn attached(&self) -> bool;
    /// The build has the backdrop call.
    fn supported(&self) -> bool;
    fn set_solid_ground(&mut self, hex: &str, edges: WindowEdges, now: Instant);
    fn set_glass(&mut self, on: bool, now: Instant);
    /// What the material wears now: false when the OS refused the backdrop or full screen holds it off.
    fn glass_on(&self) -> bool;
}

/// The window material shared with the window's own timer and state callbacks.
pub type SharedMaterial = Rc<RefCell<Option<WindowMaterial>>>;

impl LookMaterial for SharedMaterial {
    fn attached(&self) -> bool {
        self.borrow().is_some()
    }
    fn supported(&self) -> bool {
        self.borrow()
            .as_ref()
            .is_some_and(WindowMaterial::supported)
    }
    fn set_solid_ground(&mut self, hex: &str, edges: WindowEdges, now: Instant) {
        if let Some(m) = self.borrow_mut().as_mut() {
            m.set_solid_ground(hex, edges, now);
        }
    }
    fn set_glass(&mut self, on: bool, now: Instant) {
        if let Some(m) = self.borrow_mut().as_mut() {
            m.set_glass(on, now);
        }
    }
    fn glass_on(&self) -> bool {
        self.borrow().as_ref().is_some_and(WindowMaterial::glass_on)
    }
}

struct SlintUi {
    weak: slint::Weak<AppWindow>,
    host: Rc<RefCell<GridHost>>,
}

impl LookUi for SlintUi {
    fn apply(&mut self, look: &AppliedLook) {
        if let Some(app) = self.weak.upgrade() {
            apply_chrome(&app, look);
        }
        apply_to_host(&mut self.host.borrow_mut(), look);
    }
}

// ---- theme reports to the programs in the tabs ------------------------------------------------------

/// A tab as the theme push sees it: what the program asked for, a way to answer, and its size.
pub trait ThemePushTarget {
    fn reports(&self) -> ThemeReports;
    fn set_reports(&self, s: ThemeReports);
    fn write_report(&self, bytes: &str);
    fn size(&self) -> (usize, usize);
    fn resize(&self, cols: usize, rows: usize);
}

/// A tab whose handle does not expose the program's `?2031h` state: it is never told anything.
/// (M2's emulator parses `?2031h` but the handle keeps no per-tab report state, so every tab is this
/// until the handle grows one.)
pub struct NoReports;

impl ThemePushTarget for NoReports {
    fn reports(&self) -> ThemeReports {
        THEME_REPORTS_OFF
    }
    fn set_reports(&self, _s: ThemeReports) {}
    fn write_report(&self, _bytes: &str) {}
    fn size(&self) -> (usize, usize) {
        (0, 0)
    }
    fn resize(&self, _cols: usize, _rows: usize) {}
}

enum NudgeStage {
    /// Waiting to make the tab one column narrower.
    Narrow(Instant),
    /// Narrower now; back to full width at this time.
    Back(Instant),
}

struct Nudge {
    target: Rc<dyn ThemePushTarget>,
    cols: usize,
    rows: usize,
    stage: NudgeStage,
}

// ---- the controller ----------------------------------------------------------------------------

thread_local! {
    /// The controller the coalesced apply reaches (the UI thread only).
    static CURRENT: RefCell<Option<Rc<RefCell<LookCtl>>>> = const { RefCell::new(None) };
}

fn schedule_on_ui() {
    let _ = slint::invoke_from_event_loop(|| {
        let rc = CURRENT.with(|c| c.borrow().clone());
        let Some(rc) = rc else { return };
        match rc.try_borrow_mut() {
            Ok(mut ctl) => ctl.apply_now(),
            // Busy (a re-entrant moment): try again after the current turn rather than lose it.
            Err(_) => schedule_on_ui(),
        }
    });
}

pub struct LookCtl {
    store: LookStore,
    overrides: LookOverrides,
    ui: Box<dyn LookUi>,
    material: Option<Box<dyn LookMaterial>>,
    shared: Option<SharedMaterial>,
    /// The window was created transparent on the GPU renderer (decision D8).
    outcome_glass: bool,
    /// What the UI last wore: glass only when the material wore it too.
    was_glass: bool,
    /// What the settings last asked of the material.
    was_wanted: bool,
    last_light: bool,
    queued: Arc<AtomicBool>,
    scheduler: Arc<dyn Fn() + Send + Sync>,
    nudges: Vec<Nudge>,
    sub: Option<Subscription>,
}

impl LookCtl {
    /// The controller for this window. `material` is the slot the window shares with its own timer;
    /// it stays empty until [`LookCtl::attach_material`] (no OS window exists before `show()`).
    pub fn new(
        app: &AppWindow,
        store: LookStore,
        host: Rc<RefCell<GridHost>>,
        material: SharedMaterial,
        outcome: &BackendOutcome,
        overrides: LookOverrides,
    ) -> Self {
        let ui = SlintUi {
            weak: slint::ComponentHandle::as_weak(app),
            host,
        };
        let mut c = Self::with_parts(
            Box::new(ui),
            store,
            outcome.glass_possible,
            overrides,
            Arc::new(schedule_on_ui),
        );
        c.shared = Some(material);
        c
    }

    /// The same from its parts (tests use fakes for the UI, the material and the scheduler).
    pub fn with_parts(
        ui: Box<dyn LookUi>,
        store: LookStore,
        glass_possible: bool,
        overrides: LookOverrides,
        scheduler: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self {
            store,
            overrides,
            ui,
            material: None,
            shared: None,
            outcome_glass: glass_possible,
            was_glass: false,
            was_wanted: false,
            last_light: false,
            queued: Arc::new(AtomicBool::new(false)),
            scheduler,
            nudges: Vec::new(),
            sub: None,
        }
    }

    /// Puts this controller where the coalesced apply finds it and listens to the store. Not called by
    /// `--e2e` and the headless scenes (they apply once, with overrides).
    pub fn install(self) -> Rc<RefCell<LookCtl>> {
        let rc = Rc::new(RefCell::new(self));
        {
            let mut c = rc.borrow_mut();
            let trigger = c.trigger();
            c.sub = Some(c.store.on_look_change(trigger));
        }
        CURRENT.with(|cur| *cur.borrow_mut() = Some(rc.clone()));
        rc
    }

    /// Puts this controller where the window's state changes find it, without listening to the store
    /// (`--e2e` and the headless-style runs apply once, with overrides).
    pub fn install_quiet(self) -> Rc<RefCell<LookCtl>> {
        let rc = Rc::new(RefCell::new(self));
        CURRENT.with(|cur| *cur.borrow_mut() = Some(rc.clone()));
        rc
    }

    /// Runs `f` on the thread's controller; false when there is none or it is busy.
    pub fn with_current(f: impl FnOnce(&mut LookCtl)) -> bool {
        let rc = CURRENT.with(|c| c.borrow().clone());
        let Some(rc) = rc else { return false };
        match rc.try_borrow_mut() {
            Ok(mut ctl) => {
                f(&mut ctl);
                true
            }
            Err(_) => false,
        }
    }

    /// Lets go of the thread's controller (the window closed).
    pub fn uninstall() {
        CURRENT.with(|cur| *cur.borrow_mut() = None);
    }

    /// The override flags in force.
    pub fn overrides(&self) -> &LookOverrides {
        &self.overrides
    }

    /// The store this controller reads and writes.
    pub fn store(&self) -> &LookStore {
        &self.store
    }

    /// The light or dark of the look last applied.
    pub fn mode(&self) -> Mode {
        if self.last_light {
            Mode::Light
        } else {
            Mode::Dark
        }
    }

    /// Glass can be used at all: the window is transparent and, once attached, the build has the call.
    fn glass_possible(&self) -> bool {
        self.outcome_glass && self.material.as_ref().is_none_or(|m| m.supported())
    }

    fn glass_usable(&self) -> bool {
        self.outcome_glass
            && self
                .material
                .as_ref()
                .is_some_and(|m| m.attached() && m.supported())
    }

    /// The stored look with the one-run overrides laid over it (they write nothing).
    fn snapshot(&self) -> LookSnapshot {
        let mut s = self.store.input_snapshot();
        let o = &self.overrides;
        if let Some(t) = &o.theme {
            s.theme_id = live_theme_id(t).to_string();
        }
        if let Some(n) = o.see_through {
            s.see_through = n.min(SEE_THROUGH_TOP);
        }
        if let Some(c) = &o.accent {
            s.accent = Some(c.clone());
        }
        if let Some(c) = &o.background {
            s.background = Some(c.clone());
        }
        if let Some(c) = &o.secondary {
            s.secondary = Some(c.clone());
        }
        if let Some(e) = o.edges {
            s.edges = e;
        }
        s
    }

    /// What the window wears now, from the store and the overrides.
    pub fn current_look(&self) -> AppliedLook {
        apply_look(&self.snapshot().as_input(self.glass_usable()))
    }

    /// The same look with the ground opaque: what the window wears while the material is not glass.
    fn opaque_look(&self) -> AppliedLook {
        apply_look(&self.snapshot().as_input(false))
    }

    /// Reads the settings and wears them: the UI and the material, in the order the glass change needs.
    /// While no material is attached the window stays opaque (no translucent frame without a backdrop),
    /// and the UI wears glass only when the material does: a refused backdrop or a full screen leaves
    /// the window solid (rule 74).
    pub fn apply_now(&mut self) {
        self.queued.store(false, Ordering::SeqCst);
        let look = self.current_look();
        let now = Instant::now();
        let wanted_changed = look.glass != self.was_wanted;
        let material_first = order_for(self.was_glass, look.glass) == Order::MaterialThenUi;
        if material_first {
            self.apply_material(&look, wanted_changed, now);
        }
        let worn_glass = look.glass && self.material.as_ref().is_some_and(|m| m.glass_on());
        let worn = if look.glass && !worn_glass {
            self.opaque_look()
        } else {
            look.clone()
        };
        self.ui.apply(&worn);
        if !material_first {
            self.apply_material(&look, wanted_changed, now);
        }
        self.was_glass = worn_glass;
        self.was_wanted = look.glass;
        self.last_light = worn.light;
    }

    fn apply_material(&mut self, look: &AppliedLook, wanted_changed: bool, now: Instant) {
        if let Some(m) = self.material.as_mut() {
            m.set_solid_ground(&look.solid_ground, look.edges, now);
            if wanted_changed {
                m.set_glass(look.glass, now);
            }
        }
    }

    /// Before the window goes full screen: the UI turns solid FIRST, then the backdrop comes off (D7).
    pub fn before_fullscreen(&mut self) {
        let Some(shared) = self.shared.clone() else {
            return;
        };
        if self.was_glass {
            let opaque = self.opaque_look();
            self.ui.apply(&opaque);
            self.was_glass = false;
        }
        if let Ok(mut m) = shared.try_borrow_mut()
            && let Some(m) = m.as_mut()
        {
            m.before_fullscreen();
        }
    }

    /// The window was maximised, restored, moved or went full screen. Going full screen turns the UI
    /// solid before the backdrop leaves; when the backdrop comes back the UI follows it.
    pub fn window_state(&mut self, maximised: bool, fullscreen: bool, now: Instant) {
        let Some(shared) = self.shared.clone() else {
            return;
        };
        if fullscreen && self.was_glass {
            let opaque = self.opaque_look();
            self.ui.apply(&opaque);
            self.was_glass = false;
        }
        let after = {
            let Ok(mut m) = shared.try_borrow_mut() else {
                return;
            };
            let Some(m) = m.as_mut() else { return };
            m.on_window_state(maximised, fullscreen, now);
            m.glass_on()
        };
        if after != self.was_glass {
            self.apply_now();
        }
    }

    /// The window has a handle: keep the material and apply again (glass turning on goes backdrop first).
    pub fn attach_material(&mut self, m: WindowMaterial) {
        if let Some(shared) = self.shared.clone() {
            *shared.borrow_mut() = Some(m);
            self.attach_port(Box::new(shared));
        }
    }

    /// The same for any material port (tests attach a fake).
    pub fn attach_port(&mut self, m: Box<dyn LookMaterial>) {
        self.material = Some(m);
        self.apply_now();
    }

    /// The store changed: many calls in one turn schedule one apply.
    pub fn on_look_changed(&self) {
        (self.trigger())();
    }

    /// The store subscription's body, usable from any thread.
    pub fn trigger(&self) -> impl Fn() + Send + Sync + 'static {
        let queued = Arc::clone(&self.queued);
        let scheduler = Arc::clone(&self.scheduler);
        move || {
            if !queued.swap(true, Ordering::SeqCst) {
                scheduler();
            }
        }
    }

    // ---- theme reports ------------------------------------------------------------------------

    /// After a theme change: tell each program that asked (`?2031h`) the new mode, and for a sent report
    /// schedule the one-column nudge so a program that only re-queries on resize notices.
    pub fn push_theme(&mut self, tabs: &[Rc<dyn ThemePushTarget>], mode: Mode, now: Instant) {
        for tab in tabs {
            let (send, state) = theme_push(tab.reports(), mode);
            tab.set_reports(state);
            let Some(sent) = send else { continue };
            tab.write_report(&flare_term::replies::dsr_theme_reply(sent));
            self.nudges.push(Nudge {
                target: Rc::clone(tab),
                cols: 0,
                rows: 0,
                stage: NudgeStage::Narrow(now + Duration::from_millis(NUDGE_AFTER_MS)),
            });
        }
    }

    /// Runs the nudges that are due: one column narrower, then back after the hold.
    pub fn tick(&mut self, now: Instant) {
        let mut keep = Vec::new();
        for mut n in std::mem::take(&mut self.nudges) {
            match n.stage {
                NudgeStage::Narrow(at) if now >= at => {
                    let (cols, rows) = n.target.size();
                    if let Some((narrow, _)) = nudge_widths(cols as i64) {
                        n.cols = cols;
                        n.rows = rows;
                        n.target.resize(narrow as usize, rows);
                        n.stage = NudgeStage::Back(now + Duration::from_millis(NUDGE_HOLD_MS));
                        keep.push(n);
                    }
                }
                NudgeStage::Back(at) if now >= at => {
                    n.target.resize(n.cols, n.rows);
                }
                _ => keep.push(n),
            }
        }
        self.nudges = keep;
    }

    /// When `tick` next has something to do.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.nudges
            .iter()
            .map(|n| match n.stage {
                NudgeStage::Narrow(at) | NudgeStage::Back(at) => at,
            })
            .min()
    }

    // ---- the developer chords (decision D13) ------------------------------------------------------

    /// F9 and Shift+F9: the next or previous theme of the wall (Custom first when saved, then the host
    /// default, then the ordered rest). Writes the store; wraps.
    pub fn debug_step_theme(&mut self, dir: i32) {
        let ids = wall_ids(self.store.custom().is_some(), HOST_DEFAULT_THEME);
        let n = ids.len() as i32;
        if n == 0 {
            return;
        }
        let here = self.store.theme_id();
        let at = ids.iter().position(|i| *i == here).map_or(-1, |i| i as i32);
        let next = (at + dir.signum()).rem_euclid(n) as usize;
        self.store.set_theme_id(&ids[next]);
    }

    /// F8: the see-through level plus ten, 70 back to 0. Ignored when the window cannot show glass.
    pub fn debug_step_see_through(&mut self) {
        if !self.glass_possible() {
            log::info!("see-through: this window cannot show glass (renderer or Windows build)");
            return;
        }
        let level = self.store.see_through();
        let next = if level >= SEE_THROUGH_TOP {
            0
        } else {
            level + 10
        };
        self.store.set_see_through(next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RendererChoice;
    use crate::host::{GridPainter, GridSource};
    use crate::ui_harness::{Frame, Harness};
    use flare_render::glyph::FakeGlyphs;
    use flare_render::grid::Grid;
    use flare_settings::kv::PrefStore;
    use flare_theme::order::wall_ids;
    use flare_theme::reports::theme_reports;
    use std::cell::Cell;
    use std::sync::atomic::AtomicUsize;

    type Log = Rc<RefCell<Vec<&'static str>>>;

    struct FakeUi(Log);
    impl LookUi for FakeUi {
        fn apply(&mut self, _look: &AppliedLook) {
            self.0.borrow_mut().push("ui");
        }
    }

    struct FakeMat {
        log: Log,
        supported: bool,
        chrome_calls: Rc<Cell<usize>>,
        on: bool,
        refuse: bool,
    }
    impl LookMaterial for FakeMat {
        fn attached(&self) -> bool {
            true
        }
        fn supported(&self) -> bool {
            self.supported
        }
        fn set_solid_ground(&mut self, _hex: &str, _edges: WindowEdges, _now: Instant) {
            self.chrome_calls.set(self.chrome_calls.get() + 1);
        }
        fn set_glass(&mut self, on: bool, _now: Instant) {
            self.log.borrow_mut().push("material");
            self.on = on && !self.refuse;
        }
        fn glass_on(&self) -> bool {
            self.on
        }
    }

    fn noop_scheduler() -> Arc<dyn Fn() + Send + Sync> {
        Arc::new(|| {})
    }

    fn ctl_with(
        glass_possible: bool,
        overrides: LookOverrides,
    ) -> (LookCtl, PrefStore, LookStore, Log) {
        let prefs = PrefStore::memory();
        let store = LookStore::new(prefs.clone());
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        let c = LookCtl::with_parts(
            Box::new(FakeUi(log.clone())),
            store.clone(),
            glass_possible,
            overrides,
            noop_scheduler(),
        );
        (c, prefs, store, log)
    }

    fn fake_mat(log: &Log) -> Box<dyn LookMaterial> {
        Box::new(FakeMat {
            log: log.clone(),
            supported: true,
            chrome_calls: Rc::new(Cell::new(0)),
            on: false,
            refuse: false,
        })
    }

    #[test]
    fn overrides_beat_the_store_and_write_nothing() {
        let (mut c, _, store, _) = ctl_with(
            false,
            LookOverrides {
                theme: Some("nord".into()),
                ..LookOverrides::default()
            },
        );
        store.set_theme_id("dracula");
        assert_eq!(c.current_look().theme_id, "nord");
        c.apply_now();
        assert_eq!(c.current_look().theme_id, "nord");
        assert_eq!(store.theme_id(), "dracula");
    }

    #[test]
    fn many_changes_in_one_turn_apply_once() {
        let n = Arc::new(AtomicUsize::new(0));
        let n2 = Arc::clone(&n);
        let prefs = PrefStore::memory();
        let mut c = LookCtl::with_parts(
            Box::new(FakeUi(Rc::new(RefCell::new(Vec::new())))),
            LookStore::new(prefs),
            false,
            LookOverrides::default(),
            Arc::new(move || {
                n2.fetch_add(1, Ordering::SeqCst);
            }),
        );
        for _ in 0..5 {
            c.on_look_changed();
        }
        assert_eq!(n.load(Ordering::SeqCst), 1);
        // The apply opens the gate again.
        c.apply_now();
        c.on_look_changed();
        assert_eq!(n.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn the_order_follows_the_glass_change() {
        let (mut c, _, store, log) = ctl_with(true, LookOverrides::default());
        c.attach_port(fake_mat(&log));
        log.borrow_mut().clear();
        store.set_see_through(40);
        c.apply_now();
        assert_eq!(*log.borrow(), vec!["material", "ui"]);
        log.borrow_mut().clear();
        store.set_see_through(0);
        c.apply_now();
        assert_eq!(*log.borrow(), vec!["ui", "material"]);
        store.set_see_through(40);
        c.apply_now();
        log.borrow_mut().clear();
        store.set_see_through(20);
        c.apply_now();
        assert_eq!(*log.borrow(), vec!["ui"]);
    }

    #[test]
    fn no_glass_without_a_possible_surface() {
        let (mut c, _, store, log) = ctl_with(false, LookOverrides::default());
        c.attach_port(fake_mat(&log));
        log.borrow_mut().clear();
        store.set_see_through(40);
        c.apply_now();
        assert_eq!(*log.borrow(), vec!["ui"]);
        let look = c.current_look();
        assert!(!look.glass);
        assert_eq!(
            look.chrome.vars["--p-bg"].len(),
            7,
            "{}",
            look.chrome.vars["--p-bg"]
        );
    }

    #[test]
    fn glass_waits_for_the_window_handle() {
        let (mut c, _, store, log) = ctl_with(true, LookOverrides::default());
        store.set_see_through(40);
        c.apply_now();
        let look = c.current_look();
        assert_eq!(
            look.chrome.vars["--p-bg"].len(),
            7,
            "opaque before the handle"
        );
        assert_eq!(*log.borrow(), vec!["ui"]);
        log.borrow_mut().clear();
        c.attach_port(fake_mat(&log));
        assert_eq!(*log.borrow(), vec!["material", "ui"]);
        let look = c.current_look();
        assert_eq!(look.chrome.vars["--p-bg"].len(), 9);
        assert!(
            look.chrome.vars["--p-bg"].ends_with("99"),
            "{}",
            look.chrome.vars["--p-bg"]
        );
    }

    /// A UI that records the ground it was given.
    struct GroundUi(Rc<RefCell<Vec<String>>>);
    impl LookUi for GroundUi {
        fn apply(&mut self, look: &AppliedLook) {
            self.0.borrow_mut().push(look.chrome.vars["--p-bg"].clone());
        }
    }

    #[test]
    fn a_refused_backdrop_leaves_the_window_solid() {
        let prefs = PrefStore::memory();
        let store = LookStore::new(prefs);
        let grounds = Rc::new(RefCell::new(Vec::new()));
        let mut c = LookCtl::with_parts(
            Box::new(GroundUi(grounds.clone())),
            store.clone(),
            true,
            LookOverrides::default(),
            noop_scheduler(),
        );
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        c.attach_port(Box::new(FakeMat {
            log: log.clone(),
            supported: true,
            chrome_calls: Rc::new(Cell::new(0)),
            on: false,
            refuse: true,
        }));
        store.set_see_through(40);
        c.apply_now();
        let last = grounds.borrow().last().cloned().unwrap();
        assert_eq!(last.len(), 7, "opaque after a refusal: {last}");
        assert!(!c.was_glass);
        // Another change does not ask the OS again and stays solid.
        log.borrow_mut().clear();
        store.set_see_through(50);
        c.apply_now();
        assert!(log.borrow().is_empty(), "{:?}", log.borrow());
        assert_eq!(grounds.borrow().last().unwrap().len(), 7);
    }

    struct FakeTab {
        reports: Cell<ThemeReports>,
        written: RefCell<Vec<String>>,
        size: Cell<(usize, usize)>,
        resizes: RefCell<Vec<(usize, usize)>>,
    }

    impl FakeTab {
        fn new(on: bool, last: Mode) -> Rc<Self> {
            Rc::new(Self {
                reports: Cell::new(theme_reports(THEME_REPORTS_OFF, on, last)),
                written: RefCell::new(Vec::new()),
                size: Cell::new((80, 24)),
                resizes: RefCell::new(Vec::new()),
            })
        }
    }

    impl ThemePushTarget for FakeTab {
        fn reports(&self) -> ThemeReports {
            self.reports.get()
        }
        fn set_reports(&self, s: ThemeReports) {
            self.reports.set(s);
        }
        fn write_report(&self, bytes: &str) {
            self.written.borrow_mut().push(bytes.to_string());
        }
        fn size(&self) -> (usize, usize) {
            self.size.get()
        }
        fn resize(&self, cols: usize, rows: usize) {
            self.resizes.borrow_mut().push((cols, rows));
            self.size.set((cols, rows));
        }
    }

    #[test]
    fn a_mode_change_pushes_once_then_nudges_one_column_and_back() {
        let (mut c, _, _, _) = ctl_with(false, LookOverrides::default());
        let tab = FakeTab::new(true, Mode::Dark);
        let dynamic: Rc<dyn ThemePushTarget> = tab.clone();
        let t0 = Instant::now();
        c.push_theme(&[dynamic], Mode::Light, t0);
        assert_eq!(*tab.written.borrow(), vec!["\x1b[?997;2n".to_string()]);
        assert!(tab.resizes.borrow().is_empty());
        c.tick(t0 + Duration::from_millis(249));
        assert!(tab.resizes.borrow().is_empty());
        c.tick(t0 + Duration::from_millis(250));
        assert_eq!(*tab.resizes.borrow(), vec![(79, 24)]);
        c.tick(t0 + Duration::from_millis(329));
        assert_eq!(tab.resizes.borrow().len(), 1);
        c.tick(t0 + Duration::from_millis(330));
        assert_eq!(*tab.resizes.borrow(), vec![(79, 24), (80, 24)]);
        assert!(c.next_deadline().is_none());
    }

    #[test]
    fn no_push_while_reports_are_off_and_none_for_the_same_mode() {
        let (mut c, _, _, _) = ctl_with(false, LookOverrides::default());
        let t0 = Instant::now();
        let none: Rc<dyn ThemePushTarget> = Rc::new(NoReports);
        c.push_theme(&[none], Mode::Light, t0);
        assert!(c.next_deadline().is_none());
        let tab = FakeTab::new(true, Mode::Dark);
        let dynamic: Rc<dyn ThemePushTarget> = tab.clone();
        c.push_theme(&[dynamic], Mode::Dark, t0);
        assert!(tab.written.borrow().is_empty());
        assert!(c.next_deadline().is_none());
    }

    fn with_custom(prefs: &PrefStore) {
        prefs.set(
            flare_settings::custom_theme::CUSTOM_KEY,
            r##"{"bg":"#111111","fg":"#eeeeee","cursor":"#ffffff","ansi":{}}"##,
        );
    }

    #[test]
    fn f9_walks_the_wall_and_wraps() {
        let (mut c, prefs, store, _) = ctl_with(false, LookOverrides::default());
        with_custom(&prefs);
        assert!(store.custom().is_some());
        let wall = wall_ids(true, "pt-default");
        assert_eq!(wall.len(), 40);
        assert_eq!(wall[0], "custom");
        assert_eq!(wall[1], "pt-default");
        assert_eq!(store.theme_id(), "pt-default");
        for i in 0..40 {
            c.debug_step_theme(1);
            assert_eq!(store.theme_id(), wall[(2 + i) % 40], "press {i}");
        }
        assert_eq!(store.theme_id(), "pt-default");
        c.debug_step_theme(-1);
        assert_eq!(store.theme_id(), "custom");
        c.debug_step_theme(-1);
        assert_eq!(store.theme_id(), "butter");
        assert_eq!(wall.last().map(String::as_str), Some("butter"));
    }

    #[test]
    fn f8_steps_the_see_through_by_ten_and_wraps() {
        let (mut c, _, store, _) = ctl_with(true, LookOverrides::default());
        for want in [10u8, 20, 30, 40, 50, 60, 70, 0] {
            c.debug_step_see_through();
            assert_eq!(store.see_through(), want);
        }
        let (mut c, _, store, _) = ctl_with(false, LookOverrides::default());
        c.debug_step_see_through();
        assert_eq!(store.see_through(), 0);
    }

    fn outcome() -> BackendOutcome {
        BackendOutcome {
            effective: RendererChoice::Software,
            fallback_reason: None,
            glass_possible: false,
        }
    }

    #[derive(Default)]
    struct BlankSource {
        cols: usize,
        rows: usize,
    }

    impl GridSource for BlankSource {
        fn resize(&mut self, cols: usize, rows: usize) {
            self.cols = cols;
            self.rows = rows;
        }
        fn grid(&mut self) -> Grid {
            Grid::new(self.cols, self.rows)
        }
    }

    #[test]
    fn the_first_frame_wears_the_saved_theme() {
        for scale in [1.0_f32, 1.5, 2.25] {
            let h = Harness::new(scale, 1000, 640, || {
                AppWindow::new().expect("the window builds")
            });
            let store = LookStore::new(PrefStore::memory());
            store.set_theme_id("paper");
            let host = Rc::new(RefCell::new(GridHost::new(
                Box::new(FakeGlyphs::new(8, 16)),
                GridPainter::Cpu,
            )));
            host.borrow_mut()
                .set_source(Box::new(BlankSource::default()));
            let mut ctl = LookCtl::new(
                &h.comp,
                store,
                host.clone(),
                Rc::new(RefCell::new(None)),
                &outcome(),
                LookOverrides::default(),
            );
            ctl.apply_now();
            let (gw, gh) = (
                h.comp.get_grid_phys_width() as u32,
                h.comp.get_grid_phys_height() as u32,
            );
            host.borrow_mut().resize(gw, gh, scale);
            if let Some(img) = host.borrow_mut().render() {
                h.comp.set_grid_image(img);
            }
            let frame = h.render();
            let mid = frame.px(frame.w / 2, frame.h / 2);
            assert!(
                Frame::near(mid, [0xf6, 0xf4, 0xee, 255], 2),
                "scale {scale}: {mid:?}"
            );
        }
    }

    #[test]
    fn the_debug_keys_work_only_with_the_overlay_open() {
        let h = Harness::new(1.0, 1000, 640, || {
            AppWindow::new().expect("the window builds")
        });
        let themes = Rc::new(Cell::new(0));
        let levels = Rc::new(Cell::new(0));
        h.comp.on_debug_step_theme({
            let themes = themes.clone();
            move |dir| themes.set(themes.get() + dir)
        });
        h.comp.on_debug_step_see_through({
            let levels = levels.clone();
            move || levels.set(levels.get() + 1)
        });
        let f9: slint::SharedString = slint::platform::Key::F9.into();
        let f8: slint::SharedString = slint::platform::Key::F8.into();
        h.key_text(&f9);
        h.key_text(&f8);
        assert_eq!((themes.get(), levels.get()), (0, 0));
        h.comp.set_overlay_visible(true);
        h.key_text(&f9);
        h.key_text(&f8);
        assert_eq!((themes.get(), levels.get()), (1, 1));
    }
}
