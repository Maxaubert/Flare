//! The caret's tick (rule 1233): a hard on and off, one period each, while the terminal in front holds the keys
//! of a focused window. Typing or output keeps it lit, and the tick starts again one period after the last of
//! them; the window getting the focus lights it at once. Pure: every call takes the time, so the tests need no
//! clock and no sleep.

use std::time::{Duration, Instant};

/// The tick's half period when the system names none: Windows' default caret blink time.
pub const DEFAULT_PERIOD: Duration = Duration::from_millis(530);

/// The system's caret blink time, as Windows' `GetCaretBlinkTime` gives it. `None` means "do not blink" (the
/// person turned blinking off in Windows); anything odd falls back to [`DEFAULT_PERIOD`]. macOS has no such
/// setting, so it ticks at the default.
pub fn system_period() -> Option<Duration> {
    period_from(flare_win::system_caret::blink_time_ms())
}

/// Pure: a raw blink time to the tick's period. `u32::MAX` (Windows' INFINITE) is no blinking, 0 (the call
/// failed or there is no system value) and values outside 100 ms to 5 s are the default.
pub fn period_from(raw_ms: Option<u32>) -> Option<Duration> {
    match raw_ms {
        Some(u32::MAX) => None,
        Some(ms) if (100..=5000).contains(&ms) => Some(Duration::from_millis(u64::from(ms))),
        _ => Some(DEFAULT_PERIOD),
    }
}

/// The tick's state. The caret is lit in the first period after `anchor`, dark in the second, and so on.
#[derive(Clone, Debug)]
pub struct CaretBlink {
    /// The half period; `None` never blinks (the system says so).
    period: Option<Duration>,
    /// The Blink setting.
    enabled: bool,
    /// The window has the keyboard focus.
    focused: bool,
    /// The last thing that lit the caret: typing, output, the focus coming back, a setting changed.
    anchor: Option<Instant>,
}

impl CaretBlink {
    /// A tick of this half period, on, in a window that does not have the focus yet.
    pub fn new(period: Option<Duration>) -> Self {
        Self {
            period: period.filter(|p| !p.is_zero()),
            enabled: true,
            focused: false,
            anchor: None,
        }
    }

    /// The Blink setting changed. Turning it on starts lit.
    pub fn set_enabled(&mut self, on: bool, now: Instant) {
        if on != self.enabled {
            self.enabled = on;
            self.activity(now);
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// The window gained or lost the focus. Gaining it lights the caret at once. Returns whether it changed.
    pub fn set_focused(&mut self, on: bool, now: Instant) -> bool {
        if on == self.focused {
            return false;
        }
        self.focused = on;
        self.activity(now);
        true
    }

    /// The person typed or the program printed at `at`: the caret stays lit for a full period from then.
    /// An older time than the last one changes nothing.
    pub fn activity(&mut self, at: Instant) {
        if self.anchor.is_none_or(|a| at > a) {
            self.anchor = Some(at);
        }
    }

    /// Whether the caret ticks at all: the setting is on, the system blinks and the window has the focus. A
    /// window without the focus shows the caret steady.
    pub fn ticking(&self) -> bool {
        self.enabled && self.focused && self.period.is_some()
    }

    /// Whether the caret is lit at `now`. Always true while not ticking.
    pub fn lit(&self, now: Instant) -> bool {
        let (true, Some(period)) = (self.ticking(), self.period) else {
            return true;
        };
        let since = self
            .anchor
            .map_or(Duration::ZERO, |a| now.saturating_duration_since(a));
        (since.as_nanos() / period.as_nanos()).is_multiple_of(2)
    }

    /// How long from `now` until the caret next turns on or off, or `None` while it does not tick (no timer).
    pub fn next_change_in(&self, now: Instant) -> Option<Duration> {
        let period = self.period.filter(|_| self.ticking())?;
        let since = self
            .anchor
            .map_or(Duration::ZERO, |a| now.saturating_duration_since(a));
        let into = since.as_nanos() % period.as_nanos();
        Some(period - Duration::from_nanos(into as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: Duration = Duration::from_millis(530);

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// A focused, enabled tick anchored at `t0`.
    fn focused_at(t0: Instant) -> CaretBlink {
        let mut b = CaretBlink::new(Some(P));
        b.set_focused(true, t0);
        b
    }

    #[test]
    fn it_ticks_530_on_530_off_from_the_focus() {
        let t0 = Instant::now();
        let b = focused_at(t0);
        assert!(b.ticking());
        assert!(b.lit(t0), "lit the moment the window gets the focus");
        assert!(b.lit(t0 + ms(529)));
        assert!(!b.lit(t0 + ms(530)), "off after one period");
        assert!(!b.lit(t0 + ms(1059)));
        assert!(b.lit(t0 + ms(1060)), "on again after two");
        assert!(!b.lit(t0 + ms(1590)));
    }

    #[test]
    fn the_next_change_is_the_next_period_edge() {
        let t0 = Instant::now();
        let b = focused_at(t0);
        assert_eq!(b.next_change_in(t0), Some(P));
        assert_eq!(b.next_change_in(t0 + ms(100)), Some(ms(430)));
        assert_eq!(b.next_change_in(t0 + ms(530)), Some(P));
        assert_eq!(b.next_change_in(t0 + ms(700)), Some(ms(360)));
    }

    #[test]
    fn typing_or_output_keeps_it_lit_and_the_tick_resumes_a_period_later() {
        let t0 = Instant::now();
        let mut b = focused_at(t0);
        // Dark at 600 ms, then a key at 700: lit at once and for a full period.
        assert!(!b.lit(t0 + ms(600)));
        b.activity(t0 + ms(700));
        assert!(b.lit(t0 + ms(700)));
        assert!(b.lit(t0 + ms(1229)));
        assert!(!b.lit(t0 + ms(1230)), "ticks again 530 ms after the key");
        // A stream of output every 100 ms holds it lit.
        for k in 0..20 {
            let at = t0 + ms(2000 + k * 100);
            b.activity(at);
            assert!(b.lit(at + ms(99)));
        }
        // An older time (a key stamped before the last output) moves nothing.
        b.activity(t0 + ms(10));
        assert!(!b.lit(t0 + ms(2000 + 19 * 100 + 530)));
    }

    #[test]
    fn an_unfocused_window_shows_it_steady_and_runs_no_timer() {
        let t0 = Instant::now();
        let mut b = CaretBlink::new(Some(P));
        assert!(!b.ticking(), "no focus yet");
        assert!(b.lit(t0 + ms(800)));
        assert_eq!(b.next_change_in(t0 + ms(800)), None);
        assert!(b.set_focused(true, t0));
        assert!(
            !b.set_focused(true, t0 + ms(5)),
            "the same state is no change"
        );
        assert!(!b.lit(t0 + ms(800)));
        assert!(b.set_focused(false, t0 + ms(900)));
        assert!(b.lit(t0 + ms(1500)), "steady while away");
        assert_eq!(b.next_change_in(t0 + ms(1500)), None);
        // Back: lit at once, whatever the old phase was.
        b.set_focused(true, t0 + ms(1600));
        assert!(b.lit(t0 + ms(1600)));
        assert_eq!(b.next_change_in(t0 + ms(1600)), Some(P));
    }

    #[test]
    fn the_blink_setting_off_is_steady_and_on_again_starts_lit() {
        let t0 = Instant::now();
        let mut b = focused_at(t0);
        b.set_enabled(false, t0 + ms(10));
        assert!(!b.ticking());
        assert!(b.lit(t0 + ms(600)));
        assert_eq!(b.next_change_in(t0 + ms(600)), None);
        b.set_enabled(true, t0 + ms(600));
        assert!(b.lit(t0 + ms(600)));
        assert!(!b.lit(t0 + ms(1130)));
    }

    #[test]
    fn the_systems_blink_time_sets_the_period_and_infinite_is_steady() {
        assert_eq!(period_from(Some(530)), Some(P));
        assert_eq!(period_from(Some(1200)), Some(ms(1200)));
        assert_eq!(
            period_from(Some(0)),
            Some(DEFAULT_PERIOD),
            "the call failed"
        );
        assert_eq!(period_from(None), Some(DEFAULT_PERIOD), "no system value");
        assert_eq!(period_from(Some(9)), Some(DEFAULT_PERIOD), "nonsense");
        assert_eq!(
            period_from(Some(u32::MAX)),
            None,
            "Windows says do not blink"
        );
        let t0 = Instant::now();
        let mut b = CaretBlink::new(None);
        b.set_focused(true, t0);
        assert!(!b.ticking());
        assert!(b.lit(t0 + ms(600)));
        assert_eq!(b.next_change_in(t0), None);
    }
}
