//! The agent's touch indicators over the device picture (rule 1321): the finger dot (with its ripples), the swipe
//! trail, the keyboard badge and the edge glow. Pure state on a clock the caller passes in; Slint draws it over the
//! picture, so it is never in the sim host's picture and never in `device_look`.
//!
//! The dot and the glow are the web panel's agent cursor and border as they are
//! ([`flare_surfaces::agent_cursor`]): the same glide timing, rest memory, ripples, fade and "animations off". Their
//! viewport is the device's screen in points (current orientation). This file adds only the trail and the badge,
//! and the hold after the last action.

use std::collections::VecDeque;

use flare_surfaces::agent_cursor::{AgentBorder, AgentCursor, CursorFrame, Point};

use crate::geom::Size;

/// The finger dot's diameter, in device points. A ripple grows from this to twice it.
pub const DOT_PT: f64 = 44.0;
/// How long a swipe's trail stays behind the dot.
pub const TRAIL_MS: u64 = 250;
/// The trail's width, in device points.
pub const TRAIL_PT: f64 = 6.0;
/// How long the dot and the glow stay after the last action before they fade.
pub const HOLD_MS: u64 = 1_500;
/// The keyboard badge shows at least this long, so a key press (done in a frame or two) is seen too.
pub const BADGE_MIN_MS: u64 = 600;

/// The keyboard badge: a pill under the dot (or at the bottom centre with no dot).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Badge {
    /// The agent's name ("Claude", "Codex", "Agent").
    pub agent: String,
    /// "Typing..." while `device_type` runs, or the key's name ("Return") for a key press.
    pub text: String,
}

/// What to draw now, in device points.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IndicatorFrame {
    pub dot: Option<CursorFrame>,
    /// The trail's points, oldest first, each with its opacity (0..1, fading with age).
    pub trail: Vec<(f64, f64, f32)>,
    pub badge: Option<Badge>,
    /// The edge glow's opacity.
    pub glow: Option<f32>,
}

/// The indicators of one device panel.
#[derive(Debug, Default)]
pub struct Indicators {
    cursor: AgentCursor,
    border: AgentBorder,
    screen: Option<Size>,
    trail: VecDeque<(u64, f64, f64)>,
    badge: Option<Badge>,
    /// When the badge was put up, and (after its job) until when it stays.
    badge_from: u64,
    badge_until: Option<u64>,
    /// When the last action ended, while the hold runs (None while a job runs).
    hold_from: Option<u64>,
    /// Animation effects are off.
    still: bool,
}

impl Indicators {
    /// Animation effects off: the dot is placed at once, ripples are one frame, nothing fades.
    pub fn set_still(&mut self, still: bool) {
        self.still = still;
        self.cursor.set_still(still);
        self.border.set_still(still);
    }

    /// The device's screen in points (current orientation). A new size (a rotation) means the dot's resting spot
    /// means nothing any more: the next action appears at its target.
    pub fn set_screen(&mut self, screen: Size) {
        if self.screen != Some(screen) {
            self.screen = Some(screen);
            self.cursor.forget();
            self.trail.clear();
        }
    }

    fn point(&self, x: f64, y: f64) -> Point {
        let s = self.screen.unwrap_or(Size::new(0.0, 0.0));
        Point {
            x,
            y,
            vp_w: s.w,
            vp_h: s.h,
        }
    }

    /// The agent's job started: the glow is up until the hold after it ends.
    pub fn job_started(&mut self) {
        self.border.show();
        self.hold_from = None;
        self.badge = None;
        self.badge_until = None;
    }

    /// The next touch lands at `(x, y)`: the dot glides there (the agent cursor's timing). Returns how long the
    /// job holds the touch for the glide.
    pub fn move_to(&mut self, x: f64, y: f64, now_ms: u64) -> u64 {
        self.border.show();
        self.hold_from = None;
        let p = self.point(x, y);
        self.cursor.point(p, now_ms)
    }

    /// A touch went down at the dot: a ripple.
    pub fn press(&mut self, now_ms: u64) {
        self.cursor.press(now_ms);
    }

    /// A swipe moved on to `(x, y)`: the dot follows the path at once and leaves a trail.
    pub fn drag_to(&mut self, x: f64, y: f64, now_ms: u64) {
        let p = self.point(x, y);
        if self.cursor.position(now_ms) != Some(p) {
            // Placed, not glided: the path is the motion.
            self.cursor.set_still(true);
            self.cursor.point(p, now_ms);
            self.cursor.set_still(self.still);
        }
        self.trail.push_back((now_ms, x, y));
        self.trim(now_ms);
    }

    /// The badge says `text` for `agent` ("Typing...", or a key's name) from `now_ms`.
    pub fn badge(&mut self, agent: &str, text: &str, now_ms: u64) {
        self.badge = Some(Badge {
            agent: agent.to_string(),
            text: text.to_string(),
        });
        self.badge_from = now_ms;
        self.badge_until = None;
    }

    /// The job is over: the hold runs, then the dot and the glow fade. The badge goes now, or once it has shown for
    /// [`BADGE_MIN_MS`].
    pub fn job_ended(&mut self, now_ms: u64) {
        if self.badge.is_some() {
            let until = self.badge_from + BADGE_MIN_MS;
            if until > now_ms && !self.still {
                self.badge_until = Some(until);
            } else {
                self.badge = None;
            }
        }
        self.hold_from = Some(now_ms);
    }

    /// Gone at once, all of it (the person took over, the panel closed, the tab went behind, the window was
    /// minimised).
    pub fn hide(&mut self) {
        self.cursor.hide();
        self.border.hide();
        self.trail.clear();
        self.badge = None;
        self.badge_until = None;
        self.hold_from = None;
    }

    fn trim(&mut self, now_ms: u64) {
        while self
            .trail
            .front()
            .is_some_and(|(t, ..)| now_ms.saturating_sub(*t) >= TRAIL_MS)
        {
            self.trail.pop_front();
        }
    }

    /// What to draw at `now_ms`.
    pub fn frame(&mut self, now_ms: u64) -> IndicatorFrame {
        if let Some(from) = self.hold_from
            && now_ms >= from + HOLD_MS
        {
            self.cursor.fade(from + HOLD_MS);
            self.border.fade(from + HOLD_MS);
            self.hold_from = None;
        }
        self.trim(now_ms);
        if self.badge_until.is_some_and(|u| now_ms >= u) {
            self.badge = None;
            self.badge_until = None;
        }
        let trail = self
            .trail
            .iter()
            .map(|&(t, x, y)| {
                let age = now_ms.saturating_sub(t) as f32 / TRAIL_MS as f32;
                (x, y, (1.0 - age).clamp(0.0, 1.0))
            })
            .collect();
        IndicatorFrame {
            dot: self.cursor.frame(now_ms),
            trail,
            badge: self.badge.clone(),
            glow: self.border.frame(now_ms),
        }
    }

    /// Something moves (a glide, a ripple, a fade, a trail, a hold to end), so the window must draw again soon.
    pub fn needs_tick(&self, now_ms: u64) -> bool {
        self.cursor.needs_tick(now_ms)
            || self.border.needs_tick()
            || !self.trail.is_empty()
            || self.hold_from.is_some()
            || self.badge_until.is_some()
    }

    /// Nothing is drawn.
    pub fn is_hidden(&self) -> bool {
        !self.cursor.shown()
            && !self.border.shown()
            && self.trail.is_empty()
            && self.badge.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_surfaces::agent_cursor::{APPEAR_MS, FADE_MS, RIPPLE_MS, glide_ms};

    const PHONE: Size = Size { w: 402.0, h: 874.0 };

    fn dp(x: f64, y: f64) -> Point {
        Point {
            x,
            y,
            vp_w: 402.0,
            vp_h: 874.0,
        }
    }

    fn ind() -> Indicators {
        let mut i = Indicators::default();
        i.set_screen(PHONE);
        i
    }

    #[test]
    fn the_dot_is_the_agent_cursor_in_device_points() {
        let mut i = ind();
        assert_eq!(i.move_to(100.0, 200.0, 0), APPEAR_MS, "appears first");
        let f = i.frame(0);
        assert_eq!(
            f.dot.as_ref().unwrap().at,
            dp(100.0, 200.0),
            "device points, the screen as viewport"
        );
        assert_eq!(f.glow, Some(1.0));
        // The next touch glides with the cursor's timing.
        let ms = i.move_to(300.0, 600.0, 1_000);
        assert_eq!(ms, glide_ms(dp(100.0, 200.0), dp(300.0, 600.0)));
        assert_eq!(
            i.frame(1_000).dot.unwrap().at,
            dp(100.0, 200.0),
            "never jumps"
        );
        assert_eq!(i.frame(1_000 + ms).dot.unwrap().at, dp(300.0, 600.0));
        // A tap ripples.
        i.press(1_000 + ms);
        assert_eq!(
            i.frame(1_000 + ms + RIPPLE_MS / 2).dot.unwrap().ripples,
            [0.5]
        );
        // After the job, the hold, then the fade; the spot is remembered for the next action.
        i.job_ended(2_000);
        assert!(i.frame(2_000 + HOLD_MS - 1).dot.is_some());
        let f = i.frame(2_000 + HOLD_MS + FADE_MS / 2);
        assert!((f.dot.unwrap().alpha - 0.5).abs() < 1e-6);
        assert!(i.frame(2_000 + HOLD_MS + FADE_MS).dot.is_none());
        assert!(i.is_hidden());
        let ms = i.move_to(100.0, 600.0, 9_000);
        assert_eq!(
            ms,
            glide_ms(dp(300.0, 600.0), dp(100.0, 600.0)),
            "glides from its rest"
        );
        // A rotation forgets the spot.
        i.job_ended(9_000);
        i.frame(9_000 + HOLD_MS + FADE_MS);
        i.set_screen(PHONE.turned());
        assert_eq!(i.move_to(10.0, 10.0, 20_000), APPEAR_MS);
        // Animations off: placed at once.
        let mut i = ind();
        i.set_still(true);
        assert_eq!(i.move_to(10.0, 10.0, 0), 0);
        assert_eq!(i.move_to(300.0, 10.0, 10), 0);
    }

    #[test]
    fn the_trail_keeps_only_the_last_250_ms() {
        let mut i = ind();
        i.move_to(100.0, 100.0, 0);
        for k in 0..=50u32 {
            i.drag_to(100.0, 100.0 + f64::from(k) * 8.0, u64::from(k) * 10);
        }
        // At 500 ms the trail holds the points of 260..500 ms (25 of them), the newest whole.
        let f = i.frame(500);
        assert_eq!(f.trail.len(), 25);
        assert_eq!(f.trail.last().unwrap(), &(100.0, 500.0, 1.0));
        assert!(
            f.trail[0].2 > 0.0 && f.trail[0].2 < 0.2,
            "the oldest nearly gone"
        );
        assert!(
            f.trail.windows(2).all(|w| w[0].2 <= w[1].2),
            "older is fainter"
        );
        assert_eq!(
            f.dot.unwrap().at,
            dp(100.0, 500.0),
            "the dot follows the path, no glide"
        );
        assert!(i.needs_tick(500));
        // 250 ms after the last point the trail is gone.
        assert_eq!(i.frame(749).trail.len(), 1);
        assert!(i.frame(750).trail.is_empty());
    }

    #[test]
    fn the_badge_shows_while_typing_and_names_the_key() {
        let mut i = ind();
        assert_eq!(i.frame(0).badge, None);
        i.job_started();
        i.badge("Claude", "Typing...", 10);
        let b = i.frame(10).badge.unwrap();
        assert_eq!((b.agent.as_str(), b.text.as_str()), ("Claude", "Typing..."));
        assert!(
            i.frame(10).dot.is_none(),
            "no dot: the badge sits at the bottom centre"
        );
        i.badge("Claude", crate::keymap::label("return").unwrap(), 20);
        assert_eq!(i.frame(20).badge.unwrap().text, "Return");
        i.job_ended(30);
        // A key press is done in a frame: its badge stays long enough to be seen, then goes.
        assert!(i.frame(30).badge.is_some(), "seen for a moment");
        assert!(i.needs_tick(30));
        assert_eq!(
            i.frame(20 + BADGE_MIN_MS).badge,
            None,
            "then gone with the job"
        );
        assert_eq!(i.frame(30).glow, Some(1.0), "the glow holds");
        // A long typing job's badge goes as the job ends.
        i.job_started();
        i.badge("Claude", "Typing...", 1_000);
        i.job_ended(1_000 + BADGE_MIN_MS + 1);
        assert_eq!(i.frame(1_000 + BADGE_MIN_MS + 1).badge, None);
    }

    #[test]
    fn hide_clears_dot_trail_badge_and_glow_at_once() {
        let mut i = ind();
        i.job_started();
        i.move_to(50.0, 50.0, 0);
        i.press(0);
        i.drag_to(60.0, 60.0, 10);
        i.badge("Codex", "Typing...", 0);
        let f = i.frame(20);
        assert!(f.dot.is_some() && !f.trail.is_empty() && f.badge.is_some() && f.glow.is_some());
        i.hide();
        assert_eq!(i.frame(20), IndicatorFrame::default());
        assert!(i.is_hidden());
        assert!(!i.needs_tick(20));
        // The next action appears at its target, no glide from where it was.
        assert_eq!(i.move_to(300.0, 300.0, 30), APPEAR_MS);
        assert_eq!(i.frame(30).dot.unwrap().at, dp(300.0, 300.0));
    }
}
