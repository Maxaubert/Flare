//! human_for: the duration words (Task 12; M6 adds the rest).

/// A duration in words for a sentence: rounded down, coarse on purpose, and "a few seconds" under ten.
pub fn human_for(ms: u64) -> String {
    let s = ms / 1000;
    if s < 10 {
        return "a few seconds".to_string();
    }
    if s < 60 {
        return plural(s, "second");
    }
    let m = s / 60;
    if m < 60 {
        return plural(m, "minute");
    }
    let h = m / 60;
    let rest = m % 60;
    if rest == 0 {
        plural(h, "hour")
    } else {
        format!("{} {}", plural(h, "hour"), plural(rest, "minute"))
    }
}

fn plural(n: u64, unit: &str) -> String {
    let s = if n == 1 { "" } else { "s" };
    format!("{n} {unit}{s}")
}

/// How long each session has been working (PT `agentClock.ts`). Not PT's output run (that restarts on every
/// pause): this is when the session ENTERED the working set and stayed there.
#[derive(Default)]
pub struct AgentClock {
    started: std::collections::HashMap<String, u64>,
}

impl AgentClock {
    /// Empty clock.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reconcile against the current working set: a session newly in it gets `now`; one that left is forgotten.
    pub fn note_working(&mut self, ids: &std::collections::HashSet<String>, now: u64) {
        for id in ids {
            self.started.entry(id.clone()).or_insert(now);
        }
        self.started.retain(|id, _| ids.contains(id));
    }

    /// How long this session has been working (`now - start`, never negative), or None if it is not.
    pub fn working_for(&self, id: &str, now: u64) -> Option<u64> {
        self.started.get(id).map(|at| now.saturating_sub(*at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_for_a_few_seconds_under_ten() {
        assert_eq!(human_for(0), "a few seconds");
        assert_eq!(human_for(9_400), "a few seconds");
    }

    #[test]
    fn human_for_counts_seconds_minutes_hours() {
        assert_eq!(human_for(10_000), "10 seconds");
        assert_eq!(human_for(59_900), "59 seconds");
        assert_eq!(human_for(60_000), "1 minute");
        assert_eq!(human_for(125_000), "2 minutes");
        assert_eq!(human_for(3_600_000), "1 hour");
        assert_eq!(human_for(3_900_000), "1 hour 5 minutes");
        assert_eq!(human_for(7_200_000), "2 hours");
    }

    #[test]
    fn human_for_rounds_down() {
        assert_eq!(human_for(119_999), "1 minute");
    }

    fn set(ids: &[&str]) -> std::collections::HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn starts_the_clock_the_first_time_a_session_is_seen_working() {
        let mut c = AgentClock::new();
        c.note_working(&set(&["a"]), 1000);
        assert_eq!(c.working_for("a", 4000), Some(3000));
    }

    #[test]
    fn does_not_restart_it_while_the_session_keeps_working() {
        let mut c = AgentClock::new();
        c.note_working(&set(&["a"]), 1000);
        c.note_working(&set(&["a"]), 9000);
        c.note_working(&set(&["a"]), 20_000);
        assert_eq!(c.working_for("a", 20_000), Some(19_000));
    }

    #[test]
    fn forgets_a_session_that_stops() {
        let mut c = AgentClock::new();
        c.note_working(&set(&["a"]), 1000);
        c.note_working(&set(&[]), 2000);
        assert_eq!(c.working_for("a", 3000), None);
    }

    #[test]
    fn restarts_the_clock_when_it_starts_again() {
        let mut c = AgentClock::new();
        c.note_working(&set(&["a"]), 1000);
        c.note_working(&set(&[]), 2000);
        c.note_working(&set(&["a"]), 5000);
        assert_eq!(c.working_for("a", 6000), Some(1000));
    }

    #[test]
    fn keeps_sessions_apart() {
        let mut c = AgentClock::new();
        c.note_working(&set(&["a"]), 1000);
        c.note_working(&set(&["a", "b"]), 4000);
        assert_eq!(c.working_for("a", 5000), Some(4000));
        assert_eq!(c.working_for("b", 5000), Some(1000));
    }

    #[test]
    fn answers_none_for_a_session_never_working() {
        let c = AgentClock::new();
        assert_eq!(c.working_for("nobody", 0), None);
    }

    #[test]
    fn never_answers_negative() {
        let mut c = AgentClock::new();
        c.note_working(&set(&["a"]), 5000);
        assert_eq!(c.working_for("a", 1000), Some(0));
    }
}
