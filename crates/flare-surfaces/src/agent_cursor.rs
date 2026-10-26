//! The agent's cursor on the web panel (Surfaces stage 4): where the arrow is, its glide to the next
//! target, the ripples of its presses and its fade. Pure state on a clock the caller passes in; the window
//! code maps the point to the screen and draws it (`flare_win::agent_cursor`).
//!
//! A point is in CSS pixels of the page's visual viewport, with that viewport's size, as the DevTools
//! Protocol's input events take it. The arrow appears at its first target (no glide from nowhere),
//! glides from where it is to each next one in [`GLIDE_MS`] (ease-out), ripples on each press, and fades
//! in [`FADE_MS`] once the window says so. A hide is at once.

/// How long the arrow takes from where it was to the next target; the job holds its events this long.
pub const GLIDE_MS: u64 = 180;
/// How long a ripple grows and fades.
pub const RIPPLE_MS: u64 = 400;
/// How long the arrow takes to fade out after the ribbon's hold.
pub const FADE_MS: u64 = 200;
/// Two presses closer than this ripple one after the other, so a double click shows two.
pub const RIPPLE_GAP_MS: u64 = 120;

/// Where a pointer action lands: CSS pixels of the visual viewport, and that viewport's size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
    pub vp_w: f64,
    pub vp_h: f64,
}

/// What to draw now: the tip's point, the arrow's opacity (0..1) and the ripples' progress (0..1 each).
#[derive(Clone, Debug, PartialEq)]
pub struct CursorFrame {
    pub at: Point,
    pub alpha: f32,
    pub ripples: Vec<f32>,
}

#[derive(Clone, Copy, Debug)]
struct Glide {
    from: Point,
    start_ms: u64,
}

/// The cursor's state. `Default` is hidden.
#[derive(Clone, Debug, Default)]
pub struct AgentCursor {
    /// Where the arrow rests (or is gliding to); None while hidden.
    at: Option<Point>,
    glide: Option<Glide>,
    /// When each ripple started (one may start a little later than its press, see [`RIPPLE_GAP_MS`]).
    ripples: Vec<u64>,
    fade_ms: Option<u64>,
    /// How many presses rippled since the window began (the e2e reads it).
    presses: u64,
}

/// Ease-out (cubic): fast at first, settling at the end.
fn ease_out(t: f64) -> f64 {
    let u = 1.0 - t.clamp(0.0, 1.0);
    1.0 - u * u * u
}

impl AgentCursor {
    pub fn shown(&self) -> bool {
        self.at.is_some()
    }

    pub fn presses(&self) -> u64 {
        self.presses
    }

    /// Where the tip is drawn at `now_ms` (partway through a glide, or where it rests).
    pub fn position(&self, now_ms: u64) -> Option<Point> {
        let to = self.at?;
        let Some(g) = self.glide else {
            return Some(to);
        };
        let t = ease_out(now_ms.saturating_sub(g.start_ms) as f64 / GLIDE_MS as f64);
        Some(Point {
            x: g.from.x + (to.x - g.from.x) * t,
            y: g.from.y + (to.y - g.from.y) * t,
            vp_w: to.vp_w,
            vp_h: to.vp_h,
        })
    }

    /// The next action lands at `p`: the arrow glides there from where it is, or appears there when it
    /// was hidden. A fade under way is called off.
    pub fn point(&mut self, p: Point, now_ms: u64) {
        match self.position(now_ms) {
            Some(from) if from.x != p.x || from.y != p.y => {
                self.glide = Some(Glide {
                    from,
                    start_ms: now_ms,
                });
            }
            Some(_) => self.glide = None,
            None => {
                self.glide = None;
                self.ripples.clear();
            }
        }
        self.at = Some(p);
        self.fade_ms = None;
    }

    /// A button went down at the tip: a ripple, after the previous one by at least [`RIPPLE_GAP_MS`].
    pub fn press(&mut self, now_ms: u64) {
        if self.at.is_none() {
            return;
        }
        let start = self
            .ripples
            .last()
            .map_or(now_ms, |last| now_ms.max(last + RIPPLE_GAP_MS));
        self.ripples.push(start);
        self.presses += 1;
    }

    /// The ribbon's hold is over: the arrow fades out (a second call is a no-op).
    pub fn fade(&mut self, now_ms: u64) {
        if self.at.is_some() && self.fade_ms.is_none() {
            self.fade_ms = Some(now_ms);
        }
    }

    pub fn fading(&self) -> bool {
        self.fade_ms.is_some()
    }

    /// Gone at once (the person took over, the page went out of sight).
    pub fn hide(&mut self) {
        let presses = self.presses;
        *self = Self {
            presses,
            ..Self::default()
        };
    }

    /// What to draw at `now_ms`, None when nothing is. A finished glide, finished ripples and a finished
    /// fade are let go of here.
    pub fn frame(&mut self, now_ms: u64) -> Option<CursorFrame> {
        if self.glide.is_some_and(|g| now_ms >= g.start_ms + GLIDE_MS) {
            self.glide = None;
        }
        self.ripples.retain(|s| now_ms < s + RIPPLE_MS);
        let alpha = match self.fade_ms {
            Some(f) => 1.0 - now_ms.saturating_sub(f) as f32 / FADE_MS as f32,
            None => 1.0,
        };
        if alpha <= 0.0 {
            self.hide();
            return None;
        }
        let at = self.position(now_ms)?;
        let ripples = self
            .ripples
            .iter()
            .filter(|s| **s <= now_ms)
            .map(|s| (now_ms - s) as f32 / RIPPLE_MS as f32)
            .collect();
        Some(CursorFrame {
            at,
            alpha: alpha.min(1.0),
            ripples,
        })
    }

    /// Is something moving (a glide, a ripple, a fade), so the window must draw again soon?
    pub fn needs_tick(&self, now_ms: u64) -> bool {
        self.at.is_some()
            && (self.glide.is_some_and(|g| now_ms < g.start_ms + GLIDE_MS)
                || self.ripples.iter().any(|s| now_ms < s + RIPPLE_MS)
                || self.fade_ms.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> Point {
        Point {
            x,
            y,
            vp_w: 800.0,
            vp_h: 500.0,
        }
    }

    #[test]
    fn the_first_show_is_at_the_target_with_no_glide() {
        let mut c = AgentCursor::default();
        assert!(!c.shown());
        assert_eq!(c.frame(0), None);
        c.point(p(100.0, 50.0), 1_000);
        let f = c.frame(1_000).unwrap();
        assert_eq!(f.at, p(100.0, 50.0));
        assert_eq!(f.alpha, 1.0);
        assert!(f.ripples.is_empty());
        assert!(!c.needs_tick(1_000), "nothing moves");
    }

    #[test]
    fn a_glide_eases_out_from_where_it_was_to_the_next_target() {
        let mut c = AgentCursor::default();
        c.point(p(0.0, 0.0), 0);
        c.point(p(100.0, 200.0), 1_000);
        assert!(c.needs_tick(1_000));
        assert_eq!(
            c.frame(1_000).unwrap().at,
            p(0.0, 0.0),
            "it starts where it was"
        );
        let mid = c.frame(1_000 + GLIDE_MS / 2).unwrap().at;
        // Ease-out: past half the way at half the time (1 - 0.5^3 = 0.875).
        assert!(
            (mid.x - 87.5).abs() < 1e-9 && (mid.y - 175.0).abs() < 1e-9,
            "{mid:?}"
        );
        assert_eq!(c.frame(1_000 + GLIDE_MS).unwrap().at, p(100.0, 200.0));
        assert!(!c.needs_tick(1_000 + GLIDE_MS));
        // A new target mid-glide starts from where the arrow is drawn, not where it was going.
        c.point(p(0.0, 0.0), 2_000);
        c.point(p(50.0, 0.0), 2_000 + GLIDE_MS / 2);
        let from = c.frame(2_000 + GLIDE_MS / 2).unwrap().at;
        assert!((from.x - 100.0 * (1.0 - 0.875)).abs() < 1e-9, "{from:?}");
    }

    #[test]
    fn a_ripple_grows_for_its_time_and_is_gone_after() {
        let mut c = AgentCursor::default();
        c.press(0);
        assert_eq!(c.presses(), 0, "no arrow, no ripple");
        c.point(p(10.0, 10.0), 0);
        c.press(100);
        assert!(
            c.frame(99).unwrap().ripples.is_empty(),
            "not before its press"
        );
        assert_eq!(c.frame(100).unwrap().ripples, [0.0]);
        assert_eq!(c.frame(300).unwrap().ripples, [0.5]);
        assert!(c.needs_tick(300));
        assert!(c.frame(100 + RIPPLE_MS).unwrap().ripples.is_empty());
        assert!(!c.needs_tick(100 + RIPPLE_MS));
        assert_eq!(c.presses(), 1);
    }

    #[test]
    fn a_double_click_ripples_twice_one_after_the_other() {
        let mut c = AgentCursor::default();
        c.point(p(10.0, 10.0), 0);
        c.press(10);
        c.press(14);
        assert_eq!(c.presses(), 2);
        assert_eq!(
            c.frame(20).unwrap().ripples.len(),
            1,
            "the second waits its turn"
        );
        let f = c.frame(10 + RIPPLE_GAP_MS).unwrap();
        assert_eq!(f.ripples.len(), 2);
    }

    #[test]
    fn the_fade_takes_its_time_and_then_the_arrow_is_gone() {
        let mut c = AgentCursor::default();
        c.point(p(10.0, 10.0), 0);
        c.fade(1_000);
        assert!(c.needs_tick(1_000));
        assert_eq!(c.frame(1_000).unwrap().alpha, 1.0);
        assert!((c.frame(1_000 + FADE_MS / 2).unwrap().alpha - 0.5).abs() < 1e-6);
        assert_eq!(c.frame(1_000 + FADE_MS), None);
        assert!(!c.shown());
        assert!(!c.needs_tick(1_000 + FADE_MS));
        // A new action while fading brings it back whole.
        let mut c = AgentCursor::default();
        c.point(p(10.0, 10.0), 0);
        c.fade(100);
        c.point(p(20.0, 10.0), 150);
        assert!(!c.fading());
        assert_eq!(c.frame(150 + GLIDE_MS).unwrap().alpha, 1.0);
    }

    #[test]
    fn a_hide_is_immediate_and_the_next_show_appears_at_its_target() {
        let mut c = AgentCursor::default();
        c.point(p(10.0, 10.0), 0);
        c.press(0);
        c.hide();
        assert!(!c.shown());
        assert_eq!(c.frame(1), None);
        assert!(!c.needs_tick(1));
        assert_eq!(c.presses(), 1, "the count is kept");
        c.point(p(300.0, 300.0), 5);
        assert_eq!(
            c.frame(5).unwrap().at,
            p(300.0, 300.0),
            "no glide from the old place"
        );
        assert!(c.frame(5).unwrap().ripples.is_empty());
    }
}
