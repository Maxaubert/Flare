//! The Copied badge: its clock (pure; the time is passed in) and its Slint component.
//! Shown for 1200 ms, then a 200 ms fade, then gone (Prism Terminal `CopiedBadge.tsx`).
//! Every copy says "Copied"; a second copy restarts the clock rather than stacking a badge.
#![allow(dead_code)] // wired by TermUi and the window (Tasks 19 and 23)

/// How long the badge stays fully shown.
pub const COPIED_BADGE_MS: u64 = 1200;
/// The fade out after the shown time, after which the badge is gone.
pub const FADE_MS: u64 = 200;

/// Where the badge is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadgePhase {
    Gone,
    Shown,
    Leaving,
}

impl BadgePhase {
    /// The value the Slint `CopiedBadge.phase` property takes: 0 gone, 1 shown, 2 leaving.
    pub fn code(self) -> i32 {
        match self {
            BadgePhase::Gone => 0,
            BadgePhase::Shown => 1,
            BadgePhase::Leaving => 2,
        }
    }
}

/// The badge clock: the moment the last copy happened, if any.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BadgeClock {
    shown_at: Option<u64>,
}

impl BadgeClock {
    pub fn new() -> Self {
        Self::default()
    }

    /// A copy happened at `now_ms`: the badge shows, or its clock restarts.
    pub fn copied(&mut self, now_ms: u64) {
        self.shown_at = Some(now_ms);
    }

    /// The phase at `now_ms`.
    pub fn phase(&self, now_ms: u64) -> BadgePhase {
        let Some(t) = self.shown_at else {
            return BadgePhase::Gone;
        };
        let elapsed = now_ms.saturating_sub(t);
        if elapsed < COPIED_BADGE_MS {
            BadgePhase::Shown
        } else if elapsed < COPIED_BADGE_MS + FADE_MS {
            BadgePhase::Leaving
        } else {
            BadgePhase::Gone
        }
    }

    /// The next moment the phase changes after `now_ms`, or `None` when the badge is gone.
    pub fn next_due(&self, now_ms: u64) -> Option<u64> {
        let t = self.shown_at?;
        match self.phase(now_ms) {
            BadgePhase::Shown => Some(t + COPIED_BADGE_MS),
            BadgePhase::Leaving => Some(t + COPIED_BADGE_MS + FADE_MS),
            BadgePhase::Gone => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shown_for_1200_ms_then_leaves_for_200_ms_then_gone() {
        let mut c = BadgeClock::new();
        c.copied(0);
        assert_eq!(c.phase(0), BadgePhase::Shown);
        assert_eq!(c.phase(1199), BadgePhase::Shown);
        assert_eq!(c.phase(1200), BadgePhase::Leaving);
        assert_eq!(c.phase(1399), BadgePhase::Leaving);
        assert_eq!(c.phase(1400), BadgePhase::Gone);
    }

    #[test]
    fn a_second_copy_restarts_the_clock() {
        let mut c = BadgeClock::new();
        c.copied(0);
        c.copied(1000);
        assert_eq!(c.phase(2100), BadgePhase::Shown);
        assert_eq!(c.phase(2200), BadgePhase::Leaving);
    }

    #[test]
    fn next_due_names_the_next_change() {
        let mut c = BadgeClock::new();
        c.copied(0);
        assert_eq!(c.next_due(10), Some(1200));
        assert_eq!(c.next_due(1300), Some(1400));
        assert_eq!(c.next_due(1400), None);
    }

    #[test]
    fn a_new_clock_is_gone_with_nothing_due() {
        let c = BadgeClock::new();
        assert_eq!(c.phase(0), BadgePhase::Gone);
        assert_eq!(c.next_due(0), None);
        assert_eq!(BadgeClock::default(), c);
    }

    #[test]
    fn the_phase_codes_match_the_slint_component() {
        assert_eq!(BadgePhase::Gone.code(), 0);
        assert_eq!(BadgePhase::Shown.code(), 1);
        assert_eq!(BadgePhase::Leaving.code(), 2);
    }
}
