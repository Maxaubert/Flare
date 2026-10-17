//! REVEAL, KEEP, side_wants and Reveal: the sidebar hover reveal (Task 11).
//!
//! Source: Prism Terminal's `hoverReveal.ts` and `revealZones.ts`. Time is an argument (`now`, in
//! milliseconds on any monotonic clock); the glue arms a timer for `next_due`.

use flare_win::edge_watch::REVEAL_ZONE;

/// How far past the sidebar's far edge the pointer may go and still keep it shown.
pub const KEEP: f64 = 4.0;

/// The reveal timings: the zone the pointer comes in by, and the waits either way.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RevealDelays {
    pub left: f64,
    pub top: f64,
    pub open_delay: u64,
    pub close_delay: u64,
}

pub const REVEAL: RevealDelays = RevealDelays {
    left: REVEAL_ZONE.left,
    top: REVEAL_ZONE.top,
    open_delay: 60,
    close_delay: 100,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// The collapsed sidebar, from `top` down, `width` wide: false without a point, above `top`, or
/// left of -outside; true within the left zone; else, when shown, while x <= width + KEEP.
pub fn side_wants(p: Option<Point>, shown: bool, top: f64, width: f64) -> bool {
    let Some(p) = p else {
        return false;
    };
    if p.y < top || p.x < -REVEAL_ZONE.outside {
        return false;
    }
    if p.x <= REVEAL.left {
        return true;
    }
    shown && p.x <= width + KEEP
}

/// The show-or-hide timing of one panel, without a clock of its own.
#[derive(Debug, Default)]
pub struct Reveal {
    shown: bool,
    held: bool,
    wants: bool,
    pending: Option<(bool, u64)>,
}

impl Reveal {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn shown(&self) -> bool {
        self.shown
    }

    /// The pointer moved: does it want the panel shown?
    pub fn feel(&mut self, wants: bool, now: u64) {
        self.wants = wants;
        self.aim(now);
    }

    /// Keep it shown whatever the pointer does (a menu of its own is open), or let go.
    pub fn hold(&mut self, on: bool, now: u64) {
        self.held = on;
        self.aim(now);
    }

    /// Put it away now, unless held. Returns the change.
    pub fn hide(&mut self) -> Option<bool> {
        self.wants = false;
        if self.held {
            return None;
        }
        self.set(false)
    }

    /// Fires the pending change when it is due; returns the new shown value when it changed.
    pub fn poll(&mut self, now: u64) -> Option<bool> {
        match self.pending {
            Some((target, due)) if due <= now => self.set(target),
            _ => None,
        }
    }

    /// Set the shown value now, dropping any pending change. Returns the change.
    fn set(&mut self, v: bool) -> Option<bool> {
        self.pending = None;
        if self.shown == v {
            return None;
        }
        self.shown = v;
        Some(v)
    }

    /// Work out the target (held or wanted) and the wait for it. A pending change for the same
    /// target keeps its due time, so a pointer moving about inside does not restart the wait.
    fn aim(&mut self, now: u64) {
        let target = self.held || self.wants;
        if target == self.shown {
            self.pending = None;
            return;
        }
        if matches!(self.pending, Some((t, _)) if t == target) {
            return;
        }
        let delay = if target {
            REVEAL.open_delay
        } else {
            REVEAL.close_delay
        };
        self.pending = Some((target, now + delay));
    }

    pub fn next_due(&self) -> Option<u64> {
        self.pending.map(|(_, due)| due)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_win::edge_watch::Zone;

    /// A reveal driven by a clock the test moves; `seen` records every change, in order.
    struct Clock {
        now: u64,
        seen: Vec<bool>,
        r: Reveal,
    }

    impl Clock {
        fn new() -> Self {
            Clock {
                now: 0,
                seen: Vec::new(),
                r: Reveal::new(),
            }
        }
        fn advance(&mut self, ms: u64) {
            self.now += ms;
            let now = self.now;
            let change = self.r.poll(now);
            self.note(change);
        }
        fn note(&mut self, change: Option<bool>) {
            if let Some(v) = change {
                self.seen.push(v);
            }
        }
        fn feel(&mut self, wants: bool) {
            let now = self.now;
            self.r.feel(wants, now);
        }
        fn hold(&mut self, on: bool) {
            let now = self.now;
            self.r.hold(on, now);
        }
    }

    #[test]
    fn opens_only_after_the_open_delay() {
        let mut c = Clock::new();
        c.feel(true);
        c.advance(59);
        assert!(c.seen.is_empty());
        c.advance(1);
        assert_eq!(c.seen, vec![true]);
    }

    #[test]
    fn a_pointer_that_only_passes_opens_nothing() {
        let mut c = Clock::new();
        c.feel(true);
        c.advance(30);
        c.feel(false);
        c.advance(1000);
        assert!(c.seen.is_empty());
    }

    #[test]
    fn stays_through_a_short_slip_and_goes_after_the_grace() {
        let mut c = Clock::new();
        c.feel(true);
        c.advance(60);
        assert_eq!(c.seen, vec![true]);
        c.feel(false);
        c.advance(90);
        c.feel(true);
        c.advance(1000);
        assert_eq!(c.seen, vec![true]);
        c.feel(false);
        c.advance(100);
        assert_eq!(c.seen, vec![true, false]);
    }

    #[test]
    fn a_pointer_moving_inside_does_not_restart_the_wait() {
        let mut c = Clock::new();
        c.feel(true);
        c.advance(30);
        c.feel(true);
        c.advance(30);
        assert_eq!(c.seen, vec![true]);
    }

    #[test]
    fn a_hold_keeps_it_shown_and_lets_go_after_the_grace() {
        let mut c = Clock::new();
        c.feel(true);
        c.advance(60);
        c.hold(true);
        c.feel(false);
        c.advance(5000);
        assert_eq!(c.seen, vec![true]);
        c.hold(false);
        c.advance(100);
        assert_eq!(c.seen, vec![true, false]);
    }

    #[test]
    fn hide_is_immediate_unless_held() {
        let mut c = Clock::new();
        c.feel(true);
        c.advance(60);
        assert_eq!(c.r.hide(), Some(false));
        c.feel(true);
        c.advance(60);
        assert!(c.r.shown());
        c.hold(true);
        assert_eq!(c.r.hide(), None);
        assert!(c.r.shown());
    }

    #[test]
    fn the_sidebar_comes_within_the_left_zone_below_its_top() {
        assert!(side_wants(
            Some(Point { x: 24.0, y: 300.0 }),
            false,
            0.0,
            220.0
        ));
        assert!(!side_wants(
            Some(Point { x: 25.0, y: 300.0 }),
            false,
            0.0,
            220.0
        ));
        assert!(!side_wants(
            Some(Point { x: 5.0, y: 10.0 }),
            false,
            36.0,
            220.0
        ));
    }

    #[test]
    fn the_sidebar_stays_on_itself_and_four_pixels_past() {
        assert!(side_wants(
            Some(Point { x: 100.0, y: 10.0 }),
            true,
            0.0,
            220.0
        ));
        assert!(side_wants(
            Some(Point { x: 224.0, y: 400.0 }),
            true,
            0.0,
            220.0
        ));
        assert!(!side_wants(
            Some(Point { x: 225.0, y: 400.0 }),
            true,
            0.0,
            220.0
        ));
    }

    #[test]
    fn nothing_without_a_point() {
        assert!(!side_wants(None, true, 0.0, 220.0));
    }

    #[test]
    fn reveal_constants_are_the_shared_zone() {
        assert_eq!(REVEAL.left, REVEAL_ZONE.left);
        assert_eq!(REVEAL.top, REVEAL_ZONE.top);
        assert_eq!(
            REVEAL_ZONE,
            Zone {
                top: 16.0,
                left: 24.0,
                outside: 8.0
            }
        );
        assert_eq!(REVEAL.open_delay, 60);
        assert_eq!(REVEAL.close_delay, 100);
    }
}
