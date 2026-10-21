//! SidebarState: mode change, slide and hold (Task 11).
//!
//! Source: Prism Terminal's `sidebarPrefs.ts` (the slide and the fit hold). A change of mode starts a
//! slide, and holds the terminals' fit for the slide and one frame past it, so the terminals are
//! fitted once, at the end. Time is an argument, never read.

use crate::fit_hold::FitHold;
use flare_settings::sidebar::SidebarMode;

/// The sidebar's slide, in milliseconds (Prism Terminal's SIDEBAR_SLIDE_MS).
pub const SIDEBAR_SLIDE_MS: u64 = 240;
/// The slide plus a frame, so its last frame is laid out before the fit.
pub const SLIDE_SETTLE_MS: u64 = SIDEBAR_SLIDE_MS + 40;

#[derive(Debug)]
pub struct SidebarState {
    mode: SidebarMode,
    slide_until: Option<u64>,
}

impl SidebarState {
    pub fn new(mode: SidebarMode) -> Self {
        Self {
            mode,
            slide_until: None,
        }
    }

    pub fn mode(&self) -> SidebarMode {
        self.mode
    }

    /// A different mode starts a slide (a new change restarts it) and holds the terminals' fit for
    /// SLIDE_SETTLE_MS; the same mode is nothing. Returns whether the mode changed.
    pub fn set_mode(&mut self, mode: SidebarMode, now: u64, fit: &mut FitHold) -> bool {
        if self.mode == mode {
            return false;
        }
        self.mode = mode;
        self.slide_until = Some(now + SLIDE_SETTLE_MS);
        fit.hold(SLIDE_SETTLE_MS, now);
        true
    }

    pub fn toggle(&mut self, now: u64, fit: &mut FitHold) -> SidebarMode {
        let next = match self.mode {
            SidebarMode::Open => SidebarMode::Collapsed,
            SidebarMode::Collapsed => SidebarMode::Open,
        };
        self.set_mode(next, now, fit);
        self.mode
    }

    pub fn sliding(&self, now: u64) -> bool {
        self.slide_until.is_some_and(|end| end > now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinning_and_unpinning_slide_and_hold_the_fit() {
        let mut fit = FitHold::new();
        let mut s = SidebarState::new(SidebarMode::Open);
        assert!(s.set_mode(SidebarMode::Collapsed, 0, &mut fit));
        assert!(s.sliding(279));
        assert!(!s.sliding(280));
        assert!(fit.held(279));
        assert!(!fit.held(280));
        assert!(s.set_mode(SidebarMode::Open, 280, &mut fit));
        assert_eq!(s.mode(), SidebarMode::Open);
        assert!(s.sliding(280));
        assert!(fit.held(280));
        assert!(!s.sliding(560) && !fit.held(560));
        assert!(!s.set_mode(SidebarMode::Open, 560, &mut fit));
        assert!(!s.sliding(560));
        assert!(s.set_mode(SidebarMode::Collapsed, 560, &mut fit));
        assert!(s.set_mode(SidebarMode::Open, 660, &mut fit));
        assert!(s.sliding(900));
        assert!(!s.sliding(940));
    }

    #[test]
    fn toggle_flips_the_mode_and_slides() {
        let mut fit = FitHold::new();
        let mut s = SidebarState::new(SidebarMode::Open);
        assert_eq!(s.toggle(0, &mut fit), SidebarMode::Collapsed);
        assert!(s.sliding(0));
        assert_eq!(s.toggle(500, &mut fit), SidebarMode::Open);
    }
}
