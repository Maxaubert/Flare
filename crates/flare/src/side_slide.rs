//! The sidebar's slide, paced by the frames (audit V3, 2026-10-20). Both apps declare 240 ms of
//! `cubic-bezier(0.16, 1, 0.3, 1)`. A Slint `animate` takes its start when the property is set, in the
//! middle of the key's handler, and counts whole milliseconds, so Flare's first frame was already well
//! into the curve. Here the slide starts at the first frame that draws it, as a CSS transition does, and
//! every frame reads the curve at its own time (fractional ms).
//!
//! Two values move: the slot (the room the sidebar takes) and the panel's x. Each eases from what the
//! screen showed last to its new end. The glue calls `aim` when the sidebar's state changes and `frame`
//! as each frame begins; the values go to `sidebar-slot-now` and `sidebar-slide-now`.

/// The declared slide.
pub const SLIDE_MS: f64 = 240.0;
const EASE: (f64, f64, f64, f64) = (0.16, 1.0, 0.3, 1.0);
/// Hidden, the panel waits this far past its own width (the corner's radius), so its right edge and
/// the cut travel together.
pub const CORNER_R: f64 = 10.0;

/// CSS `cubic-bezier(x1, y1, x2, y2)`: the progress at time fraction `t`.
pub fn bezier(c: (f64, f64, f64, f64), t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    if t <= 0.0 || t >= 1.0 {
        return t;
    }
    let at = |a: f64, b: f64, s: f64| {
        3.0 * a * (1.0 - s).powi(2) * s + 3.0 * b * (1.0 - s) * s * s + s.powi(3)
    };
    // x(s) is monotonic for x1, x2 in 0..=1: bisection to well under a millionth.
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for _ in 0..48 {
        let mid = (lo + hi) / 2.0;
        if at(c.0, c.2, mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    at(c.1, c.3, (lo + hi) / 2.0)
}

/// The sidebar's curve at time fraction `t`.
pub fn ease(t: f64) -> f64 {
    bezier(EASE, t)
}

/// One eased value.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Track {
    from: f64,
    to: f64,
    /// Set by the first frame after `aim`; `None` while that frame has not come.
    start: Option<f64>,
    dur: f64,
    /// What the last frame showed.
    shown: f64,
}

impl Track {
    fn at(v: f64) -> Self {
        Self {
            from: v,
            to: v,
            start: None,
            dur: 0.0,
            shown: v,
        }
    }

    fn aim(&mut self, to: f64, dur: f64) {
        if (to - self.to).abs() < 1e-6 {
            return;
        }
        self.from = self.shown;
        self.to = to;
        self.start = None;
        self.dur = dur;
        if dur <= 0.0 {
            self.from = to;
            self.shown = to;
        }
    }

    fn frame(&mut self, now: f64) -> f64 {
        if self.moving() {
            let start = *self.start.get_or_insert(now);
            let k = ((now - start) / self.dur).clamp(0.0, 1.0);
            self.shown = self.from + (self.to - self.from) * ease(k);
            if k >= 1.0 {
                self.from = self.to;
                self.shown = self.to;
                self.start = None;
            }
        }
        self.shown
    }

    fn moving(&self) -> bool {
        (self.from - self.to).abs() > 1e-6
    }
}

/// What the sidebar is, as the slide needs it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SlideAim {
    pub collapsed: bool,
    pub revealed: bool,
    /// css px
    pub width: f64,
    /// A pin/unpin slide runs: the slot moves too.
    pub sliding: bool,
    /// The OS asks for no animation: everything jumps.
    pub reduced: bool,
}

impl SlideAim {
    /// The room it takes at rest.
    pub fn slot(&self) -> f64 {
        if self.collapsed { 0.0 } else { self.width }
    }

    /// The panel's x at rest: hidden, it waits past its own width and the corner.
    pub fn panel(&self) -> f64 {
        if self.collapsed && !self.revealed {
            -(self.width + CORNER_R)
        } else {
            0.0
        }
    }
}

/// The slot and the panel's x.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SideSlide {
    tracks: Option<(Track, Track)>,
}

impl SideSlide {
    /// The sidebar changed (or not): the values head for its new rest. The slot slides only in a
    /// pin/unpin slide; the panel slides while collapsed (the reveal) or in a pin/unpin slide; the
    /// first aim and reduced motion jump. True when a frame is now owed.
    pub fn aim(&mut self, a: &SlideAim) -> bool {
        let dur = |on: bool| if on && !a.reduced { SLIDE_MS } else { 0.0 };
        match &mut self.tracks {
            None => {
                self.tracks = Some((Track::at(a.slot()), Track::at(a.panel())));
            }
            Some((slot, panel)) => {
                slot.aim(a.slot(), dur(a.sliding));
                panel.aim(a.panel(), dur(a.collapsed || a.sliding));
            }
        }
        self.running()
    }

    /// A frame begins at `now` (ms): a slide that has not started starts here. The values it shows.
    pub fn frame(&mut self, now: f64) -> (f32, f32) {
        match &mut self.tracks {
            Some((slot, panel)) => (slot.frame(now) as f32, panel.frame(now) as f32),
            None => (0.0, 0.0),
        }
    }

    /// The e2e's picture at an offset: a slide that is moving is put `ms` into its time as of `now` (the
    /// frames that come next go on from there), and the values it shows are returned.
    pub fn place(&mut self, now: f64, ms: f64) -> (f32, f32) {
        if let Some((slot, panel)) = &mut self.tracks {
            for t in [slot, panel] {
                if t.moving() {
                    t.start = Some(now - ms);
                }
            }
        }
        self.frame(now)
    }

    /// What the last frame showed (or the rest a jump went to).
    pub fn shown(&self) -> (f32, f32) {
        self.tracks
            .as_ref()
            .map_or((0.0, 0.0), |(s, p)| (s.shown as f32, p.shown as f32))
    }

    /// A slide is waiting for its first frame or still moving.
    pub fn running(&self) -> bool {
        self.tracks
            .as_ref()
            .is_some_and(|(s, p)| s.moving() || p.moving())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(sliding: bool) -> SlideAim {
        SlideAim {
            collapsed: false,
            revealed: false,
            width: 220.0,
            sliding,
            reduced: false,
        }
    }

    fn shut(sliding: bool) -> SlideAim {
        SlideAim {
            collapsed: true,
            ..open(sliding)
        }
    }

    #[test]
    fn the_curve_is_the_css_one() {
        assert_eq!(ease(0.0), 0.0);
        assert_eq!(ease(1.0), 1.0);
        // Its ends: y'(0) / x'(0) = 1 / 0.16, flat at the end.
        assert!((ease(0.001) - 0.001 / 0.16).abs() < 0.001);
        assert!(ease(0.5) > 0.96 && ease(0.5) < 0.98, "{}", ease(0.5));
        let mut last = 0.0;
        for i in 0..=100 {
            let v = ease(f64::from(i) / 100.0);
            assert!(v >= last, "monotonic at {i}");
            last = v;
        }
    }

    #[test]
    fn the_first_aim_takes_the_rest_without_a_slide() {
        let mut s = SideSlide::default();
        assert!(!s.aim(&shut(true)));
        assert_eq!(s.shown(), (0.0, -230.0));
    }

    #[test]
    fn a_slide_starts_at_its_first_frame_however_late_it_comes() {
        let mut s = SideSlide::default();
        s.aim(&open(false));
        assert!(s.aim(&shut(true)), "a frame is owed");
        assert_eq!(s.shown(), (220.0, 0.0), "nothing moves before the frame");
        // The first frame comes 70 ms after the key: it still shows the start.
        assert_eq!(s.frame(1070.0), (220.0, 0.0));
        let (slot, panel) = s.frame(1070.0 + 120.0);
        let want = 220.0 * (1.0 - ease(0.5));
        assert!((f64::from(slot) - want).abs() < 1e-3, "{slot} vs {want}");
        assert!(
            (f64::from(panel) + 230.0 * ease(0.5)).abs() < 1e-3,
            "{panel}"
        );
        assert!(s.running());
        // A fraction of a millisecond counts.
        let a = s.frame(1070.0 + 7.25).0;
        let b = s.frame(1070.0 + 7.75).0;
        assert!(a > b, "{a} then {b}");
        assert_eq!(s.frame(1070.0 + 240.0), (0.0, -230.0));
        assert!(!s.running());
    }

    #[test]
    fn the_slot_holds_still_outside_a_pin_slide_and_the_reveal_slides_the_panel_only() {
        let mut s = SideSlide::default();
        s.aim(&shut(false));
        // A drag of the grip while pinned: no slide.
        s.aim(&open(false));
        assert_eq!(s.shown(), (220.0, 0.0));
        s.aim(&SlideAim {
            width: 260.0,
            ..open(false)
        });
        assert_eq!(s.shown(), (260.0, 0.0));
        assert!(!s.running());
        // Unpinned with no pin slide: the slot jumps, the panel slides away (it is collapsed).
        assert!(s.aim(&SlideAim {
            width: 260.0,
            ..shut(false)
        }));
        assert_eq!(s.shown(), (0.0, 0.0));
        s.frame(0.0);
        assert_eq!(s.frame(240.0), (0.0, -270.0));
        // Collapsed, the pointer reveals it: the panel slides, the slot stays 0.
        assert!(s.aim(&SlideAim {
            revealed: true,
            width: 260.0,
            ..shut(false)
        }));
        s.frame(0.0);
        let (slot, panel) = s.frame(120.0);
        assert_eq!(slot, 0.0);
        assert!(panel > -270.0 && panel < 0.0, "{panel}");
    }

    #[test]
    fn a_change_mid_slide_turns_back_from_what_the_screen_shows() {
        let mut s = SideSlide::default();
        s.aim(&open(false));
        s.aim(&shut(true));
        s.frame(0.0);
        let (mid, _) = s.frame(30.0);
        assert!(mid > 0.0 && mid < 220.0);
        s.aim(&open(true));
        assert_eq!(s.shown().0, mid, "it starts from the shown width");
        assert_eq!(s.frame(500.0).0, mid, "its own first frame starts it");
        let back = s.frame(560.0).0;
        assert!(back > mid && back < 220.0, "{back}");
        assert_eq!(s.frame(740.0).0, 220.0);
    }

    #[test]
    fn a_slide_can_be_placed_at_an_offset_before_any_frame_came() {
        let mut s = SideSlide::default();
        s.aim(&open(false));
        assert!(s.aim(&shut(true)));
        let (slot, panel) = s.place(5000.0, 60.0);
        let k = ease(60.0 / SLIDE_MS);
        assert!((f64::from(slot) - 220.0 * (1.0 - k)).abs() < 1e-3, "{slot}");
        assert!((f64::from(panel) + 230.0 * k).abs() < 1e-3, "{panel}");
        assert!(s.running());
        // The slide goes on from there with the clock.
        let (later, _) = s.frame(5000.0 + 120.0);
        assert!(f64::from(later) < f64::from(slot));
        // Past its end it has arrived.
        assert_eq!(s.place(9000.0, 260.0), (0.0, -230.0));
        assert!(!s.running());
        // Offset 0 is the start itself.
        s.aim(&open(true));
        assert_eq!(s.place(1.0, 0.0), (0.0, -230.0));
    }

    #[test]
    fn reduced_motion_jumps() {
        let mut s = SideSlide::default();
        s.aim(&open(false));
        let reduced = SlideAim {
            reduced: true,
            ..shut(true)
        };
        assert!(!s.aim(&reduced));
        assert_eq!(s.shown(), (0.0, -230.0));
    }
}
