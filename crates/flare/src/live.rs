//! The live tab: a shell served by flare-term, drawn through the GridHost's `GridSource` seam.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use flare_render::decor::Decor;
use flare_render::grid::Grid;
use flare_render::metrics::CellMetrics;
use flare_term::handle::TermHandle;
use flare_term::session::SessionManager;

use crate::host::GridSource;
use crate::term_ui::DecorInputs;

/// The terminal as the grid host's source.
pub struct LiveSource {
    handle: Arc<TermHandle>,
    inputs: Option<Rc<RefCell<DecorInputs>>>,
}

impl LiveSource {
    pub fn new(handle: Arc<TermHandle>) -> Self {
        Self {
            handle,
            inputs: None,
        }
    }

    /// A source that also paints the controller's decoration (selection, find, links).
    pub fn with_inputs(handle: Arc<TermHandle>, inputs: Rc<RefCell<DecorInputs>>) -> Self {
        Self {
            handle,
            inputs: Some(inputs),
        }
    }
}

impl GridSource for LiveSource {
    fn resize(&mut self, cols: usize, rows: usize) {
        self.handle.resize(cols, rows);
    }

    fn grid(&mut self) -> Grid {
        self.handle.snapshot()
    }

    fn grid_into(&mut self, out: &mut Grid) {
        self.handle.snapshot_into(out);
    }

    fn dirty(&self) -> bool {
        self.handle.is_dirty()
    }

    fn decor(&mut self) -> Decor {
        match &self.inputs {
            Some(i) => i.borrow_mut().decor(&self.handle),
            None => Decor::default(),
        }
    }
}

/// The running session manager and the one tab.
pub struct Live {
    pub mgr: Arc<SessionManager>,
    pub handle: Arc<TermHandle>,
}

/// Kills every shell and waits (3 s cap) until each is gone. Safe to call twice.
pub fn shutdown(live: &Live) {
    live.handle.kill();
    live.mgr.kill_all();
    if !live.mgr.shells_gone(Duration::from_secs(3)) {
        log::warn!("shutdown: a shell was still running after 3 s");
    }
}

/// Pixel position inside the window to a cell, clamped to the grid. `origin` is the grid area's
/// top-left in window pixels (the window exports it as `grid-left` / `grid-top`), so a sidebar or a
/// taller strip moves it without this function knowing the layout.
pub fn cell_at(
    x_phys: f64,
    y_phys: f64,
    origin: (f64, f64),
    m: &CellMetrics,
    cols: usize,
    rows: usize,
) -> (usize, usize) {
    let pad = f64::from(m.padding);
    let col = ((x_phys - origin.0 - pad) / f64::from(m.cell_w))
        .floor()
        .max(0.0) as usize;
    let row = ((y_phys - origin.1 - pad) / f64::from(m.cell_h))
        .floor()
        .max(0.0) as usize;
    (
        col.min(cols.saturating_sub(1)),
        row.min(rows.saturating_sub(1)),
    )
}

/// Whether the caret is drawn as an outline: a popup, a dialog or a field holds the keyboard, so the
/// terminal has lost it (PT's inactive xterm cursor; audit V11).
pub fn caret_is_hollow(chrome_focus: bool, find_focus: bool, menu_open: bool) -> bool {
    chrome_focus || find_focus || menu_open
}

/// What decides whether a wheel turn belongs to the terminal in front.
#[derive(Debug, Clone, Copy)]
pub struct WheelGate {
    /// The front tab is a terminal (not Settings, the start screen or a placeholder).
    pub front_is_grid: bool,
    /// A field, dialog or popup holds the keyboard.
    pub chrome_focus: bool,
    /// A context menu is open.
    pub menu_open: bool,
    /// The grid area (left, top, width, height) in css px.
    pub grid: (f32, f32, f32, f32),
    /// Where the open command-help sheet starts (css px from the left), if it is open.
    pub help_from: Option<f32>,
}

/// Whether a wheel at `p` (css px) scrolls the terminal: only over the grid, never over the strip,
/// the sidebar, the help sheet or a menu, which take it themselves (audit 2026-10-20: M5).
pub fn wheel_is_terminals(p: (f32, f32), g: &WheelGate) -> bool {
    if !g.front_is_grid || g.chrome_focus || g.menu_open {
        return false;
    }
    if g.help_from.is_some_and(|from| p.0 >= from) {
        return false;
    }
    let (l, t, w, h) = g.grid;
    p.0 >= l && p.0 <= l + w && p.1 >= t && p.1 <= t + h
}

/// Wheel turns into whole notches: touchpad pixels and fractional lines both keep their remainder.
#[derive(Debug, Default)]
pub struct WheelAccum {
    acc: f64,
    lines: f64,
}

impl WheelAccum {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds pixels and returns the whole notches reached (toward zero), keeping the remainder.
    /// One notch is 3 cell heights of pixels.
    pub fn add(&mut self, px: f64, cell_h: u32) -> i32 {
        let notch = f64::from(3 * cell_h.max(1));
        self.acc += px;
        let n = (self.acc / notch).trunc() as i32;
        self.acc -= f64::from(n) * notch;
        n
    }

    /// Adds wheel lines (a mouse notch is 1.0, a smooth touchpad sends fractions) and returns the
    /// whole notches reached, keeping the remainder (audit 2026-10-20: M6).
    pub fn add_lines(&mut self, lines: f32) -> i32 {
        self.lines += f64::from(lines);
        let n = self.lines.trunc() as i32;
        self.lines -= f64::from(n);
        n
    }

    /// One macOS scroll event, filtered and turned into a step the way Terminal.app scrolls
    /// (rule 1129): momentum (AppKit's synthetic events after the fingers lift) moves nothing; a
    /// finger gesture's precise pixels build up into whole lines, one per cell height, and a new
    /// gesture (`Began`) starts from zero, so neither a leftover nor a reversed drag carries over; a
    /// plain mouse wheel (no phase) keeps its notches, `WHEEL_LINES_PER_NOTCH` lines each.
    /// `notches` asks for the pixels in notches instead (Cmd+wheel zoom, one pixel a notch).
    pub fn add_mac(
        &mut self,
        delta: ScrollDelta,
        phase: ScrollPhase,
        cell_h: u32,
        notches: bool,
    ) -> WheelStep {
        if phase == ScrollPhase::Momentum {
            return WheelStep::Lines(0);
        }
        if phase == ScrollPhase::Began {
            self.acc = 0.0;
            self.lines = 0.0;
        }
        match delta {
            ScrollDelta::Lines(l) => WheelStep::Notches(self.add_lines(l)),
            ScrollDelta::Pixels(px) if notches => WheelStep::Notches(self.add(px, cell_h)),
            ScrollDelta::Pixels(px) => {
                let line = f64::from(cell_h.max(1));
                self.acc += px;
                let n = (self.acc / line).trunc() as i32;
                self.acc -= f64::from(n) * line;
                WheelStep::Lines(n)
            }
        }
    }
}

/// One scroll event's amount, as winit gives it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScrollDelta {
    /// Wheel lines (a mouse notch is 1.0).
    Lines(f32),
    /// Precise touchpad pixels, physical.
    Pixels(f64),
}

/// A scroll event's place in its trackpad gesture (macOS); every event off macOS is `Wheel`.
pub use flare_win::scroll_phase::ScrollPhase;

/// What one scroll event moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelStep {
    /// Whole wheel notches (`WHEEL_LINES_PER_NOTCH` lines of view each).
    Notches(i32),
    /// Whole lines (a trackpad drag on macOS).
    Lines(i32),
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_render::glyph::{FakeGlyphs, GlyphSource};
    use flare_render::metrics::cell_metrics;
    use flare_term::fake::{FakeShells, FakeSpawner};
    use flare_term::handle::StartOptions;
    use flare_term::session::SessionOptions;

    fn fake_handle() -> (Arc<SessionManager>, Arc<TermHandle>, Arc<FakeSpawner>) {
        let sp = FakeSpawner::new();
        let mgr = SessionManager::new(sp.clone(), FakeShells::new(), SessionOptions::default());
        let handle = TermHandle::new("t1", Arc::clone(&mgr), 80, 24);
        (mgr, handle, sp)
    }

    #[test]
    fn cell_at_maps_pixels_to_cells() {
        let m = cell_metrics(&FakeGlyphs::new(8, 16).font_units(), 1.0);
        assert_eq!(
            cell_at(
                4.0 + 8.0 * 3.0 + 1.0,
                36.0 + 4.0 + 16.0 * 2.0 + 1.0,
                (0.0, 36.0),
                &m,
                100,
                40
            ),
            (3, 2)
        );
    }

    #[test]
    fn cell_at_clamps_into_the_grid() {
        let m = cell_metrics(&FakeGlyphs::new(8, 16).font_units(), 1.0);
        assert_eq!(cell_at(0.0, 0.0, (0.0, 36.0), &m, 100, 40), (0, 0));
        assert_eq!(cell_at(5000.0, 5000.0, (0.0, 36.0), &m, 100, 40), (99, 39));
    }

    #[test]
    fn cell_at_scales() {
        let m2 = cell_metrics(&FakeGlyphs::new(8, 16).font_units(), 2.0);
        assert!(m2.cell_w == 15 && m2.cell_h == 31 && m2.padding == 8);
        assert_eq!(
            cell_at(
                8.0 + 15.0 + 1.0,
                72.0 + 8.0 + 1.0,
                (0.0, 72.0),
                &m2,
                100,
                40
            ),
            (1, 0)
        );
    }

    #[test]
    fn wheel_accum_collects_pixels_into_notches() {
        let mut w = WheelAccum::new();
        assert_eq!(w.add(30.0, 16), 0);
        assert_eq!(w.add(30.0, 16), 1);
        assert_eq!(w.add(-100.0, 16), -1);
        assert_eq!(w.add(-8.0, 16), -1);
    }

    #[test]
    fn cell_at_follows_the_grid_origin_not_a_repeated_layout() {
        let m = cell_metrics(&FakeGlyphs::new(8, 16).font_units(), 1.0);
        // A 260 px sidebar and a 52 px strip: the origin says so, the cell maths does not know.
        assert_eq!(
            cell_at(
                260.0 + 4.0 + 8.0 * 5.0 + 1.0,
                52.0 + 4.0 + 16.0 * 3.0 + 1.0,
                (260.0, 52.0),
                &m,
                100,
                40
            ),
            (5, 3)
        );
    }

    // Rule 1129: Mac scrolling is Terminal.app's, line for line while the fingers move, and no
    // momentum after they lift; a mouse wheel keeps its notches.
    #[test]
    fn mac_scroll_moves_with_the_fingers_and_never_with_the_momentum() {
        use ScrollDelta::{Lines as L, Pixels as P};
        use ScrollPhase::*;
        use WheelStep::{Lines, Notches};
        // (case, events, the steps they give); cell height 16 px.
        type Ev = (ScrollDelta, ScrollPhase);
        let table: &[(&str, &[Ev], &[WheelStep])] = &[
            (
                "a finger gesture scrolls one line per cell height",
                &[
                    (P(0.0), Began),
                    (P(32.0), Changed),
                    (P(16.0), Changed),
                    (P(0.0), Ended),
                ],
                &[Lines(0), Lines(2), Lines(1), Lines(0)],
            ),
            (
                "momentum after the lift scrolls nothing",
                &[
                    (P(48.0), Momentum),
                    (P(160.0), Momentum),
                    (P(0.0), Momentum),
                ],
                &[Lines(0), Lines(0), Lines(0)],
            ),
            (
                "a gesture then its momentum: only the fingers count",
                &[
                    (P(20.0), Began),
                    (P(20.0), Changed),
                    (P(0.0), Ended),
                    (P(400.0), Momentum),
                ],
                &[Lines(1), Lines(1), Lines(0), Lines(0)],
            ),
            (
                "a mouse wheel notch is one notch, as on Windows",
                &[
                    (L(1.0), Wheel),
                    (L(-2.0), Wheel),
                    (L(0.5), Wheel),
                    (L(0.5), Wheel),
                ],
                &[Notches(1), Notches(-2), Notches(0), Notches(1)],
            ),
            (
                "small deltas add up until they cross one line",
                &[
                    (P(5.0), Began),
                    (P(5.0), Changed),
                    (P(5.0), Changed),
                    (P(5.0), Changed),
                ],
                &[Lines(0), Lines(0), Lines(0), Lines(1)],
            ),
            (
                "the remainder resets when a new gesture begins",
                &[
                    (P(15.0), Began),
                    (P(0.0), Ended),
                    (P(2.0), Began),
                    (P(2.0), Changed),
                    (P(-15.0), Began),
                    (P(-2.0), Changed),
                ],
                &[Lines(0), Lines(0), Lines(0), Lines(0), Lines(0), Lines(-1)],
            ),
        ];
        for (case, events, want) in table {
            let mut w = WheelAccum::new();
            let got: Vec<WheelStep> = events
                .iter()
                .map(|&(d, p)| w.add_mac(d, p, 16, false))
                .collect();
            assert_eq!(&got, want, "{case}");
        }
        // Cmd+wheel zoom on a trackpad keeps the notches (3 cell heights), momentum still dropped.
        let mut w = WheelAccum::new();
        assert_eq!(w.add_mac(P(40.0), Began, 16, true), Notches(0));
        assert_eq!(w.add_mac(P(10.0), Changed, 16, true), Notches(1));
        assert_eq!(w.add_mac(P(480.0), Momentum, 16, true), Lines(0));
    }

    #[test]
    fn wheel_accum_keeps_fractional_lines() {
        let mut w = WheelAccum::new();
        // A smooth touchpad: four 0.3 line steps make one notch, none is dropped.
        assert_eq!(w.add_lines(0.3), 0);
        assert_eq!(w.add_lines(0.3), 0);
        assert_eq!(w.add_lines(0.3), 0);
        assert_eq!(w.add_lines(0.3), 1);
        // 0.2 is left over; going back down reaches a whole notch only when it is really reached.
        assert_eq!(w.add_lines(-0.5), 0);
        assert_eq!(w.add_lines(-0.5), 0);
        assert_eq!(w.add_lines(-0.5), -1);
        // A mouse notch is one notch, two at once are two, and pixels do not mix into lines.
        let mut w = WheelAccum::new();
        assert_eq!(w.add_lines(1.0), 1);
        assert_eq!(w.add_lines(-2.0), -2);
        assert_eq!(w.add(30.0, 16), 0);
        assert_eq!(w.add_lines(0.0), 0);
    }

    #[test]
    fn the_caret_is_hollow_while_a_popup_or_a_field_holds_the_keys() {
        // Audit V11: PT shows the caret as an outline while the keyboard is elsewhere.
        assert!(!caret_is_hollow(false, false, false));
        assert!(caret_is_hollow(false, false, true), "a menu is open");
        assert!(
            caret_is_hollow(true, false, false),
            "a dialog or a chrome field"
        );
        assert!(caret_is_hollow(false, true, false), "the find field");
    }
    fn gate() -> WheelGate {
        WheelGate {
            front_is_grid: true,
            chrome_focus: false,
            menu_open: false,
            grid: (260.0, 36.0, 700.0, 500.0),
            help_from: None,
        }
    }

    #[test]
    fn a_wheel_is_the_terminals_only_over_the_grid() {
        let g = gate();
        assert!(wheel_is_terminals((500.0, 200.0), &g));
        // The strip, the sidebar's tab list and the margin below belong to something else.
        assert!(!wheel_is_terminals((500.0, 20.0), &g));
        assert!(!wheel_is_terminals((100.0, 200.0), &g));
        assert!(!wheel_is_terminals((500.0, 600.0), &g));
        assert!(!wheel_is_terminals((970.0, 200.0), &g));
    }

    #[test]
    fn a_wheel_over_an_overlay_or_a_held_keyboard_is_not_the_terminals() {
        let mut g = gate();
        g.help_from = Some(700.0);
        assert!(wheel_is_terminals((650.0, 200.0), &g), "left of the sheet");
        assert!(
            !wheel_is_terminals((750.0, 200.0), &g),
            "over the help sheet"
        );
        let mut g = gate();
        g.menu_open = true;
        assert!(!wheel_is_terminals((500.0, 200.0), &g), "a menu is open");
        let mut g = gate();
        g.chrome_focus = true;
        assert!(!wheel_is_terminals((500.0, 200.0), &g));
        let mut g = gate();
        g.front_is_grid = false;
        assert!(
            !wheel_is_terminals((500.0, 200.0), &g),
            "Settings is in front"
        );
    }

    #[test]
    fn live_source_resizes_the_handle_and_returns_its_grid() {
        let (_mgr, handle, _sp) = fake_handle();
        let mut src = LiveSource::new(Arc::clone(&handle));
        src.resize(30, 10);
        let g = src.grid();
        assert_eq!((g.cols(), g.rows()), (30, 10));
        assert!(!src.dirty());
        assert_eq!(src.dirty(), handle.is_dirty());
        handle.resize(31, 10);
        assert!(src.dirty());
        assert_eq!(src.dirty(), handle.is_dirty());
    }

    #[test]
    fn shutdown_kills_and_waits() {
        let (mgr, handle, sp) = fake_handle();
        handle.start(StartOptions {
            root: "C:\\x".to_string(),
            shell_id: Some("pwsh".to_string()),
            ..StartOptions::default()
        });
        let t = std::time::Instant::now();
        while sp.made().is_empty() && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let live = Live { mgr, handle };
        shutdown(&live);
        assert!(sp.made()[0].killed());
        shutdown(&live);
    }
}
