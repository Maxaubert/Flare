//! Font size and family apply, and the one zoom every tab shares (Task 27, rule 1154).
//!
//! The size and family live in the look store. Ctrl+wheel (Cmd+wheel on the Mac) and the Mac zoom
//! keys move one app-wide zoom, held in memory as a pixel offset on the settings' size and never
//! saved. Every tab draws at the same size, so a zoom in any tab reaches the grid, and the new
//! `cols x rows` goes to every open tab, hidden ones included, through the refit path (the same
//! resize path as a window resize). New tabs open at the shared size.

#![allow(dead_code)] // wired by the window and the tab switch (the orchestrator)

use crate::host::GridHost;
use flare_settings::look::LookStore;
use flare_theme::fonts::{base_font_px, font_families, zoom_step};
use std::cell::RefCell;
use std::rc::Rc;

/// The pixel size every tab draws at: the base size of the chosen percent moved by the shared zoom
/// (pixels, may be negative), kept within the zoom range (7 to 64). No zoom is the base size as is.
pub fn effective_px(zoom: i32, base_pct: u32) -> i32 {
    let base = base_font_px(base_pct);
    if zoom == 0 {
        base
    } else {
        zoom_step(base, zoom)
    }
}

/// What a wheel notch did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WheelAction {
    /// Every tab zoomed to this many pixels.
    Zoomed(i32),
    /// Not ours: the program or the scrollback keeps it.
    PassThrough,
}

pub struct FontCtl {
    store: LookStore,
    host: Rc<RefCell<GridHost>>,
    refit: Option<Box<dyn FnMut(usize, usize)>>,
    /// `--font` and `--font-pct` for this run (decision D13); they write nothing.
    font_id: Option<String>,
    font_pct: Option<u32>,
    /// The shared zoom, in pixels on top of the settings' size. In memory only, for this run.
    zoom: i32,
}

impl FontCtl {
    pub fn new(store: LookStore, host: Rc<RefCell<GridHost>>) -> Self {
        Self {
            store,
            host,
            refit: None,
            font_id: None,
            font_pct: None,
            zoom: 0,
        }
    }

    /// The one-run face and size flags, laid over the store.
    pub fn with_overrides(mut self, font_id: Option<String>, font_pct: Option<u32>) -> Self {
        self.font_id = font_id;
        self.font_pct = font_pct;
        self
    }

    /// The size percent in force: this run's flag, else the settings.
    pub fn pct(&self) -> u32 {
        self.font_pct.unwrap_or_else(|| self.store.font_pct())
    }

    /// Where a changed grid size goes: `TabHost::refit_all` for every open tab.
    pub fn with_refit(mut self, refit: impl FnMut(usize, usize) + 'static) -> Self {
        self.refit = Some(Box::new(refit));
        self
    }

    /// The shared zoom in pixels on top of the settings' size (0: none).
    pub fn zoom(&self) -> i32 {
        self.zoom
    }

    /// The pixel size every tab draws at now.
    pub fn px(&self) -> i32 {
        effective_px(self.zoom, self.pct())
    }

    /// Moves the shared zoom one pixel per step (up is positive), within 7 to 64 px. Returns the
    /// new size. Nothing is stored.
    pub fn zoom_by(&mut self, step: i32) -> i32 {
        let base = base_font_px(self.pct());
        let next = zoom_step(self.px(), step.signum());
        self.zoom = next - base;
        next
    }

    /// Back to the settings' size (Cmd+0).
    pub fn reset_zoom(&mut self) {
        self.zoom = 0;
    }

    /// Gives the grid the face and size in force (shared by every tab). A no-op when neither
    /// changed. Returns the new `(cols, rows)` when the grid changed (every tab has been refitted
    /// then).
    pub fn apply(&mut self) -> Option<(usize, usize)> {
        let px = self.px();
        let id = self
            .font_id
            .as_deref()
            .unwrap_or_else(|| self.store.font_id());
        let families = font_families(id);
        let refs: Vec<&str> = families.iter().map(String::as_str).collect();
        // A busy grid (a re-entrant moment) keeps its face; the next apply catches up.
        let dims = self
            .host
            .try_borrow_mut()
            .ok()?
            .set_font(&refs, px as f32)?;
        if let Some(refit) = self.refit.as_mut() {
            refit(dims.0, dims.1);
        }
        Some(dims)
    }

    /// Ctrl+wheel up zooms every tab in one pixel, down out one (never stored). Without Ctrl the
    /// notch is not ours.
    pub fn wheel(&mut self, ctrl: bool, delta_y: f64) -> WheelAction {
        if !ctrl || delta_y == 0.0 || !delta_y.is_finite() {
            return WheelAction::PassThrough;
        }
        let next = self.zoom_by(if delta_y > 0.0 { 1 } else { -1 });
        self.apply();
        WheelAction::Zoomed(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::GridPainter;
    use flare_render::glyph::{FakeGlyphs, GlyphBitmap, GlyphKey, GlyphSource};
    use flare_render::metrics::FontUnits;
    use flare_settings::kv::PrefStore;

    /// A fake face that remembers the family stack it was last given.
    struct Recording {
        inner: FakeGlyphs,
        families: Rc<RefCell<Vec<String>>>,
    }

    impl GlyphSource for Recording {
        fn glyph(&mut self, key: &GlyphKey) -> Option<&GlyphBitmap> {
            self.inner.glyph(key)
        }
        fn font_units(&self) -> FontUnits {
            self.inner.font_units()
        }
        fn set_scale(&mut self, scale: f32) {
            self.inner.set_scale(scale);
        }
        fn set_font(&mut self, families: &[&str], base_px: f32) {
            *self.families.borrow_mut() = families.iter().map(|f| f.to_string()).collect();
            self.inner.set_font(families, base_px);
        }
    }

    struct Rig {
        ctl: FontCtl,
        store: LookStore,
        prefs: PrefStore,
        families: Rc<RefCell<Vec<String>>>,
        refits: Rc<RefCell<Vec<(usize, usize)>>>,
    }

    fn rig() -> Rig {
        let prefs = PrefStore::memory();
        let store = LookStore::new(prefs.clone());
        let families = Rc::new(RefCell::new(Vec::new()));
        let host = Rc::new(RefCell::new(GridHost::new(
            Box::new(Recording {
                inner: FakeGlyphs::new(8, 16),
                families: families.clone(),
            }),
            GridPainter::Cpu,
        )));
        host.borrow_mut().resize(648, 264, 1.0);
        let refits = Rc::new(RefCell::new(Vec::new()));
        let ctl = FontCtl::new(store.clone(), host).with_refit({
            let refits = refits.clone();
            move |c, r| refits.borrow_mut().push((c, r))
        });
        Rig {
            ctl,
            store,
            prefs,
            families,
            refits,
        }
    }

    #[test]
    fn a_plain_wheel_stays_with_the_program_or_the_scrollback() {
        let mut r = rig();
        assert_eq!(r.ctl.wheel(false, -1.0), WheelAction::PassThrough);
        assert_eq!(r.ctl.wheel(false, 3.0), WheelAction::PassThrough);
        assert_eq!(r.ctl.zoom(), 0);
    }

    #[test]
    fn ctrl_wheel_up_zooms_in_one_notch_down_zooms_out() {
        let mut r = rig();
        assert_eq!(r.ctl.wheel(true, 1.0), WheelAction::Zoomed(14));
        assert_eq!(r.ctl.zoom(), 1);
        assert_eq!(r.ctl.wheel(true, -1.0), WheelAction::Zoomed(13));
        assert_eq!(r.ctl.wheel(true, -1.0), WheelAction::Zoomed(12));
        assert_eq!(r.ctl.zoom(), -1);
        r.ctl.reset_zoom();
        assert_eq!(r.ctl.px(), 13);
    }

    #[test]
    fn zoom_stops_at_7_and_64() {
        let mut r = rig();
        for _ in 0..100 {
            r.ctl.wheel(true, -1.0);
        }
        assert_eq!(r.ctl.px(), 7);
        // At the floor one notch up is one pixel up, not a climb out of a debt.
        assert_eq!(r.ctl.wheel(true, 1.0), WheelAction::Zoomed(8));
        for _ in 0..100 {
            r.ctl.wheel(true, 1.0);
        }
        assert_eq!(r.ctl.px(), 64);
        assert_eq!(r.ctl.wheel(true, -1.0), WheelAction::Zoomed(63));
    }

    #[test]
    fn zoom_is_never_saved() {
        let mut r = rig();
        r.store.set_font_pct(100);
        let before = (
            r.store.font_pct(),
            r.store.font_id(),
            r.prefs.get("flare.term.fontPct"),
            r.prefs.get("flare.term.zoom"),
        );
        r.ctl.wheel(true, 1.0);
        r.ctl.wheel(true, 1.0);
        assert_eq!(
            (
                r.store.font_pct(),
                r.store.font_id(),
                r.prefs.get("flare.term.fontPct"),
                r.prefs.get("flare.term.zoom")
            ),
            before
        );
        assert_eq!(r.store.font_pct(), 100);
    }

    #[test]
    fn the_zoom_is_one_offset_on_the_settings_size() {
        let mut r = rig();
        r.ctl.apply();
        r.ctl.zoom_by(1);
        r.ctl.zoom_by(1);
        assert_eq!(r.ctl.px(), 15);
        let grown = r.ctl.apply();
        assert!(grown.is_some(), "the zoom changes the grid");
        assert!(r.ctl.apply().is_none(), "an unchanged size is a no-op");
        // A settings size change keeps the zoom on top of the new base.
        r.store.set_font_pct(200);
        assert_eq!(r.ctl.px(), base_font_px(200) + 2);
        r.ctl.reset_zoom();
        assert_eq!(r.ctl.px(), base_font_px(200));
    }

    #[test]
    fn a_size_change_refits_every_open_tab() {
        let mut r = rig();
        r.ctl.apply();
        r.refits.borrow_mut().clear();
        r.store.set_font_pct(200);
        let dims = r.ctl.apply().expect("the grid changes");
        // One call to the hook, which fans out to every tab (TabHost::refit_all).
        assert_eq!(*r.refits.borrow(), vec![dims]);
    }

    #[test]
    fn changing_the_family_changes_the_grid_face_and_refits() {
        let mut r = rig();
        r.ctl.apply();
        r.refits.borrow_mut().clear();
        r.store.set_font_id("consolas");
        r.ctl.apply();
        assert_eq!(
            r.families.borrow().first().map(String::as_str),
            Some("Consolas")
        );
        // The face is new; the refit runs when the cell grid changes with it.
        assert!(r.refits.borrow().len() <= 1);
    }

    #[test]
    fn a_missing_face_falls_through_quietly() {
        let mut r = rig();
        r.store.set_font_id("iosevka");
        r.ctl.apply();
        let f = r.families.borrow();
        assert!(!f.is_empty(), "a stack is still given");
        assert!(f.len() > 1, "a tail of fallbacks follows the missing face");
        assert_eq!(f.last().map(String::as_str), Some("monospace"));
    }
}
