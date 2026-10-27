//! The agent's cursor on the web panel (Surfaces stage 4): where the arrow is, its glide to the next
//! target, the ripples of its presses and its fade, and the border round the page while the agent uses
//! it. Pure state on a clock the caller passes in; the window code maps the point to the screen and draws
//! it (`flare_win::agent_cursor`).
//!
//! A point is in CSS pixels of the page's visual viewport, with that viewport's size, as the DevTools
//! Protocol's input events take it. The arrow never jumps (Max, 2026-10-27): it glides from where it is
//! to each next target in [`glide_ms`] (ease-in-out), and after its fade it remembers where it rested, so
//! the next action on the same page shows it there and glides on. Only the first action on a page, one
//! after a navigation or a change of the viewport's size, and one after a hide (the person took over, the
//! page went out of sight) appear at the target. It ripples on each press and fades in [`FADE_MS`] once
//! the window says so. With animation effects off nothing glides or fades: the arrow is placed at once.

/// The shortest and longest glide, and what each CSS pixel of the way adds between them.
pub const GLIDE_MIN_MS: u64 = 350;
pub const GLIDE_MAX_MS: u64 = 700;
const GLIDE_MS_PER_PX: f64 = 0.5;
/// When the arrow appears at its target (nothing to glide from), the events wait this long so the person
/// sees it before the press.
pub const APPEAR_MS: u64 = 180;
/// How long a ripple grows and fades.
pub const RIPPLE_MS: u64 = 400;
/// How long the arrow and the border take to fade out after the ribbon's hold.
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

impl Point {
    fn same_viewport(&self, o: &Point) -> bool {
        self.vp_w == o.vp_w && self.vp_h == o.vp_h
    }
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
    ms: u64,
}

/// How long the arrow takes from `from` to `to`: [`GLIDE_MIN_MS`] plus half a millisecond a CSS pixel,
/// at most [`GLIDE_MAX_MS`], so a short hop is still seen and a long one does not drag.
pub fn glide_ms(from: Point, to: Point) -> u64 {
    let d = (to.x - from.x).hypot(to.y - from.y);
    let ms = GLIDE_MIN_MS as f64 + d * GLIDE_MS_PER_PX;
    (ms.round() as u64).clamp(GLIDE_MIN_MS, GLIDE_MAX_MS)
}

/// Ease-in-out (cubic): it starts and stops softly.
fn ease_in_out(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        let u = -2.0 * t + 2.0;
        1.0 - u * u * u / 2.0
    }
}

/// The cursor's state. `Default` is hidden, with motion.
#[derive(Clone, Debug, Default)]
pub struct AgentCursor {
    /// Where the arrow rests (or is gliding to); None while hidden.
    at: Option<Point>,
    glide: Option<Glide>,
    /// When each ripple started (one may start a little later than its press, see [`RIPPLE_GAP_MS`]).
    ripples: Vec<u64>,
    fade_ms: Option<u64>,
    /// Where the tip was when the arrow last faded, while hidden: the next action glides from here.
    rest: Option<Point>,
    /// Animation effects are off: no glide, no fade.
    still: bool,
    /// How many presses rippled since the window began (the e2e reads it).
    presses: u64,
}

impl AgentCursor {
    pub fn shown(&self) -> bool {
        self.at.is_some()
    }

    pub fn presses(&self) -> u64 {
        self.presses
    }

    /// Whether a spot is remembered for the next action to glide from (the arrow faded on this page).
    pub fn remembers(&self) -> bool {
        self.rest.is_some()
    }

    /// Animation effects off (`still`): the arrow is placed at once and goes at once.
    pub fn set_still(&mut self, still: bool) {
        self.still = still;
    }

    /// Where the tip is drawn at `now_ms` (partway through a glide, or where it rests).
    pub fn position(&self, now_ms: u64) -> Option<Point> {
        let to = self.at?;
        let Some(g) = self.glide else {
            return Some(to);
        };
        let t = ease_in_out(now_ms.saturating_sub(g.start_ms) as f64 / g.ms.max(1) as f64);
        Some(Point {
            x: g.from.x + (to.x - g.from.x) * t,
            y: g.from.y + (to.y - g.from.y) * t,
            vp_w: to.vp_w,
            vp_h: to.vp_h,
        })
    }

    /// The next action lands at `p`: the arrow glides there from where it is drawn, or from where it
    /// rested before its fade (on the same viewport), and appears there when neither is known. A fade
    /// under way is called off. Answers how long the job holds its events: the glide's time,
    /// [`APPEAR_MS`] for an appearance, 0 when the arrow is already there or nothing moves (no motion).
    pub fn point(&mut self, p: Point, now_ms: u64) -> u64 {
        let rest = self.rest.take().filter(|r| r.same_viewport(&p));
        let from = self.position(now_ms).or(rest);
        let mut ms = 0;
        match from {
            Some(from) if !self.still && (from.x != p.x || from.y != p.y) => {
                ms = glide_ms(from, p);
                self.glide = Some(Glide {
                    from,
                    start_ms: now_ms,
                    ms,
                });
            }
            Some(_) => self.glide = None,
            None => {
                self.glide = None;
                self.ripples.clear();
                if !self.still {
                    ms = APPEAR_MS;
                }
            }
        }
        self.at = Some(p);
        self.fade_ms = None;
        ms
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

    /// The ribbon's hold is over: the arrow fades out (a second call is a no-op), or goes at once with
    /// animation effects off. Either way its spot is remembered.
    pub fn fade(&mut self, now_ms: u64) {
        if self.at.is_some() && self.fade_ms.is_none() {
            if self.still {
                self.rest_at(now_ms);
            } else {
                self.fade_ms = Some(now_ms);
            }
        }
    }

    pub fn fading(&self) -> bool {
        self.fade_ms.is_some()
    }

    /// Gone at once (the person took over, the page went out of sight): the next action appears at its
    /// target.
    pub fn hide(&mut self) {
        *self = Self {
            presses: self.presses,
            still: self.still,
            ..Self::default()
        };
    }

    /// The page changed (a navigation): the spot the arrow rested at means nothing on the new page.
    pub fn forget(&mut self) {
        self.rest = None;
    }

    /// Hidden after its fade, its tip's spot kept for the next action.
    fn rest_at(&mut self, now_ms: u64) {
        let rest = self.position(now_ms);
        self.hide();
        self.rest = rest;
    }

    /// What to draw at `now_ms`, None when nothing is. A finished glide, finished ripples and a finished
    /// fade are let go of here.
    pub fn frame(&mut self, now_ms: u64) -> Option<CursorFrame> {
        if self.glide.is_some_and(|g| now_ms >= g.start_ms + g.ms) {
            self.glide = None;
        }
        self.ripples.retain(|s| now_ms < s + RIPPLE_MS);
        let alpha = match self.fade_ms {
            Some(f) => 1.0 - now_ms.saturating_sub(f) as f32 / FADE_MS as f32,
            None => 1.0,
        };
        if alpha <= 0.0 {
            self.rest_at(now_ms);
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
            && (self.glide.is_some_and(|g| now_ms < g.start_ms + g.ms)
                || self.ripples.iter().any(|s| now_ms < s + RIPPLE_MS)
                || self.fade_ms.is_some())
    }
}

/// The light orange border round the page while the agent uses it (Max, 2026-10-27): on from the agent's
/// first action until the ribbon's hold ends, then it fades with the arrow in [`FADE_MS`]; gone at once
/// when the person takes over. `Default` is off, with motion.
#[derive(Clone, Debug, Default)]
pub struct AgentBorder {
    on: bool,
    fade_ms: Option<u64>,
    /// Animation effects are off: it goes at once.
    still: bool,
}

impl AgentBorder {
    pub fn shown(&self) -> bool {
        self.on
    }

    pub fn set_still(&mut self, still: bool) {
        self.still = still;
    }

    /// The agent is using the page: the border is up, whole (a fade under way is called off).
    pub fn show(&mut self) {
        self.on = true;
        self.fade_ms = None;
    }

    /// The ribbon's hold is over: it fades (a second call is a no-op), or goes at once without motion.
    pub fn fade(&mut self, now_ms: u64) {
        if self.on && self.fade_ms.is_none() {
            if self.still {
                self.hide();
            } else {
                self.fade_ms = Some(now_ms);
            }
        }
    }

    /// Gone at once.
    pub fn hide(&mut self) {
        self.on = false;
        self.fade_ms = None;
    }

    /// Its opacity at `now_ms` (0..1), None when it is not drawn; a finished fade turns it off here.
    pub fn frame(&mut self, now_ms: u64) -> Option<f32> {
        if !self.on {
            return None;
        }
        let alpha = match self.fade_ms {
            Some(f) => 1.0 - now_ms.saturating_sub(f) as f32 / FADE_MS as f32,
            None => 1.0,
        };
        if alpha <= 0.0 {
            self.hide();
            return None;
        }
        Some(alpha.min(1.0))
    }

    /// Is it fading, so the window must draw again soon?
    pub fn needs_tick(&self) -> bool {
        self.on && self.fade_ms.is_some()
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

    fn close(a: Point, b: Point) -> bool {
        (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9
    }

    #[test]
    fn the_first_show_is_at_the_target_with_no_glide() {
        let mut c = AgentCursor::default();
        assert!(!c.shown());
        assert_eq!(c.frame(0), None);
        assert_eq!(
            c.point(p(100.0, 50.0), 1_000),
            APPEAR_MS,
            "seen before the press"
        );
        let f = c.frame(1_000).unwrap();
        assert_eq!(f.at, p(100.0, 50.0));
        assert_eq!(f.alpha, 1.0);
        assert!(f.ripples.is_empty());
        assert!(!c.needs_tick(1_000), "nothing moves");
    }

    #[test]
    fn the_glide_takes_longer_the_further_it_goes_within_its_bounds() {
        assert_eq!(glide_ms(p(0.0, 0.0), p(0.0, 0.0)), GLIDE_MIN_MS);
        assert_eq!(glide_ms(p(0.0, 0.0), p(30.0, 40.0)), 375, "50 px");
        assert_eq!(glide_ms(p(0.0, 0.0), p(600.0, 0.0)), 650);
        assert_eq!(glide_ms(p(0.0, 0.0), p(700.0, 0.0)), GLIDE_MAX_MS);
        assert_eq!(glide_ms(p(0.0, 0.0), p(4_000.0, 3_000.0)), GLIDE_MAX_MS);
        assert_eq!(glide_ms(p(100.0, 0.0), p(0.0, 0.0)), 400, "either way");
    }

    #[test]
    fn the_ease_starts_and_stops_softly_and_is_half_way_at_half_time() {
        assert_eq!(ease_in_out(0.0), 0.0);
        assert_eq!(ease_in_out(1.0), 1.0);
        assert!((ease_in_out(0.5) - 0.5).abs() < 1e-12);
        // Slow at both ends: a tenth of the time covers far less than a tenth of the way.
        assert!((ease_in_out(0.1) - 0.004).abs() < 1e-12);
        assert!((1.0 - ease_in_out(0.9) - 0.004).abs() < 1e-12);
        // Symmetric, and never back.
        for i in 0..=100 {
            let t = f64::from(i) / 100.0;
            assert!((ease_in_out(t) + ease_in_out(1.0 - t) - 1.0).abs() < 1e-12);
            assert!(ease_in_out(t + 0.01) >= ease_in_out(t));
        }
    }

    #[test]
    fn a_glide_eases_from_where_it_was_to_the_next_target_for_its_time() {
        let mut c = AgentCursor::default();
        c.point(p(0.0, 0.0), 0);
        let ms = c.point(p(300.0, 400.0), 1_000);
        assert_eq!(ms, 600, "500 px");
        assert!(c.needs_tick(1_000));
        assert_eq!(
            c.frame(1_000).unwrap().at,
            p(0.0, 0.0),
            "it starts where it was"
        );
        // A tenth of the time in, it has barely left (ease-in).
        let early = c.frame(1_060).unwrap().at;
        assert!(close(early, p(1.2, 1.6)), "{early:?}");
        let mid = c.frame(1_300).unwrap().at;
        assert!(close(mid, p(150.0, 200.0)), "{mid:?}");
        assert!(c.needs_tick(1_599));
        assert_eq!(c.frame(1_600).unwrap().at, p(300.0, 400.0));
        assert!(!c.needs_tick(1_600));
        assert_eq!(c.point(p(300.0, 400.0), 1_700), 0, "already there");
        // A new target mid-glide starts from where the arrow is drawn, not where it was going.
        c.point(p(0.0, 0.0), 2_000);
        c.point(p(50.0, 0.0), 2_300);
        let from = c.frame(2_300).unwrap().at;
        assert!(close(from, p(150.0, 200.0)), "{from:?}");
    }

    #[test]
    fn after_its_fade_the_next_action_glides_from_where_it_rested() {
        let mut c = AgentCursor::default();
        c.point(p(100.0, 100.0), 0);
        c.fade(2_000);
        assert_eq!(c.frame(2_000 + FADE_MS), None);
        assert!(!c.shown());
        assert!(c.remembers());
        // Three seconds on, the next click: shown at the old spot, gliding on.
        let ms = c.point(p(400.0, 500.0), 5_000);
        assert_eq!(ms, glide_ms(p(100.0, 100.0), p(400.0, 500.0)));
        assert_eq!(c.frame(5_000).unwrap().at, p(100.0, 100.0), "no jump");
        assert_eq!(c.frame(5_000).unwrap().alpha, 1.0);
        assert_eq!(c.frame(5_000 + ms).unwrap().at, p(400.0, 500.0));
        assert!(!c.remembers(), "used");
        // Faded mid-glide: it rests where it was drawn.
        let mut c = AgentCursor::default();
        c.point(p(0.0, 0.0), 0);
        let ms = c.point(p(200.0, 0.0), 0);
        c.fade(ms / 2 - FADE_MS);
        assert_eq!(c.frame(ms / 2), None);
        c.point(p(0.0, 0.0), 10_000);
        assert!(close(c.frame(10_000).unwrap().at, p(100.0, 0.0)));
    }

    #[test]
    fn a_navigation_a_new_viewport_or_a_hide_forgets_the_spot_so_the_arrow_appears() {
        let faded = || {
            let mut c = AgentCursor::default();
            c.point(p(100.0, 100.0), 0);
            c.fade(1_000);
            c.frame(1_000 + FADE_MS);
            assert!(c.remembers());
            c
        };
        let mut c = faded();
        c.forget();
        assert_eq!(c.point(p(300.0, 300.0), 5_000), APPEAR_MS);
        assert_eq!(c.frame(5_000).unwrap().at, p(300.0, 300.0));
        let mut c = faded();
        let other = Point {
            vp_w: 900.0,
            ..p(300.0, 300.0)
        };
        assert_eq!(
            c.point(other, 5_000),
            APPEAR_MS,
            "the viewport changed size"
        );
        assert_eq!(c.frame(5_000).unwrap().at, other);
        let mut c = faded();
        c.hide();
        assert!(!c.remembers());
        assert_eq!(c.point(p(300.0, 300.0), 5_000), APPEAR_MS);
    }

    #[test]
    fn with_no_motion_the_arrow_is_placed_and_goes_at_once() {
        let mut c = AgentCursor::default();
        c.set_still(true);
        assert_eq!(c.point(p(0.0, 0.0), 0), 0, "placed, no wait");
        assert_eq!(c.point(p(300.0, 400.0), 100), 0);
        assert_eq!(c.frame(100).unwrap().at, p(300.0, 400.0));
        assert!(!c.needs_tick(100));
        c.fade(200);
        assert!(!c.shown(), "no fade");
        assert!(c.remembers());
        assert_eq!(c.point(p(10.0, 10.0), 300), 0);
        assert_eq!(c.frame(300).unwrap().at, p(10.0, 10.0));
        c.hide();
        c.point(p(0.0, 0.0), 400);
        assert_eq!(c.point(p(50.0, 0.0), 400), 0, "still after a hide");
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
        let ms = c.point(p(20.0, 10.0), 150);
        assert!(!c.fading());
        assert_eq!(c.frame(150 + ms).unwrap().alpha, 1.0);
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

    #[test]
    fn the_border_is_up_while_the_agent_works_fades_after_and_goes_at_once_on_a_hide() {
        let mut b = AgentBorder::default();
        assert_eq!(b.frame(0), None);
        b.show();
        assert_eq!(b.frame(0), Some(1.0));
        assert!(!b.needs_tick(), "nothing moves while it is up");
        b.fade(1_000);
        b.fade(1_100);
        assert!(b.needs_tick());
        assert!((b.frame(1_000 + FADE_MS / 2).unwrap() - 0.5).abs() < 1e-6);
        assert_eq!(b.frame(1_000 + FADE_MS), None);
        assert!(!b.shown() && !b.needs_tick());
        // The next action brings it back whole, a fade under way too.
        b.show();
        b.fade(2_000);
        b.show();
        assert_eq!(b.frame(2_000 + FADE_MS), Some(1.0));
        // A takeover: gone at once.
        b.hide();
        assert_eq!(b.frame(2_000 + FADE_MS), None);
        // No motion: no fade.
        b.set_still(true);
        b.show();
        b.fade(3_000);
        assert_eq!(b.frame(3_000), None);
    }
}
