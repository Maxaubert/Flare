//! The window frame controller (Task 25): the window edge and the dark or light frame.
//!
//! Owns the 250 ms border applier over the window's handle and the dark or light frame that follows the
//! theme's solid ground (DWM's immersive dark mode on Windows, the Aqua appearance on macOS). Every
//! call is cosmetic (rule 74): a refusal leaves the window as it was and never panics.
//!
//! Until 2026-11-28 this was the window material, which also drove the see-through backdrop (acrylic
//! on Windows, an `NSVisualEffectView` on macOS). That window was removed; every window is solid with
//! its theme ground. Liquid Glass on the Mac comes later, as its own feature.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use flare_theme::edges::WindowEdges;
use flare_theme::ground::DEFAULT_GROUND;
use flare_win::dwm::{self, Border, Hwnd};
use flare_win::window_edge::{EdgeApplier, EdgeTarget, is_light_bg, valid_bg};

/// The border colour as Slint's `Theme.edge` wants it: RGBA bytes, `None` for no edge.
pub fn edge_rgba(b: &Border) -> Option<[u8; 4]> {
    match b {
        Border::Colour(hex) if valid_bg(hex) => {
            let n = u32::from_str_radix(&hex[1..], 16).ok()?;
            Some([(n >> 16) as u8, (n >> 8) as u8, n as u8, 255])
        }
        _ => None,
    }
}

/// The window edge: the DWM border call plus the Slint-drawn frame (the DWM border does not show on a
/// frameless window: the material spike's E4, 2026-10-17).
pub struct DwmEdgeTarget {
    hwnd: Hwnd,
    flat: Rc<Cell<bool>>,
    on_edge: Box<dyn Fn(Option<[u8; 4]>)>,
}

impl EdgeTarget for DwmEdgeTarget {
    fn set_border(&mut self, b: &Border) {
        // Cosmetic: a refused call is ignored, the Slint frame still draws.
        if self.hwnd != 0 {
            // sync-ok: DWM call, returns in microseconds
            dwm::set_border(self.hwnd, b);
        }
        (self.on_edge)(edge_rgba(b));
    }
    fn is_flat(&self) -> bool {
        self.flat.get()
    }
}

/// Wakes the UI thread once, when the frame next has something to do. The frame arms it after every
/// change and disarms it when nothing is due, so an idle window runs no timer at all (the border wait is
/// the only timed work).
pub trait Wake {
    /// Fire once after `after`; a later call replaces the earlier one.
    fn wake_in(&self, after: Duration);
    /// Nothing is due: no wake.
    fn cancel(&self);
}

/// The live wake: one Slint single-shot timer that ticks the window's shared frame.
pub struct SlintWake {
    timer: slint::Timer,
    fire: Rc<dyn Fn()>,
}

impl SlintWake {
    /// Ticks the frame in `slot` when it fires; holds the slot weakly (the frame owns this).
    pub fn new(slot: Weak<RefCell<Option<WindowFrame>>>) -> Self {
        let fire: Rc<dyn Fn()> = Rc::new(move || fire_frame(&slot));
        Self {
            timer: slint::Timer::default(),
            fire,
        }
    }
}

fn fire_frame(slot: &Weak<RefCell<Option<WindowFrame>>>) {
    let Some(rc) = slot.upgrade() else { return };
    match rc.try_borrow_mut() {
        // The tick arms the next wake itself, or disarms it.
        Ok(mut m) => {
            if let Some(m) = m.as_mut() {
                m.tick(Instant::now());
            }
        }
        // Busy (a re-entrant moment): look again on the next frame rather than lose the border.
        Err(_) => {
            let slot = slot.clone();
            slint::Timer::single_shot(Duration::from_millis(16), move || fire_frame(&slot));
        }
    }
}

impl Wake for SlintWake {
    fn wake_in(&self, after: Duration) {
        let fire = self.fire.clone();
        self.timer
            .start(slint::TimerMode::SingleShot, after, move || fire());
    }
    fn cancel(&self) {
        self.timer.stop();
    }
}

pub struct WindowFrame {
    edge: EdgeApplier<DwmEdgeTarget>,
    bg: Rc<RefCell<String>>,
    edges: Rc<Cell<WindowEdges>>,
    flat: Rc<Cell<bool>>,
    /// The dark (true) or light frame last asked for; None before the first ground.
    dark: Option<bool>,
    on_dark: Box<dyn Fn(bool)>,
    wake: Option<Box<dyn Wake>>,
}

impl WindowFrame {
    /// The controller for this window. `None` when the window has no native handle yet.
    pub fn new(app: &crate::AppWindow, e2e: bool) -> Option<Self> {
        use slint::Global;
        let hwnd = crate::window::native_view_of(app)?;
        let weak = slint::ComponentHandle::as_weak(app);
        let on_edge = move |rgba: Option<[u8; 4]>| {
            let Some(app) = weak.upgrade() else { return };
            let c = match rgba {
                Some([r, g, b, a]) => slint::Color::from_argb_u8(a, r, g, b),
                None => slint::Color::from_argb_u8(0, 0, 0, 0),
            };
            crate::Theme::get(&app).set_edge(c);
        };
        Some(Self::with_hwnd(hwnd, e2e, on_edge))
    }

    /// The same without a Slint window (tests; a handle of 0 makes no DWM call).
    pub fn with_hwnd(hwnd: Hwnd, e2e: bool, on_edge: impl Fn(Option<[u8; 4]>) + 'static) -> Self {
        let on_dark = move |dark: bool| {
            if hwnd != 0 {
                // Cosmetic (rule 74): a refusal leaves the frame as it was.
                // sync-ok: DWM call, returns in microseconds
                dwm::set_immersive_dark(hwnd, dark);
            }
        };
        Self::from_parts(hwnd, !e2e, Box::new(on_edge), Box::new(on_dark))
    }

    /// The assembled controller; `edge_enabled` false (under `--e2e`) means the border is never applied
    /// and no deadline is ever set.
    pub fn from_parts(
        hwnd: Hwnd,
        edge_enabled: bool,
        on_edge: Box<dyn Fn(Option<[u8; 4]>)>,
        on_dark: Box<dyn Fn(bool)>,
    ) -> Self {
        let bg = Rc::new(RefCell::new(DEFAULT_GROUND.to_string()));
        let edges = Rc::new(Cell::new(WindowEdges::Hairline));
        let flat = Rc::new(Cell::new(false));
        let target = DwmEdgeTarget {
            hwnd,
            flat: flat.clone(),
            on_edge,
        };
        let edge = EdgeApplier::new(
            target,
            {
                let bg = bg.clone();
                move || bg.borrow().clone()
            },
            {
                let edges = edges.clone();
                move || edges.get()
            },
            edge_enabled,
        );
        Self {
            edge,
            bg,
            edges,
            flat,
            dark: None,
            on_dark,
            wake: None,
        }
    }

    /// Who wakes the UI thread when the border is due; armed now if something already is.
    pub fn set_wake(&mut self, wake: Box<dyn Wake>, now: Instant) {
        self.wake = Some(wake);
        self.rearm(now);
    }

    /// Arms the wake for the next deadline, or disarms it when nothing is due.
    fn rearm(&self, now: Instant) {
        let Some(w) = &self.wake else { return };
        match self.next_deadline() {
            Some(due) => w.wake_in(due.saturating_duration_since(now)),
            None => w.cancel(),
        }
    }

    /// The solid ground the border and the dark or light frame follow. Anything but `#rrggbb` leaves the
    /// ground as it was; the frame is asked again only when its light or dark changes.
    pub fn set_solid_ground(&mut self, hex: &str, edges: WindowEdges, now: Instant) {
        if valid_bg(hex) {
            *self.bg.borrow_mut() = hex.to_string();
            let dark = !is_light_bg(hex);
            if self.dark != Some(dark) {
                self.dark = Some(dark);
                (self.on_dark)(dark);
            }
        }
        self.edges.set(edges);
        self.edge.apply(now);
        self.rearm(now);
    }

    /// The window state settled (maximised / full screen / restored): the edge is flat while either holds.
    pub fn on_window_state(&mut self, maximised: bool, fullscreen: bool, now: Instant) {
        self.flat.set(maximised || fullscreen);
        self.edge.apply(now);
        self.rearm(now);
    }

    pub fn tick(&mut self, now: Instant) {
        self.edge.tick(now);
        self.rearm(now);
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.edge.next_deadline()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    type Darks = Rc<RefCell<Vec<bool>>>;

    fn frame(edge_enabled: bool) -> (WindowFrame, Darks) {
        let darks: Darks = Rc::new(RefCell::new(Vec::new()));
        let d2 = darks.clone();
        let m = WindowFrame::from_parts(
            0,
            edge_enabled,
            Box::new(|_| {}),
            Box::new(move |d| d2.borrow_mut().push(d)),
        );
        (m, darks)
    }

    #[test]
    fn the_frame_follows_the_grounds_light_or_dark_and_is_asked_only_on_a_change() {
        let (mut m, darks) = frame(true);
        let t0 = Instant::now();
        m.set_solid_ground("#0b0b0f", WindowEdges::Hairline, t0);
        m.set_solid_ground("#171717", WindowEdges::Hairline, t0);
        assert_eq!(*darks.borrow(), vec![true]);
        m.set_solid_ground("#fdf6e3", WindowEdges::Hairline, t0);
        assert_eq!(*darks.borrow(), vec![true, false]);
        // Anything but #rrggbb changes nothing.
        for bad in ["javascript:1", "#fff", "#aabbcc80"] {
            m.set_solid_ground(bad, WindowEdges::Hairline, t0);
        }
        assert_eq!(*darks.borrow(), vec![true, false]);
        assert_eq!(*m.bg.borrow(), "#fdf6e3");
    }

    #[test]
    fn e2e_disables_the_border() {
        let (mut m, _) = frame(false);
        let t0 = Instant::now();
        m.set_solid_ground("#0b0b0f", WindowEdges::Hairline, t0);
        m.on_window_state(false, false, t0);
        assert_eq!(m.next_deadline(), None);
        m.tick(t0 + Duration::from_secs(1));
        let (mut m, _) = frame(true);
        m.set_solid_ground("#0b0b0f", WindowEdges::Hairline, t0);
        assert_eq!(m.next_deadline(), Some(t0 + Duration::from_millis(250)));
    }

    #[test]
    fn the_border_reaches_the_slint_frame_after_the_wait() {
        let seen: Rc<RefCell<Vec<Option<[u8; 4]>>>> = Rc::new(RefCell::new(Vec::new()));
        let s2 = seen.clone();
        let mut m = WindowFrame::from_parts(
            0,
            true,
            Box::new(move |c| s2.borrow_mut().push(c)),
            Box::new(|_| {}),
        );
        let t0 = Instant::now();
        m.set_solid_ground("#0b0b0f", WindowEdges::Hairline, t0);
        m.tick(t0 + Duration::from_millis(250));
        let got = seen.borrow().clone();
        assert_eq!(got.len(), 1);
        let c = got[0].expect("an edge colour");
        assert_eq!(c[3], 255);
        // Maximised: no edge.
        m.on_window_state(true, false, t0);
        m.tick(t0 + Duration::from_millis(250));
        assert_eq!(seen.borrow().last(), Some(&None));
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum W {
        In(Duration),
        Cancel,
    }

    struct FakeWake(Rc<RefCell<Vec<W>>>);

    impl Wake for FakeWake {
        fn wake_in(&self, after: Duration) {
            self.0.borrow_mut().push(W::In(after));
        }
        fn cancel(&self) {
            self.0.borrow_mut().push(W::Cancel);
        }
    }

    #[test]
    fn the_wake_runs_only_while_the_border_is_due() {
        let (mut m, _) = frame(true);
        let wakes = Rc::new(RefCell::new(Vec::new()));
        let t0 = Instant::now();
        m.set_wake(Box::new(FakeWake(wakes.clone())), t0);
        // Nothing due: no timer at all.
        assert_eq!(*wakes.borrow(), vec![W::Cancel]);
        wakes.borrow_mut().clear();
        m.set_solid_ground("#0b0b0f", WindowEdges::Hairline, t0);
        assert_eq!(*wakes.borrow(), vec![W::In(Duration::from_millis(250))]);
        // A change part way restarts the wait from then (the debounce).
        wakes.borrow_mut().clear();
        let t1 = t0 + Duration::from_millis(100);
        m.on_window_state(false, false, t1);
        assert_eq!(*wakes.borrow(), vec![W::In(Duration::from_millis(250))]);
        // An early fire keeps waiting for what is left.
        wakes.borrow_mut().clear();
        m.tick(t1 + Duration::from_millis(200));
        assert_eq!(*wakes.borrow(), vec![W::In(Duration::from_millis(50))]);
        // The border applied: the wake is disarmed and stays so while idle.
        wakes.borrow_mut().clear();
        m.tick(t1 + Duration::from_millis(250));
        assert_eq!(*wakes.borrow(), vec![W::Cancel]);
        assert_eq!(m.next_deadline(), None);
    }

    #[test]
    fn under_e2e_the_wake_is_never_armed() {
        let (mut m, _) = frame(false);
        let wakes = Rc::new(RefCell::new(Vec::new()));
        let t0 = Instant::now();
        m.set_wake(Box::new(FakeWake(wakes.clone())), t0);
        m.set_solid_ground("#0b0b0f", WindowEdges::Hairline, t0);
        m.on_window_state(true, false, t0);
        m.tick(t0 + Duration::from_secs(1));
        assert!(
            wakes.borrow().iter().all(|w| *w == W::Cancel),
            "{:?}",
            wakes.borrow()
        );
    }

    #[test]
    fn edge_rgba_reads_rrggbb_only() {
        assert_eq!(
            edge_rgba(&Border::Colour("#2b2b2e".into())),
            Some([0x2b, 0x2b, 0x2e, 255])
        );
        assert_eq!(edge_rgba(&Border::None), None);
        assert_eq!(edge_rgba(&Border::Default), None);
        assert_eq!(edge_rgba(&Border::Colour("red".into())), None);
    }
}
