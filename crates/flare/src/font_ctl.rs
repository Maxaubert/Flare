//! Font size and family apply, per-tab zoom (Task 27).
//!
//! The size and family live in the look store; a tab may zoom itself with Ctrl+wheel. The zoom is
//! held per tab in memory and never saved. A change of pixel size or face reaches the grid, and the
//! new `cols x rows` goes to every open tab through the refit hook (the same resize path as a window
//! resize).

#![allow(dead_code)] // wired by the window and the tab switch (the orchestrator)

use crate::host::GridHost;
use flare_settings::look::LookStore;
use flare_theme::fonts::{base_font_px, font_families, zoom_step};
use std::cell::RefCell;
use std::rc::Rc;

/// The pixel size a tab draws at: its own zoom, else the base size of the chosen percent.
pub fn effective_px(override_px: Option<i32>, base_pct: u32) -> i32 {
    override_px.unwrap_or_else(|| base_font_px(base_pct))
}

/// What a wheel notch did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WheelAction {
    /// The tab zoomed to this many pixels.
    Zoomed(i32),
    /// Not ours: the program or the scrollback keeps it.
    PassThrough,
}

pub struct FontCtl {
    store: LookStore,
    host: Rc<RefCell<GridHost>>,
    refit: Option<Box<dyn FnMut(usize, usize)>>,
}

impl FontCtl {
    pub fn new(store: LookStore, host: Rc<RefCell<GridHost>>) -> Self {
        Self {
            store,
            host,
            refit: None,
        }
    }

    /// Where a changed grid size goes: `TabHost::refit_all` for every open tab.
    pub fn with_refit(mut self, refit: impl FnMut(usize, usize) + 'static) -> Self {
        self.refit = Some(Box::new(refit));
        self
    }

    /// Gives the grid the face and size in force for the active tab. A no-op when neither changed.
    /// Returns the new `(cols, rows)` when the grid changed (every tab has been refitted then).
    pub fn apply(&mut self, active_override: Option<i32>) -> Option<(usize, usize)> {
        let px = effective_px(active_override, self.store.font_pct());
        let families = font_families(self.store.font_id());
        let refs: Vec<&str> = families.iter().map(String::as_str).collect();
        let dims = self.host.borrow_mut().set_font(&refs, px as f32)?;
        if let Some(refit) = self.refit.as_mut() {
            refit(dims.0, dims.1);
        }
        Some(dims)
    }

    /// Ctrl+wheel up zooms in one pixel, down out one; the result is the tab's override (never
    /// stored). Without Ctrl the notch is not ours.
    pub fn wheel(
        &mut self,
        ctrl: bool,
        delta_y: f64,
        active_override: &mut Option<i32>,
    ) -> WheelAction {
        if !ctrl || delta_y == 0.0 || !delta_y.is_finite() {
            return WheelAction::PassThrough;
        }
        let current = effective_px(*active_override, self.store.font_pct());
        let next = zoom_step(current, if delta_y > 0.0 { 1 } else { -1 });
        *active_override = Some(next);
        self.apply(*active_override);
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
        let mut ov = None;
        assert_eq!(r.ctl.wheel(false, -1.0, &mut ov), WheelAction::PassThrough);
        assert_eq!(ov, None);
        assert_eq!(r.ctl.wheel(false, 3.0, &mut ov), WheelAction::PassThrough);
        assert_eq!(ov, None);
    }

    #[test]
    fn ctrl_wheel_up_zooms_in_one_notch_down_zooms_out() {
        let mut r = rig();
        let mut ov = None;
        assert_eq!(r.ctl.wheel(true, 1.0, &mut ov), WheelAction::Zoomed(14));
        assert_eq!(ov, Some(14));
        assert_eq!(r.ctl.wheel(true, -1.0, &mut ov), WheelAction::Zoomed(13));
        assert_eq!(r.ctl.wheel(true, -1.0, &mut ov), WheelAction::Zoomed(12));
        let mut fresh = None;
        assert_eq!(r.ctl.wheel(true, -1.0, &mut fresh), WheelAction::Zoomed(12));
    }

    #[test]
    fn zoom_stops_at_7_and_64() {
        let mut r = rig();
        let mut ov = None;
        for _ in 0..100 {
            r.ctl.wheel(true, -1.0, &mut ov);
        }
        assert_eq!(ov, Some(7));
        for _ in 0..100 {
            r.ctl.wheel(true, 1.0, &mut ov);
        }
        assert_eq!(ov, Some(64));
    }

    #[test]
    fn zoom_is_never_saved() {
        let mut r = rig();
        r.store.set_font_pct(100);
        let before = (
            r.store.font_pct(),
            r.store.font_id(),
            r.prefs.get("flare.term.font-pct"),
            r.prefs.get("flare.term.zoom"),
        );
        let mut ov = None;
        r.ctl.wheel(true, 1.0, &mut ov);
        r.ctl.wheel(true, 1.0, &mut ov);
        assert_eq!(
            (
                r.store.font_pct(),
                r.store.font_id(),
                r.prefs.get("flare.term.font-pct"),
                r.prefs.get("flare.term.zoom")
            ),
            before
        );
        assert_eq!(r.store.font_pct(), 100);
    }

    #[test]
    fn the_override_is_per_tab() {
        let mut r = rig();
        r.ctl.apply(None);
        let at13 = r.refits.borrow().len();
        let (a, b) = (Some(20), None);
        let grown = r.ctl.apply(a);
        assert!(grown.is_some(), "tab A at 20 changes the grid");
        let back = r.ctl.apply(b);
        assert!(back.is_some(), "tab B at 13 changes it back");
        assert_eq!(r.refits.borrow().len(), at13 + 2);
        assert!(r.ctl.apply(b).is_none(), "an unchanged tab is a no-op");
    }

    #[test]
    fn a_size_change_refits_every_open_tab() {
        let mut r = rig();
        r.ctl.apply(None);
        r.refits.borrow_mut().clear();
        r.store.set_font_pct(200);
        let dims = r.ctl.apply(None).expect("the grid changes");
        // One call to the hook, which fans out to every tab (TabHost::refit_all).
        assert_eq!(*r.refits.borrow(), vec![dims]);
    }

    #[test]
    fn changing_the_family_changes_the_grid_face_and_refits() {
        let mut r = rig();
        r.ctl.apply(None);
        r.refits.borrow_mut().clear();
        r.store.set_font_id("consolas");
        r.ctl.apply(None);
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
        r.ctl.apply(None);
        let f = r.families.borrow();
        assert!(!f.is_empty(), "a stack is still given");
        assert!(f.len() > 1, "a tail of fallbacks follows the missing face");
        assert_eq!(f.last().map(String::as_str), Some("monospace"));
    }
}
