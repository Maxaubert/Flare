//! Mark motion (Task 14): the phases of the marks and the still set, as pure functions of the
//! clock. Ported from Prism Terminal's `marks.css` timings and `marksPause.ts`. Nothing here reads
//! a clock: the caller passes the animation time in (decision D5).

use crate::tab_mark::{Indicator, MarkState};

pub const SPIN_MS: u64 = 1000;
pub const RUN_MS: u64 = 1250;
pub const BREATHE_MS: u64 = 2000;
pub const FLOW_LINE_MS: u64 = 8000;
/// One tab's width of the rainbow line's tile, in CSS px.
pub const FLOW_TILE_CSS: f32 = 114.0;
/// The run bar is 42% of the line.
pub const RUN_BAR: f32 = 0.42;
/// The resume skeleton's pulse (`animate-pulse`, 2 s).
pub const PULSE_MS: u64 = 2000;

/// Where every moving mark is at one instant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MarkPhase {
    pub still: bool,
    pub spin_deg: f32,
    pub breathe: f32,
    pub run_t: f32,
    pub flow_px: f32,
}

/// Reduced motion or animation effects off: every mark stands still.
pub const STILL: MarkPhase = MarkPhase {
    still: true,
    spin_deg: 0.0,
    breathe: 1.0,
    run_t: 0.0,
    flow_px: 0.0,
};

/// The phase of every mark at `ms` (animation time).
pub fn mark_phase(ms: u64) -> MarkPhase {
    MarkPhase {
        still: false,
        spin_deg: (ms % SPIN_MS) as f32 / SPIN_MS as f32 * 360.0,
        breathe: breathe_at((ms % BREATHE_MS) as f32 / BREATHE_MS as f32),
        run_t: ease_in_out((ms % RUN_MS) as f32 / RUN_MS as f32),
        flow_px: (ms % FLOW_LINE_MS) as f32 / FLOW_LINE_MS as f32 * FLOW_TILE_CSS,
    }
}

/// The breathing keyframes of `marks.css`, at every 10 %.
const BREATHE_KEYS: [f32; 11] = [
    1.0, 0.905, 0.655, 0.345, 0.095, 0.0, 0.095, 0.345, 0.655, 0.905, 1.0,
];

/// The breathing opacity at a fraction of one breath (0 to 1), linear between the keyframes.
pub fn breathe_at(frac: f32) -> f32 {
    let f = frac.clamp(0.0, 1.0) * 10.0;
    let i = (f.floor() as usize).min(9);
    let t = f - i as f32;
    BREATHE_KEYS[i] + (BREATHE_KEYS[i + 1] - BREATHE_KEYS[i]) * t
}

/// The curve's x and y for a parameter `s`, with the control points of `cubic-bezier(0.42, 0,
/// 0.58, 1)`.
fn bezier_x(s: f64) -> f64 {
    let (x1, x2) = (0.42, 0.58);
    3.0 * (1.0 - s).powi(2) * s * x1 + 3.0 * (1.0 - s) * s * s * x2 + s * s * s
}

fn bezier_x_slope(s: f64) -> f64 {
    let (x1, x2) = (0.42, 0.58);
    3.0 * (1.0 - s).powi(2) * x1 + 6.0 * (1.0 - s) * s * (x2 - x1) + 3.0 * s * s * (1.0 - x2)
}

fn bezier_y(s: f64) -> f64 {
    3.0 * (1.0 - s) * s * s + s * s * s
}

/// `cubic-bezier(0.42, 0, 0.58, 1)` at progress `t`: solve x(s) = t by Newton (at most 8 steps),
/// then bisection as the safety net, and return y(s).
pub fn ease_in_out(t: f32) -> f32 {
    let t = f64::from(t.clamp(0.0, 1.0));
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }
    let mut s = t;
    for _ in 0..8 {
        let err = bezier_x(s) - t;
        let slope = bezier_x_slope(s);
        if err.abs() < 1e-9 || slope.abs() < 1e-12 {
            break;
        }
        s = (s - err / slope).clamp(0.0, 1.0);
    }
    if (bezier_x(s) - t).abs() > 1e-9 {
        let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
        for _ in 0..60 {
            let mid = (lo + hi) / 2.0;
            if bezier_x(mid) < t {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        s = (lo + hi) / 2.0;
    }
    bezier_y(s) as f32
}

/// The skeleton's pulse at `ms`: 1.0 down to 0.5 and back, eased.
pub fn pulse_at(ms: u64) -> f32 {
    let tri = (ms % PULSE_MS) as f32 / PULSE_MS as f32;
    let up = if tri < 0.5 {
        tri * 2.0
    } else {
        (1.0 - tri) * 2.0
    };
    1.0 - 0.5 * ease_in_out(up)
}

/// The run bar's left edge for a line of width `w`.
pub fn run_bar_x(run_t: f32, w: f32) -> f32 {
    -RUN_BAR * w + run_t * (1.0 + RUN_BAR) * w
}

/// Is anything moving? A working tab, a question, a finished rainbow line on the Bar style, or a
/// tab still coming back (`any_loading`) needs the mark clock.
pub fn needs_clock(
    states: &[MarkState],
    indicator: Indicator,
    rainbow: bool,
    any_loading: bool,
) -> bool {
    any_loading
        || states.iter().any(|state| match state {
            MarkState::Working | MarkState::Question => true,
            MarkState::Done => indicator == Indicator::Bar && rainbow,
            MarkState::Failed => false,
        })
}

/// A window nobody can see moves nothing. While minimised the clock stands; restoring goes on
/// from where it stood.
#[derive(Default)]
pub struct MarksPause {
    minimized: bool,
    paused_at: Option<u64>,
    lost: u64,
}

impl MarksPause {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_minimized(&mut self, minimized: bool, now: u64) {
        if minimized && !self.minimized {
            self.paused_at = Some(now);
        } else if !minimized
            && self.minimized
            && let Some(at) = self.paused_at.take()
        {
            self.lost += now.saturating_sub(at);
        }
        self.minimized = minimized;
    }

    pub fn paused(&self) -> bool {
        self.minimized
    }

    /// The animation time to feed `mark_phase`: wall time minus the time spent paused.
    pub fn clock(&self, now: u64) -> u64 {
        self.paused_at.unwrap_or(now).saturating_sub(self.lost)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_motion_has_a_rule_and_the_still_set_stills_them_all() {
        let a = mark_phase(100);
        let b = mark_phase(600);
        assert!(!a.still);
        assert_ne!(a.spin_deg, b.spin_deg);
        assert_ne!(a.breathe, b.breathe);
        assert_ne!(a.run_t, b.run_t);
        assert_ne!(a.flow_px, b.flow_px);
        let still = STILL;
        assert!(still.still);
        assert_eq!(STILL.spin_deg, 0.0);
        assert_eq!(STILL.breathe, 1.0);
        assert_eq!(STILL.flow_px, 0.0);
        let src = include_str!("mark_motion.rs");
        assert!(!src.contains(concat!("Inst", "ant")));
        assert!(!src.contains(concat!("System", "Time")));
    }

    #[test]
    fn spin_is_one_second_per_turn() {
        assert_eq!(mark_phase(0).spin_deg, 0.0);
        assert_eq!(mark_phase(250).spin_deg, 90.0);
        assert_eq!(mark_phase(1000).spin_deg, 0.0);
        assert!((mark_phase(999).spin_deg - 359.6).abs() < 0.5);
    }

    #[test]
    fn breathe_follows_the_cosine_keyframes() {
        assert_eq!(breathe_at(0.0), 1.0);
        assert!((breathe_at(0.05) - 0.9525).abs() < 1e-4);
        assert!((breathe_at(0.1) - 0.905).abs() < 1e-4);
        assert!((breathe_at(0.25) - 0.5).abs() < 1e-4);
        assert!(breathe_at(0.5).abs() < 1e-4);
        assert!((breathe_at(0.75) - 0.5).abs() < 1e-4);
        assert!((breathe_at(1.0) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn run_is_ease_in_out_over_1250_ms() {
        assert_eq!(ease_in_out(0.0), 0.0);
        assert!((ease_in_out(0.25) - 0.1292).abs() < 1e-3);
        assert!((ease_in_out(0.5) - 0.5).abs() < 1e-4);
        assert!((ease_in_out(0.75) - 0.8708).abs() < 1e-3);
        assert_eq!(ease_in_out(1.0), 1.0);
        let mut last = -1.0_f32;
        for i in 0..=200 {
            let v = ease_in_out(i as f32 / 200.0);
            assert!(v >= last, "not monotonic at {i}");
            last = v;
        }
        assert!((mark_phase(625).run_t - 0.5).abs() < 1e-3);
    }

    #[test]
    fn the_rainbow_line_flows_one_tab_in_eight_seconds() {
        assert_eq!(mark_phase(0).flow_px, 0.0);
        assert!((mark_phase(4000).flow_px - 57.0).abs() < 1e-3);
        assert!((mark_phase(7999).flow_px - 114.0).abs() < 0.1);
    }

    #[test]
    fn the_skeleton_pulses_from_one_to_half_and_back() {
        assert_eq!(pulse_at(0), 1.0);
        assert!((pulse_at(1000) - 0.5).abs() < 1e-3);
        assert_eq!(pulse_at(2000), 1.0);
        let mid = pulse_at(500);
        assert!(mid > 0.5 && mid < 1.0);
    }

    #[test]
    fn the_run_bar_travels_from_one_bar_left_of_the_line_to_just_past_its_end() {
        assert!((run_bar_x(0.0, 150.0) - -63.0).abs() < 1e-3);
        assert!((run_bar_x(1.0, 150.0) - 150.0).abs() < 1e-3);
        assert!((run_bar_x(0.5, 150.0) - 43.5).abs() < 1e-3);
    }

    #[test]
    fn the_clock_is_needed_only_while_something_moves() {
        use MarkState::*;
        assert!(!needs_clock(&[], Indicator::Ring, true, false));
        assert!(needs_clock(&[], Indicator::Ring, true, true));
        assert!(!needs_clock(&[Done], Indicator::Ring, true, false));
        assert!(needs_clock(&[Done], Indicator::Bar, true, false));
        assert!(!needs_clock(&[Done], Indicator::Bar, false, false));
        assert!(needs_clock(&[Question], Indicator::Ring, false, false));
        assert!(!needs_clock(&[Failed, Done], Indicator::Ring, true, false));
        for ind in [Indicator::Ring, Indicator::Bar] {
            assert!(needs_clock(&[Working], ind, false, false));
        }
    }

    #[test]
    fn minimise_pauses_and_restore_goes_on_from_where_it_was() {
        let mut p = MarksPause::new();
        p.set_minimized(true, 1000);
        assert!(p.paused());
        assert_eq!(p.clock(5000), 1000);
        p.set_minimized(false, 5000);
        assert!(!p.paused());
        assert_eq!(p.clock(5100), 1100);
    }
}
