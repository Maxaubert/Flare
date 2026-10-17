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

/// The title bar's height in logical pixels (theme.slint `bar-height`).
const BAR_LOGICAL: f64 = 36.0;

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

/// Pixel position inside the window to a cell, clamped to the grid.
pub fn cell_at(
    x_phys: f64,
    y_phys: f64,
    scale: f32,
    m: &CellMetrics,
    cols: usize,
    rows: usize,
) -> (usize, usize) {
    let bar_phys = (BAR_LOGICAL * f64::from(scale)).round();
    let pad = f64::from(m.padding);
    let col = ((x_phys - pad) / f64::from(m.cell_w)).floor().max(0.0) as usize;
    let row = ((y_phys - bar_phys - pad) / f64::from(m.cell_h))
        .floor()
        .max(0.0) as usize;
    (
        col.min(cols.saturating_sub(1)),
        row.min(rows.saturating_sub(1)),
    )
}

/// Touchpad pixel scrolling: one notch is 3 cell heights of pixels.
#[derive(Debug, Default)]
pub struct WheelAccum {
    acc: f64,
}

impl WheelAccum {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds pixels and returns the whole notches reached (toward zero), keeping the remainder.
    pub fn add(&mut self, px: f64, cell_h: u32) -> i32 {
        let notch = f64::from(3 * cell_h.max(1));
        self.acc += px;
        let n = (self.acc / notch).trunc() as i32;
        self.acc -= f64::from(n) * notch;
        n
    }
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
                1.0,
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
        assert_eq!(cell_at(0.0, 0.0, 1.0, &m, 100, 40), (0, 0));
        assert_eq!(cell_at(5000.0, 5000.0, 1.0, &m, 100, 40), (99, 39));
    }

    #[test]
    fn cell_at_scales() {
        let m2 = cell_metrics(&FakeGlyphs::new(8, 16).font_units(), 2.0);
        assert!(m2.cell_w == 15 && m2.cell_h == 31 && m2.padding == 8);
        assert_eq!(
            cell_at(8.0 + 15.0 + 1.0, 72.0 + 8.0 + 1.0, 2.0, &m2, 100, 40),
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
