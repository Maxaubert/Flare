//! The live tab: a shell served by flare-term, drawn through the GridHost's `GridSource` seam.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use flare_render::grid::Grid;
use flare_render::metrics::CellMetrics;
use flare_term::handle::{StartOptions, TermHandle};
use flare_term::pty::PortableSpawner;
use flare_term::session::{RealShells, SessionManager, SessionOptions};
use slint::ComponentHandle;

use crate::AppWindow;
use crate::host::GridSource;

/// The title bar's height in logical pixels (theme.slint `bar-height`).
const BAR_LOGICAL: f64 = 36.0;

/// The terminal as the grid host's source.
pub struct LiveSource {
    handle: Arc<TermHandle>,
}

impl LiveSource {
    pub fn new(handle: Arc<TermHandle>) -> Self {
        Self { handle }
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
}

/// The running session manager and the one tab.
pub struct Live {
    pub mgr: Arc<SessionManager>,
    pub handle: Arc<TermHandle>,
}

/// Starts the default shell in the user's profile folder. The waker is coalesced: at most one
/// closure is queued on the event loop, however fast the output arrives.
pub fn start_live(weak: slint::Weak<AppWindow>) -> Live {
    let mgr = SessionManager::new(
        Arc::new(PortableSpawner),
        Arc::new(RealShells),
        SessionOptions::default(),
    );
    let handle = TermHandle::new("tab-1", Arc::clone(&mgr), 80, 24);
    let pending = Arc::new(AtomicBool::new(false));
    handle.set_waker(Arc::new(move || {
        if !pending.swap(true, Ordering::SeqCst) {
            let flag = Arc::clone(&pending);
            let weak = weak.clone();
            let queued = slint::invoke_from_event_loop(move || {
                flag.store(false, Ordering::SeqCst);
                if let Some(app) = weak.upgrade() {
                    app.window().request_redraw();
                }
            });
            if queued.is_err() {
                // The event loop is gone (quitting): nothing needs a redraw any more.
                log::debug!("waker: event loop closed");
            }
        }
    }));
    let root = std::env::var("USERPROFILE").unwrap_or_else(|_| ".".to_string());
    handle.start(StartOptions {
        root,
        shell_id: None,
        resume: None,
        plugin: None,
    });
    Live { mgr, handle }
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
