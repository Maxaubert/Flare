//! Install when idle (2026-10-29): the idle rule.
//!
//! "Idle" means no agent in any tab is working, none shows a question, no dictation runs and no close or install
//! question is open, and that has held for [`IDLE_HOLD_MS`] without a break. Any busy moment starts the wait again.

/// How long everything must stay idle before a staged build installs on its own.
pub const IDLE_HOLD_MS: u64 = 60_000;

/// What keeps the app busy, read from the agent hub, dictation and the window's questions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Busy {
    /// Tabs whose agent is working (`AgentHub::working_count`).
    pub working: usize,
    /// Tabs whose agent shows a question.
    pub questions: usize,
    /// A dictation is listening or transcribing.
    pub dictating: bool,
    /// The close question (or the install question) is open.
    pub asking: bool,
}

impl Busy {
    pub fn any(&self) -> bool {
        self.working > 0 || self.questions > 0 || self.dictating || self.asking
    }
}

/// The wait for a quiet minute. `observe` is told every reading; it answers whether the quiet has held long enough.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IdleClock {
    since: Option<u64>,
}

impl IdleClock {
    /// One reading at `now` (ms). Busy clears the wait; quiet starts it (if not started) and answers true once it
    /// has lasted `hold_ms`.
    pub fn observe(&mut self, busy: bool, now: u64, hold_ms: u64) -> bool {
        if busy {
            self.since = None;
            return false;
        }
        let since = *self.since.get_or_insert(now);
        now.saturating_sub(since) >= hold_ms
    }

    /// Starts the wait again (after a cancel or a new build).
    pub fn reset(&mut self) {
        self.since = None;
    }

    /// When the quiet started, if it is quiet now.
    pub fn since(&self) -> Option<u64> {
        self.since
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anything_working_asking_or_dictating_is_busy() {
        assert!(!Busy::default().any());
        assert!(
            Busy {
                working: 1,
                ..Busy::default()
            }
            .any()
        );
        assert!(
            Busy {
                questions: 1,
                ..Busy::default()
            }
            .any()
        );
        assert!(
            Busy {
                dictating: true,
                ..Busy::default()
            }
            .any()
        );
        assert!(
            Busy {
                asking: true,
                ..Busy::default()
            }
            .any()
        );
    }

    #[test]
    fn a_quiet_minute_without_a_break_is_idle() {
        let mut c = IdleClock::default();
        assert!(!c.observe(false, 1_000, IDLE_HOLD_MS));
        assert!(!c.observe(false, 60_999, IDLE_HOLD_MS));
        assert!(c.observe(false, 61_000, IDLE_HOLD_MS));
    }

    #[test]
    fn any_busy_moment_starts_the_minute_again() {
        let mut c = IdleClock::default();
        c.observe(false, 0, IDLE_HOLD_MS);
        assert!(!c.observe(true, 59_000, IDLE_HOLD_MS));
        assert_eq!(c.since(), None);
        assert!(!c.observe(false, 60_000, IDLE_HOLD_MS));
        assert!(!c.observe(false, 119_999, IDLE_HOLD_MS));
        assert!(c.observe(false, 120_000, IDLE_HOLD_MS));
    }

    #[test]
    fn reset_starts_the_wait_again() {
        let mut c = IdleClock::default();
        c.observe(false, 0, 10);
        c.reset();
        assert!(!c.observe(false, 100, 10));
        assert!(c.observe(false, 110, 10));
    }
}
