//! The sidebar controller: reveal, slide, grip (Task 28).
//!
//! Source: Prism Terminal's `TabSidebar.tsx` (reveal, the slide, the resize grip) and
//! `sidebarPrefs.ts`. Pure glue over `flare-tabs` (`SidebarState`, `Reveal`, `FitHold`) and the
//! stored prefs; time is passed in, never read.
#![allow(dead_code)] // the controller (Task 30) is the caller

use flare_render::device_pixels::on_device_pixels;
use flare_settings::kv::PrefStore;
use flare_settings::sidebar::{
    SidebarMode, set_sidebar_mode_stored, set_sidebar_width, sidebar_mode, sidebar_width,
    valid_sidebar_width_num,
};
use flare_tabs::fit_hold::FitHold;
use flare_tabs::reveal::{Point, Reveal, side_wants};
use flare_tabs::sidebar_state::{SLIDE_SETTLE_MS, SidebarState};

/// The height of the bar above the sidebar: the reveal zone starts below it.
const BAR_H: f64 = 36.0;

/// The fit waiter's id: the terminals are fitted once when the hold ends.
const FIT_ID: u32 = 0;

/// What the window draws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SidebarView {
    pub collapsed: bool,
    pub revealed: bool,
    pub width_css: f64,
    pub sliding: bool,
    /// The terminal keeps its size while the sidebar slides.
    pub frozen: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarEvent {
    Revealed(bool),
    /// The hold ended: resize the terminals once.
    FitDue,
}

pub struct SidebarCtl {
    state: SidebarState,
    reveal: Reveal,
    fit: FitHold,
    fit_due: Option<u64>,
    stored_w: f64,
    drag_w: Option<f64>,
    finding: bool,
    menu: bool,
    dragging: bool,
    queued: Vec<SidebarEvent>,
    /// The latest time any call carried: the grip calls have no clock of their own.
    last_now: u64,
    /// The pointer as the last move left it, so letting go of the grip can judge it again.
    last_pointer: Option<Point>,
}

impl SidebarCtl {
    pub fn new(prefs: &PrefStore, now: u64) -> Self {
        Self {
            state: SidebarState::new(sidebar_mode(prefs)),
            reveal: Reveal::new(),
            fit: FitHold::new(),
            fit_due: None,
            stored_w: f64::from(sidebar_width(prefs)),
            drag_w: None,
            finding: false,
            menu: false,
            dragging: false,
            queued: Vec::new(),
            last_now: now,
            last_pointer: None,
        }
    }

    fn collapsed(&self) -> bool {
        self.state.mode() == SidebarMode::Collapsed
    }

    fn width(&self) -> f64 {
        self.drag_w.unwrap_or(self.stored_w)
    }

    pub fn view(&self, scale: f64, now: u64) -> SidebarView {
        let sliding = self.state.sliding(now);
        SidebarView {
            collapsed: self.collapsed(),
            revealed: self.collapsed() && self.reveal.shown(),
            width_css: on_device_pixels(self.width(), scale),
            sliding,
            frozen: self.fit.held(now) && sliding,
        }
    }

    /// Flips the stored mode; the sidebar slides and the terminals' fit is held for the slide.
    pub fn toggle(&mut self, prefs: &PrefStore, now: u64) {
        self.last_now = now;
        let mode = self.state.toggle(now, &mut self.fit);
        set_sidebar_mode_stored(prefs, mode);
        self.fit.when_released(FIT_ID);
        self.fit_due = Some(now + SLIDE_SETTLE_MS);
        if mode == SidebarMode::Open && self.reveal.hide() == Some(false) {
            self.queued.push(SidebarEvent::Revealed(false));
        }
    }

    /// The pointer in window css coordinates (None: it left the window). Collapsed only.
    pub fn pointer(&mut self, p: Option<Point>, now: u64) {
        self.last_now = now;
        self.last_pointer = p;
        if !self.collapsed() {
            return;
        }
        let wants = side_wants(p, self.reveal.shown(), BAR_H, self.width());
        self.reveal.feel(wants, now);
    }

    /// Things that keep a revealed sidebar open: the find box, a menu, a grip drag.
    pub fn hold(&mut self, finding: bool, menu_open: bool, now: u64) {
        self.last_now = now;
        self.finding = finding;
        self.menu = menu_open;
        self.apply_hold(now);
    }

    fn apply_hold(&mut self, now: u64) {
        self.reveal
            .hold(self.finding || self.menu || self.dragging, now);
    }

    pub fn tick(&mut self, now: u64) -> Vec<SidebarEvent> {
        let mut out = std::mem::take(&mut self.queued);
        if self.collapsed()
            && let Some(v) = self.reveal.poll(now)
        {
            out.push(SidebarEvent::Revealed(v));
        }
        if !self.fit.poll(now).is_empty() {
            self.fit_due = None;
            out.push(SidebarEvent::FitDue);
        }
        out
    }

    /// The live width while the grip moves: clamped and whole.
    pub fn grip_moved(&mut self, proposed_css: f64) -> f64 {
        let w = f64::from(valid_sidebar_width_num(proposed_css));
        self.drag_w = Some(w);
        if !self.dragging {
            self.dragging = true;
            self.reveal.hold(true, self.last_now);
        }
        w
    }

    pub fn grip_released(&mut self, prefs: &PrefStore, final_css: f64) {
        set_sidebar_width(prefs, final_css);
        self.stored_w = f64::from(valid_sidebar_width_num(final_css));
        self.drag_w = None;
        self.dragging = false;
        self.apply_hold(self.last_now);
        // The last move was judged against the width the grip had before it moved; the pointer that
        // sits on the new edge is judged again now, or the sidebar leaves the moment it is resized.
        if self.collapsed() {
            let wants = side_wants(self.last_pointer, self.reveal.shown(), BAR_H, self.width());
            self.reveal.feel(wants, self.last_now);
        }
    }

    /// When the next `tick` has something to do.
    pub fn next_due(&self) -> Option<u64> {
        [self.reveal.next_due(), self.fit_due]
            .into_iter()
            .flatten()
            .min()
    }
}

/// The cursor (physical, screen) in the window's css coordinates, for the edge watch.
pub fn cursor_to_window_css(
    cursor_phys: (i32, i32),
    origin_phys: (i32, i32),
    scale: f64,
) -> (f64, f64) {
    (
        f64::from(cursor_phys.0 - origin_phys.0) / scale,
        f64::from(cursor_phys.1 - origin_phys.1) / scale,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: f64, y: f64) -> Option<Point> {
        Some(Point { x, y })
    }

    fn collapsed_ctl() -> (SidebarCtl, PrefStore) {
        let prefs = PrefStore::memory();
        set_sidebar_mode_stored(&prefs, SidebarMode::Collapsed);
        (SidebarCtl::new(&prefs, 0), prefs)
    }

    fn revealed_ctl() -> (SidebarCtl, PrefStore) {
        let (mut c, p) = collapsed_ctl();
        c.pointer(pt(10.0, 300.0), 0);
        assert_eq!(c.tick(60), vec![SidebarEvent::Revealed(true)]);
        (c, p)
    }

    #[test]
    fn a_collapsed_sidebar_reveals_after_sixty_ms_at_the_left_edge() {
        let (mut c, _p) = collapsed_ctl();
        c.pointer(pt(10.0, 300.0), 0);
        assert!(c.tick(59).is_empty());
        assert_eq!(c.tick(60), vec![SidebarEvent::Revealed(true)]);
        assert!(c.view(1.0, 60).revealed);
    }

    #[test]
    fn it_goes_after_the_grace_when_the_pointer_leaves() {
        let (mut c, _p) = revealed_ctl();
        c.pointer(pt(500.0, 300.0), 100);
        assert!(c.tick(199).is_empty());
        assert_eq!(c.tick(200), vec![SidebarEvent::Revealed(false)]);
        assert!(!c.view(1.0, 200).revealed);
    }

    #[test]
    fn a_menu_or_the_find_box_or_a_drag_holds_it_open() {
        let (mut c, _p) = revealed_ctl();
        c.hold(true, false, 100);
        c.pointer(pt(500.0, 300.0), 100);
        assert!(c.tick(5000).is_empty());
        c.hold(false, false, 5000);
        assert!(c.tick(5099).is_empty());
        assert_eq!(c.tick(5100), vec![SidebarEvent::Revealed(false)]);

        let (mut c, _p) = revealed_ctl();
        c.hold(false, true, 100);
        c.pointer(pt(500.0, 300.0), 100);
        assert!(c.tick(5000).is_empty());

        let (mut c, p) = revealed_ctl();
        c.grip_moved(300.0);
        c.pointer(pt(500.0, 300.0), 100);
        assert!(c.tick(5000).is_empty());
        c.grip_released(&p, 300.0);
        assert_eq!(c.tick(5100), vec![SidebarEvent::Revealed(false)]);
    }

    // Task 23 (verticalTabs): the cursor event is judged against the width the grip had BEFORE it
    // moved, so a quick drag ends with the pointer judged "past the edge" even though it sits on the
    // new edge. Letting go must judge it again, or the slid-out sidebar leaves the moment it is resized.
    #[test]
    fn letting_go_of_the_grip_at_the_new_edge_keeps_a_revealed_sidebar_out() {
        let (mut c, p) = revealed_ctl();
        c.grip_moved(255.0);
        c.pointer(pt(262.0, 300.0), 100);
        c.grip_moved(262.0);
        c.grip_released(&p, 262.0);
        assert!(c.tick(10_000).is_empty());
        assert!(c.view(1.0, 10_000).revealed);
    }

    #[test]
    fn letting_go_of_the_grip_far_from_the_sidebar_still_lets_it_go() {
        let (mut c, p) = revealed_ctl();
        c.grip_moved(300.0);
        c.pointer(pt(500.0, 300.0), 100);
        c.grip_released(&p, 300.0);
        assert_eq!(c.tick(5100), vec![SidebarEvent::Revealed(false)]);
    }

    #[test]
    fn an_open_sidebar_ignores_the_pointer() {
        let prefs = PrefStore::memory();
        let mut c = SidebarCtl::new(&prefs, 0);
        c.pointer(pt(10.0, 300.0), 0);
        assert!(c.tick(1000).is_empty());
        assert!(!c.view(1.0, 1000).revealed);
        assert_eq!(c.next_due(), None);
    }

    #[test]
    fn toggle_slides_holds_the_fit_and_stores_the_mode() {
        let prefs = PrefStore::memory();
        let mut c = SidebarCtl::new(&prefs, 0);
        c.toggle(&prefs, 0);
        let v = c.view(1.0, 0);
        assert!(v.collapsed && v.sliding && v.frozen);
        assert_eq!(
            prefs.get("flare.window.sidebar"),
            Some("collapsed".to_string())
        );
        assert!(c.tick(279).is_empty());
        assert!(c.tick(280).contains(&SidebarEvent::FitDue));
        assert!(!c.view(1.0, 280).sliding);
    }

    #[test]
    fn a_toggle_mid_slide_restarts_it() {
        let prefs = PrefStore::memory();
        let mut c = SidebarCtl::new(&prefs, 0);
        c.toggle(&prefs, 0);
        c.toggle(&prefs, 100);
        assert!(c.view(1.0, 300).sliding);
        assert!(!c.tick(300).contains(&SidebarEvent::FitDue));
        assert!(!c.tick(379).contains(&SidebarEvent::FitDue));
        assert!(c.tick(380).contains(&SidebarEvent::FitDue));
    }

    #[test]
    fn revealing_a_collapsed_sidebar_does_not_freeze_the_terminal() {
        let (mut c, _p) = collapsed_ctl();
        c.pointer(pt(10.0, 300.0), 0);
        assert_eq!(c.next_due(), Some(60));
        let ev = c.tick(60);
        assert!(!ev.contains(&SidebarEvent::FitDue));
        assert!(!c.view(1.0, 60).frozen);
        assert!(c.tick(10_000).is_empty());
    }

    #[test]
    fn the_grip_clamps_live_and_stores_on_release() {
        let prefs = PrefStore::memory();
        let mut c = SidebarCtl::new(&prefs, 0);
        assert_eq!(c.grip_moved(100.0), 150.0);
        assert_eq!(c.grip_moved(500.0), 420.0);
        assert_eq!(c.grip_moved(260.6), 261.0);
        assert_eq!(c.view(1.0, 0).width_css, 261.0);
        c.grip_released(&prefs, 261.0);
        assert_eq!(
            prefs.get("flare.window.sidebarWidth"),
            Some("261".to_string())
        );
        assert_eq!(c.view(1.0, 0).width_css, 261.0);
    }

    #[test]
    fn the_width_snaps_to_a_whole_device_pixel() {
        let prefs = PrefStore::memory();
        prefs.set("flare.window.sidebarWidth", "235");
        let c = SidebarCtl::new(&prefs, 0);
        let w = c.view(1.25, 0).width_css;
        assert_eq!(w * 1.25, 294.0);
        for s in [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 3.0] {
            let d = c.view(s, 0).width_css * s;
            assert!((d - d.round()).abs() < 1e-9, "scale {s}: {d}");
        }
    }

    #[test]
    fn the_cursor_is_converted_to_window_css() {
        let (x, y) = cursor_to_window_css((1130, 700), (100, 100), 2.25);
        assert!((x - 457.77).abs() < 0.01, "{x}");
        assert!((y - 266.66).abs() < 0.01, "{y}");
    }
}
