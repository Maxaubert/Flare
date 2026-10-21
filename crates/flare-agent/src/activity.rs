//! Activity: typing, echo, birth and prompt of a tab (Task 7).
//!
//! Port of Prism Terminal's `termActivity.ts`. PT keeps these as module-level maps; here they are one
//! owned struct, and every method takes the time in milliseconds, so the rules replay without a clock.
//! Output a tab causes itself (a resize repaint, the echo of a keystroke, the startup paint of an
//! agent) is kept out of the run-scoring by these rules; a person's input and the agent's answer are not.

use std::collections::{HashMap, HashSet};

/// How long a resize repaint is ignored for run-scoring (PT `suppressActivity`, 800 ms).
pub const SUPPRESS_MS: u64 = 800;
/// Output this close behind a keystroke is its echo, not work (PT `inputEcho`, 350 ms).
pub const ECHO_MS: u64 = 350;
/// The silence that ends an agent's startup paint (PT `QUIET_GAP`, 1.5 s).
pub const QUIET_GAP_MS: u64 = 1500;

/// Per-session activity state: suppression, typing, prompts and the agent's birth.
#[derive(Default)]
pub struct Activity {
    until: HashMap<String, u64>,
    touched: HashSet<String>,
    last_input: HashMap<String, u64>,
    born: HashSet<String>,
    last_out: HashMap<String, u64>,
    last_prompt: HashMap<String, u64>,
}

impl Activity {
    /// Empty state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Ignore this session's output for run-scoring for `ms` (a refit: output we caused is not work).
    pub fn suppress(&mut self, id: &str, now: u64, ms: u64) {
        self.until.insert(id.to_string(), now + ms);
    }

    /// True while the suppression set by `suppress` has not run out.
    pub fn suppressed(&self, id: &str, now: u64) -> bool {
        now < self.until.get(id).copied().unwrap_or(0)
    }

    /// A person typed (key, paste, dropped path): the session is touched, its last input is `now`, and
    /// its birth ends (typing into the agent is the other end of its startup).
    pub fn mark_touched(&mut self, id: &str, now: u64) {
        self.touched.insert(id.to_string());
        self.last_input.insert(id.to_string(), now);
        self.born.remove(id);
    }

    /// Is this output within `ms` of the session's last keystroke? A session never typed into has no window.
    pub fn input_echo(&self, id: &str, now: u64, ms: u64) -> bool {
        match self.last_input.get(id) {
            Some(last) => now.saturating_sub(*last) < ms,
            None => false,
        }
    }

    /// Has a person ever typed into this session?
    pub fn is_touched(&self, id: &str) -> bool {
        self.touched.contains(id)
    }

    /// A shell prompt was seen (the shell reports it), at `now`.
    pub fn mark_prompt(&mut self, id: &str, now: u64) {
        self.last_prompt.insert(id.to_string(), now);
    }

    /// A prompt was seen, and nothing has been typed since it was. Kept for the cd guard (decision O1), which
    /// no caller uses yet.
    pub fn idle_at_prompt(&self, id: &str) -> bool {
        match self.last_prompt.get(id) {
            Some(p) => *p > self.last_input.get(id).copied().unwrap_or(0),
            None => false,
        }
    }

    /// An agent was just detected in this session: its startup paint starts, and `now` is its first output.
    pub fn mark_born(&mut self, id: &str, now: u64) {
        self.born.insert(id.to_string());
        self.last_out.insert(id.to_string(), now);
    }

    /// Called on EVERY chunk of output. True while the chunk is still the startup paint: born, and no quiet
    /// gap seen since. A session never born here, or forgotten, answers false and gets no entry.
    pub fn startup_output(&mut self, id: &str, now: u64) -> bool {
        if !self.born.contains(id) && !self.last_out.contains_key(id) {
            return false;
        }
        let prev = self.last_out.get(id).copied().unwrap_or(0);
        self.last_out.insert(id.to_string(), now);
        if !self.born.contains(id) {
            return false;
        }
        if now.saturating_sub(prev) > QUIET_GAP_MS {
            self.born.remove(id);
            return false;
        }
        true
    }

    /// The session ended: forget everything about it.
    pub fn forget(&mut self, id: &str) {
        self.touched.remove(id);
        self.last_input.remove(id);
        self.until.remove(id);
        self.last_prompt.remove(id);
        self.born.remove(id);
        self.last_out.remove(id);
    }

    /// How many sessions the birth rule keeps a clock for (tests).
    pub fn output_clocks(&self) -> usize {
        self.last_out.len()
    }
}

/// Is this `onData`-style payload typed by a person? Terminal replies all start with ESC (focus reports,
/// DA, CPR, OSC colour answers). What is left is plain text: an IME commit.
pub fn looks_typed(data: &str) -> bool {
    !data.is_empty() && !data.starts_with('\u{1b}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_output_for_a_closed_tab_leaves_nothing() {
        let mut a = Activity::new();
        let before = a.output_clocks();
        a.mark_born("late", 0);
        assert!(a.startup_output("late", 0));
        a.forget("late");
        assert_eq!(a.output_clocks(), before);
        assert!(!a.startup_output("late", 5));
        assert!(!a.startup_output("never-born", 5));
        assert_eq!(a.output_clocks(), before);
    }

    #[test]
    fn a_fresh_keystroke_opens_the_echo_window() {
        let mut a = Activity::new();
        a.mark_touched("s1", 1_000_000);
        assert!(a.input_echo("s1", 1_000_000, ECHO_MS));
        assert!(!a.input_echo("s1", 1_000_400, ECHO_MS));
    }

    #[test]
    fn the_window_closes_once_typing_stops() {
        let mut a = Activity::new();
        a.mark_touched("s1", 1_000_000);
        assert!(a.input_echo("s1", 1_000_200, ECHO_MS));
        assert!(!a.input_echo("s1", 1_000_351, ECHO_MS));
    }

    #[test]
    fn continuous_typing_keeps_renewing_it() {
        let mut a = Activity::new();
        let mut t = 1_000_000;
        for _ in 0..10 {
            a.mark_touched("s1", t);
            t += 200;
            assert!(a.input_echo("s1", t, ECHO_MS));
        }
    }

    #[test]
    fn a_session_never_typed_into_has_no_window() {
        let a = Activity::new();
        assert!(!a.input_echo("never", 1_000_000, ECHO_MS));
    }

    #[test]
    fn forget_clears_the_echo_and_touched() {
        let mut a = Activity::new();
        a.mark_touched("s1", 1_000_000);
        a.forget("s1");
        assert!(!a.is_touched("s1"));
        assert!(!a.input_echo("s1", 1_000_000, ECHO_MS));
    }

    #[test]
    fn suppression_expires_on_schedule() {
        let mut a = Activity::new();
        a.suppress("s3", 0, SUPPRESS_MS);
        assert!(a.suppressed("s3", 0));
        assert!(!a.suppressed("s3", 900));
    }

    #[test]
    fn plain_text_is_typing_and_every_esc_led_reply_is_not() {
        assert!(looks_typed("a"));
        assert!(looks_typed("日本"));
        assert!(!looks_typed(""));
        assert!(!looks_typed("\x1b[I"));
        assert!(!looks_typed("\x1b[O"));
        assert!(!looks_typed("\x1b[?1;2c"));
        assert!(!looks_typed("\x1b[12;40R"));
        assert!(!looks_typed("\x1b[200~pasted\x1b[201~"));
    }

    #[test]
    fn idle_at_prompt_is_true_only_after_a_prompt_after_the_last_keystroke() {
        let mut a = Activity::new();
        assert!(!a.idle_at_prompt("s"));
        a.mark_prompt("s", 1_000_000);
        assert!(a.idle_at_prompt("s"));
        a.mark_touched("s", 1_000_010);
        assert!(!a.idle_at_prompt("s"));
        a.mark_prompt("s", 1_000_020);
        assert!(a.idle_at_prompt("s"));
        a.forget("s");
        assert!(!a.idle_at_prompt("s"));
    }

    #[test]
    fn the_startup_paint_ends_at_the_first_silence() {
        let mut a = Activity::new();
        a.mark_born("a", 1000);
        let mut t = 1100;
        while t <= 9000 {
            assert!(a.startup_output("a", t), "t = {t}");
            t += 300;
        }
        assert!(!a.startup_output("a", 12_000));
        assert!(!a.startup_output("a", 12_100));
    }

    #[test]
    fn a_session_with_no_agent_is_never_suppressed() {
        let mut a = Activity::new();
        assert!(!a.startup_output("plain", 1000));
        assert!(!a.startup_output("plain", 1100));
    }

    #[test]
    fn late_detection_costs_nothing() {
        let mut a = Activity::new();
        a.mark_born("b", 5000);
        assert!(!a.startup_output("b", 9000));
    }

    #[test]
    fn forgetting_clears_the_birth() {
        let mut a = Activity::new();
        a.mark_born("c", 1000);
        a.forget("c");
        assert!(!a.startup_output("c", 1100));
    }

    #[test]
    fn typing_ends_the_startup_too() {
        let mut a = Activity::new();
        a.mark_born("d", 1000);
        assert!(a.startup_output("d", 1200));
        a.mark_touched("d", 1300);
        assert!(!a.startup_output("d", 1400));
    }
}
