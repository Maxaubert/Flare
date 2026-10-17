//! The WindowMaterial controller that drives DWM (Task 25).
//!
//! Owns the material state machine over the window's `DwmSurface`, the 250 ms border applier and
//! the order of operations when glass turns on or off (decision D7): going glass calls the
//! backdrop FIRST and only then lowers the UI's ground; leaving glass raises the UI to opaque
//! FIRST and only then removes the backdrop. Every DWM call is cosmetic (rule 74): a refusal
//! leaves the solid window and never panics.

// LookCtl (Task 26) drives set_glass, before_fullscreen and the order helpers.
#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

use flare_theme::edges::WindowEdges;
use flare_theme::ground::DEFAULT_GROUND;
use flare_win::dwm::{self, Border, DwmSurface, Hwnd};
use flare_win::material::{Material, Surface, system_backdrop_ok};
use flare_win::os_build::OsVersion;
use flare_win::window_edge::{EdgeApplier, EdgeTarget};

/// Which side of a glass change goes first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    /// The UI changes, then the material (leaving glass, or no change of glass).
    UiThenMaterial,
    /// The material changes, then the UI (glass turns ON): the backdrop is there before the
    /// ground gets translucent, so no frame shows a see-through ground over nothing.
    MaterialThenUi,
}

pub fn order_for(was_glass: bool, now_glass: bool) -> Order {
    if !was_glass && now_glass {
        Order::MaterialThenUi
    } else {
        Order::UiThenMaterial
    }
}

/// Runs `ui` and `material` in the order the glass change needs.
pub fn apply_in_order(order: Order, ui: impl FnOnce(), material: impl FnOnce()) {
    match order {
        Order::MaterialThenUi => {
            material();
            ui();
        }
        Order::UiThenMaterial => {
            ui();
            material();
        }
    }
}

/// A material surface that also learns when the window is full screen.
pub trait FsSurface: Surface {
    fn set_fullscreen(&mut self, on: bool);
}

impl FsSurface for DwmSurface {
    fn set_fullscreen(&mut self, on: bool) {
        DwmSurface::set_fullscreen(self, on);
    }
}

/// The border colour as Slint's `Theme.edge` wants it: RGBA bytes, `None` for no edge.
pub fn edge_rgba(b: &Border) -> Option<[u8; 4]> {
    match b {
        Border::Colour(hex) if flare_win::material::valid_bg(hex) => {
            let n = u32::from_str_radix(&hex[1..], 16).ok()?;
            Some([(n >> 16) as u8, (n >> 8) as u8, n as u8, 255])
        }
        _ => None,
    }
}

/// The window edge: the DWM border call plus the Slint-drawn frame (the DWM border does not
/// show on a frameless window, docs/material.md E4).
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

pub struct WindowMaterial<S: FsSurface = DwmSurface> {
    material: Material<S>,
    edge: EdgeApplier<DwmEdgeTarget>,
    supported: bool,
    bg: Rc<RefCell<String>>,
    edges: Rc<Cell<WindowEdges>>,
    flat: Rc<Cell<bool>>,
}

impl WindowMaterial<DwmSurface> {
    /// The controller for this window. `None` when the window has no Win32 handle yet.
    pub fn new(app: &crate::AppWindow, os: OsVersion, e2e: bool) -> Option<Self> {
        use slint::Global;
        let hwnd = hwnd_of(app)?;
        let weak = slint::ComponentHandle::as_weak(app);
        let on_edge = move |rgba: Option<[u8; 4]>| {
            let Some(app) = weak.upgrade() else { return };
            let c = match rgba {
                Some([r, g, b, a]) => slint::Color::from_argb_u8(a, r, g, b),
                None => slint::Color::from_argb_u8(0, 0, 0, 0),
            };
            crate::Theme::get(&app).set_edge(c);
        };
        Some(Self::with_hwnd(hwnd, os, e2e, on_edge))
    }

    /// The same without a Slint window (tests; a handle of 0 makes no DWM call).
    pub fn with_hwnd(
        hwnd: Hwnd,
        os: OsVersion,
        e2e: bool,
        on_edge: impl Fn(Option<[u8; 4]>) + 'static,
    ) -> Self {
        let supported = system_backdrop_ok(os);
        let surface = DwmSurface::new(hwnd);
        Self::from_parts(
            surface,
            supported && hwnd != 0,
            hwnd,
            !e2e,
            Box::new(on_edge),
        )
    }
}

impl<S: FsSurface> WindowMaterial<S> {
    /// The assembled controller; `edge_enabled` false (under `--e2e`) means the border is never
    /// applied and no deadline is ever set.
    pub fn from_parts(
        surface: S,
        supported: bool,
        hwnd: Hwnd,
        edge_enabled: bool,
        on_edge: Box<dyn Fn(Option<[u8; 4]>)>,
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
            material: Material::new(surface, supported),
            edge,
            supported,
            bg,
            edges,
            flat,
        }
    }

    /// The build has the backdrop call (`system_backdrop_ok`) and the window has a handle.
    pub fn supported(&self) -> bool {
        self.supported
    }

    /// The solid ground the frost and the border follow.
    pub fn set_solid_ground(&mut self, hex: &str, edges: WindowEdges, now: Instant) {
        self.material.set_bg(hex);
        if flare_win::material::valid_bg(hex) {
            *self.bg.borrow_mut() = hex.to_string();
        }
        self.edges.set(edges);
        self.edge.apply(now);
    }

    /// Glass on or off. The OS rewrites window attributes on a backdrop change, so the border is
    /// applied again.
    pub fn set_glass(&mut self, on: bool, now: Instant) {
        self.material.set_acrylic(on);
        self.edge.apply(now);
    }

    pub fn glass_on(&self) -> bool {
        self.material.glass_on()
    }

    /// Before the window goes full screen: the backdrop comes off first.
    pub fn before_fullscreen(&mut self) {
        self.material.before_fullscreen();
    }

    /// The window state settled (maximised / full screen / restored).
    pub fn on_window_state(&mut self, maximised: bool, fullscreen: bool, now: Instant) {
        self.flat.set(maximised || fullscreen);
        self.material.surface().set_fullscreen(fullscreen);
        self.material.settled();
        self.edge.apply(now);
    }

    pub fn tick(&mut self, now: Instant) {
        self.edge.tick(now);
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.edge.next_deadline()
    }
}

fn hwnd_of(app: &crate::AppWindow) -> Option<Hwnd> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use slint::winit_030::WinitWindowAccessor;
    slint::ComponentHandle::window(app)
        .with_winit_window(|w| match w.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
            _ => None,
        })
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Call {
        Dark(bool),
        Backdrop(bool),
        Fullscreen(bool),
        Ui(&'static str),
    }

    type Log = Rc<RefCell<Vec<Call>>>;

    struct Rec {
        log: Log,
        full: bool,
    }

    impl Surface for Rec {
        fn set_backdrop(&mut self, on: bool) -> bool {
            self.log.borrow_mut().push(Call::Backdrop(on));
            true
        }
        fn set_dark(&mut self, dark: bool) {
            self.log.borrow_mut().push(Call::Dark(dark));
        }
        fn is_fullscreen(&self) -> bool {
            self.full
        }
    }

    impl FsSurface for Rec {
        fn set_fullscreen(&mut self, on: bool) {
            self.full = on;
            self.log.borrow_mut().push(Call::Fullscreen(on));
        }
    }

    fn wm(supported: bool, edge_enabled: bool) -> (WindowMaterial<Rec>, Log) {
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        let m = WindowMaterial::from_parts(
            Rec {
                log: log.clone(),
                full: false,
            },
            supported,
            0,
            edge_enabled,
            Box::new(|_| {}),
        );
        (m, log)
    }

    #[test]
    fn order_only_material_first_when_glass_turns_on() {
        assert_eq!(order_for(false, true), Order::MaterialThenUi);
        assert_eq!(order_for(true, false), Order::UiThenMaterial);
        assert_eq!(order_for(true, true), Order::UiThenMaterial);
        assert_eq!(order_for(false, false), Order::UiThenMaterial);
    }

    #[test]
    fn an_unsupported_build_makes_no_dwm_call() {
        let (mut m, log) = wm(false, true);
        let t0 = Instant::now();
        m.set_solid_ground("#0b0b0f", WindowEdges::Hairline, t0);
        m.set_glass(true, t0);
        m.before_fullscreen();
        m.on_window_state(false, true, t0);
        // Only the surface's own fullscreen flag moves; no dark, no backdrop call.
        assert!(
            log.borrow()
                .iter()
                .all(|c| matches!(c, Call::Fullscreen(_))),
            "{:?}",
            log.borrow()
        );
        assert!(!m.glass_on());
        assert!(!m.supported());
    }

    #[test]
    fn going_glass_calls_the_frost_then_the_backdrop_and_the_ui_waits() {
        let (mut m, log) = wm(true, true);
        let t0 = Instant::now();
        m.set_solid_ground("#0b0b0f", WindowEdges::Hairline, t0);
        assert_eq!(*log.borrow(), vec![Call::Dark(true), Call::Backdrop(false)]);
        let was = m.glass_on();
        // The change goes through the cell so the closures can both borrow it.
        let m = RefCell::new(m);
        let l2 = log.clone();
        let order = order_for(was, true);
        apply_in_order(
            order,
            || l2.borrow_mut().push(Call::Ui("translucent")),
            || m.borrow_mut().set_glass(true, t0),
        );
        let calls = log.borrow().clone();
        assert_eq!(
            calls,
            vec![
                Call::Dark(true),
                Call::Backdrop(false),
                Call::Backdrop(true),
                Call::Ui("translucent"),
            ]
        );
        assert!(m.borrow().glass_on());
        // Leaving glass: the UI is raised first, then the backdrop comes off.
        log.borrow_mut().clear();
        let l3 = log.clone();
        apply_in_order(
            order_for(true, false),
            || l3.borrow_mut().push(Call::Ui("opaque")),
            || m.borrow_mut().set_glass(false, t0),
        );
        assert_eq!(
            *log.borrow(),
            vec![Call::Ui("opaque"), Call::Backdrop(false)]
        );
    }

    #[test]
    fn fullscreen_holds_the_material_off_until_settled() {
        let (mut m, log) = wm(true, true);
        let t0 = Instant::now();
        m.set_solid_ground("#0b0b0f", WindowEdges::Hairline, t0);
        m.set_glass(true, t0);
        log.borrow_mut().clear();
        m.before_fullscreen();
        assert_eq!(*log.borrow(), vec![Call::Backdrop(false)]);
        assert!(!m.glass_on());
        m.on_window_state(false, true, t0);
        // Still off while full screen.
        assert!(!m.glass_on());
        assert_eq!(
            *log.borrow(),
            vec![Call::Backdrop(false), Call::Fullscreen(true)]
        );
        // Leaving full screen restores the backdrop.
        m.before_fullscreen();
        m.on_window_state(false, false, t0);
        assert!(m.glass_on());
        assert_eq!(log.borrow().last(), Some(&Call::Backdrop(true)));
    }

    #[test]
    fn e2e_disables_the_border() {
        let (mut m, _) = wm(true, false);
        let t0 = Instant::now();
        m.set_solid_ground("#0b0b0f", WindowEdges::Hairline, t0);
        m.on_window_state(false, false, t0);
        assert_eq!(m.next_deadline(), None);
        m.tick(t0 + Duration::from_secs(1));
        let (mut m, _) = wm(true, true);
        m.set_solid_ground("#0b0b0f", WindowEdges::Hairline, t0);
        assert_eq!(m.next_deadline(), Some(t0 + Duration::from_millis(250)));
    }

    #[test]
    fn the_border_reaches_the_slint_frame_after_the_wait() {
        let seen: Rc<RefCell<Vec<Option<[u8; 4]>>>> = Rc::new(RefCell::new(Vec::new()));
        let s2 = seen.clone();
        let mut m = WindowMaterial::from_parts(
            Rec {
                log: Rc::new(RefCell::new(Vec::new())),
                full: false,
            },
            true,
            0,
            true,
            Box::new(move |c| s2.borrow_mut().push(c)),
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

    #[test]
    fn the_slint_window_clear_follows_glass() {
        use crate::ui_harness::Harness;
        use slint::Global;
        let h = Harness::new(1.0, 1000, 640, || crate::AppWindow::new().unwrap());
        let theme = crate::Theme::get(&h.comp);
        let tabs = slint::Color::from_argb_u8(128, 31, 31, 30);
        theme.set_tabs(tabs);
        theme.set_window_clear(slint::Color::from_argb_u8(0, 0, 0, 0));
        let f = h.render();
        let p = f.px(600, 5);
        // One coat: the translucent tabs colour itself, over a clear window.
        // The harness un-premultiplies a 128-alpha pixel, which loses up to 2 per channel.
        for (i, (got, want)) in p.iter().zip([31u8, 31, 30, 128]).enumerate() {
            let tol = if i == 3 { 1 } else { 3 };
            assert!(got.abs_diff(want) <= tol, "{p:?}");
        }
        // With the window clear opaque the corner is the clear colour.
        theme.set_window_clear(slint::Color::from_argb_u8(255, 10, 20, 30));
        theme.set_edge(slint::Color::from_argb_u8(0, 0, 0, 0));
        let f = h.render();
        assert_eq!(f.px(600, 5)[3], 255);
    }
}
