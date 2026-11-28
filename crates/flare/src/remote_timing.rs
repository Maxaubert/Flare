//! Rule 1392: what a remote tab's link worker measures while the person types, and the budget its fast polls spend.
//!
//! Every call over the link is timed by its method, and a key's echo by the time from when it was typed to the first
//! output that came after its `remote.input`. A key is sampled only when the poll before it was empty, so output that
//! was flowing anyway is not read as an echo; a key with no output within [`ECHO_WAIT`] counts as no echo. A summary
//! goes to the diagnostics log as one `remote-typing` line: every [`REPORT_EVERY`] while there was typing, or sooner
//! after [`REPORT_KEYS`] keys. It holds numbers only, never what was typed (rule 110). A window with no key in it is
//! dropped without a line.
//!
//! The budget: the host takes at most 100 calls a second on one link (`ServerConfig::stdio`), and a refusal of a poll
//! drops the link. Every call spends from [`CallBudget`] ([`BUDGET_RATE`] a second); a fast poll is made only while
//! the budget has room, so fast polls add nothing once the link makes [`BUDGET_RATE`] calls a second, and the other
//! calls go as they did before rule 1392.

use flare_ctl::Method;
use serde_json::{Map, Value, json};
use std::time::{Duration, Instant};

/// How long a key waits for its echo before it counts as a key with no echo.
pub const ECHO_WAIT: Duration = Duration::from_secs(2);
/// The longest a summary waits once typing started.
pub const REPORT_EVERY: Duration = Duration::from_secs(10);
/// A summary goes sooner once this many keys were sampled.
pub const REPORT_KEYS: usize = 200;
/// The calls a link may make a second before its fast polls stop: well under the host's 100.
pub const BUDGET_RATE: f64 = 50.0;
/// The calls a link may make at once before [`BUDGET_RATE`] applies.
pub const BUDGET_BURST: f64 = 20.0;
/// The most samples kept of one kind between two summaries.
const SAMPLES_MAX: usize = 4096;

/// A token bucket that every call spends from and that only the fast polls ask for room in. A spend never refuses
/// (the calls the link made before rule 1392 still go), it can only leave no room for a fast poll.
#[derive(Clone, Debug)]
pub struct CallBudget {
    tokens: f64,
    last: Instant,
    rate: f64,
    burst: f64,
}

impl CallBudget {
    pub fn new(rate: f64, burst: f64, now: Instant) -> Self {
        Self {
            tokens: burst,
            last: now,
            rate,
            burst,
        }
    }

    fn refill(&mut self, now: Instant) {
        let dt = now.saturating_duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = (self.tokens + dt * self.rate).min(self.burst);
    }

    /// One call made.
    pub fn spend(&mut self, now: Instant) {
        self.refill(now);
        self.tokens = (self.tokens - 1.0).max(-self.burst);
    }

    /// Whether one more call fits now.
    pub fn room(&mut self, now: Instant) -> bool {
        self.refill(now);
        self.tokens >= 1.0
    }
}

/// Pure: the `p`th percentile (0 to 100) of sorted samples in microseconds, in milliseconds to one decimal.
pub fn percentile_ms(sorted: &[u32], p: usize) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let i = ((sorted.len() - 1) * p.min(100) + 50) / 100;
    ms(sorted[i])
}

fn ms(us: u32) -> f64 {
    (f64::from(us) / 100.0).round() / 10.0
}

fn micros(d: Duration) -> u32 {
    u32::try_from(d.as_micros()).unwrap_or(u32::MAX)
}

fn push(v: &mut Vec<u32>, d: Duration) {
    if v.len() < SAMPLES_MAX {
        v.push(micros(d));
    }
}

/// The short name of a method in a summary's fields: `remote.output` is `output`.
fn short(m: Method) -> &'static str {
    let n = m.name();
    n.strip_prefix("remote.").unwrap_or(n)
}

/// One link's timings and budget (rule 1392).
pub struct LinkWatch {
    calls: Vec<(Method, Vec<u32>)>,
    echoes: Vec<u32>,
    no_echo: u32,
    fast_polls: u32,
    /// The key waiting for its echo: when it was typed, and when its call was answered.
    pending: Option<(Instant, Instant)>,
    /// The last output poll brought nothing.
    quiet: bool,
    /// The first sample of this summary.
    since: Option<Instant>,
    budget: CallBudget,
}

impl LinkWatch {
    pub fn new(now: Instant) -> Self {
        Self {
            calls: Vec::new(),
            echoes: Vec::new(),
            no_echo: 0,
            fast_polls: 0,
            pending: None,
            quiet: false,
            since: None,
            budget: CallBudget::new(BUDGET_RATE, BUDGET_BURST, now),
        }
    }

    /// A new link: the host's rate counts again from nothing, and no key waits for an echo on it.
    pub fn new_link(&mut self, now: Instant) {
        self.budget = CallBudget::new(BUDGET_RATE, BUDGET_BURST, now);
        self.pending = None;
        self.quiet = false;
    }

    /// A call over the link took `took` and ended at `now`.
    pub fn call(&mut self, m: Method, took: Duration, now: Instant) {
        self.budget.spend(now);
        self.since.get_or_insert(now);
        match self.calls.iter_mut().find(|(k, _)| *k == m) {
            Some((_, v)) => push(v, took),
            None => {
                let mut v = Vec::new();
                push(&mut v, took);
                self.calls.push((m, v));
            }
        }
    }

    /// Whether a fast poll fits in the budget now.
    pub fn fast_room(&mut self, now: Instant) -> bool {
        self.budget.room(now)
    }

    /// A fast poll is about to be made (counted in the summary).
    pub fn fast_poll(&mut self) {
        self.fast_polls = self.fast_polls.saturating_add(1);
    }

    /// The host took typing that was typed at `typed`; its call was answered at `now`. Sampled when no key waits yet
    /// and the last poll was quiet.
    pub fn sent(&mut self, typed: Instant, now: Instant) {
        if self.pending.is_none() && self.quiet {
            self.pending = Some((typed, now));
        }
    }

    /// An output poll answered at `now`, with bytes or none. True when it was the echo of the key waiting.
    pub fn output(&mut self, got: bool, now: Instant) -> bool {
        self.quiet = !got;
        let Some((typed, answered)) = self.pending else {
            return false;
        };
        if got {
            self.pending = None;
            push(&mut self.echoes, now.saturating_duration_since(typed));
            return true;
        }
        if now.saturating_duration_since(answered) > ECHO_WAIT {
            self.pending = None;
            self.no_echo = self.no_echo.saturating_add(1);
        }
        false
    }

    fn keys(&self) -> usize {
        self.echoes.len() + self.no_echo as usize
    }

    /// The summary's fields when one is due at `now` (and the watch starts a new one), else None. A window with no
    /// key in it is dropped at its end without a line.
    pub fn report(&mut self, now: Instant) -> Option<Map<String, Value>> {
        let since = self.since?;
        let span = now.saturating_duration_since(since);
        if self.echoes.len() < REPORT_KEYS && span < REPORT_EVERY {
            return None;
        }
        let had_keys = self.keys() > 0;
        let fields = had_keys.then(|| self.fields(span));
        self.calls.clear();
        self.echoes.clear();
        self.no_echo = 0;
        self.fast_polls = 0;
        self.since = None;
        fields
    }

    fn fields(&mut self, span: Duration) -> Map<String, Value> {
        let mut m = Map::new();
        self.echoes.sort_unstable();
        m.insert("spanS".into(), json!(span.as_secs()));
        m.insert("keys".into(), json!(self.keys()));
        m.insert("noEcho".into(), json!(self.no_echo));
        m.insert("echoP50Ms".into(), json!(percentile_ms(&self.echoes, 50)));
        m.insert("echoP95Ms".into(), json!(percentile_ms(&self.echoes, 95)));
        m.insert(
            "echoMaxMs".into(),
            json!(self.echoes.last().copied().map(ms).unwrap_or(0.0)),
        );
        m.insert("fastPolls".into(), json!(self.fast_polls));
        for (method, v) in &mut self.calls {
            v.sort_unstable();
            let s = short(*method);
            m.insert(format!("{s}N"), json!(v.len()));
            m.insert(format!("{s}P50Ms"), json!(percentile_ms(v, 50)));
            m.insert(format!("{s}P95Ms"), json!(percentile_ms(v, 95)));
        }
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(t0: Instant, ms: u64) -> Instant {
        t0 + Duration::from_millis(ms)
    }

    #[test]
    fn percentiles_are_read_from_sorted_microseconds_in_ms() {
        assert_eq!(percentile_ms(&[], 50), 0.0);
        let v: Vec<u32> = (1..=100).map(|n| n * 1000).collect();
        assert_eq!(percentile_ms(&v, 50), 51.0);
        assert_eq!(percentile_ms(&v, 95), 95.0);
        assert_eq!(percentile_ms(&v, 100), 100.0);
        assert_eq!(percentile_ms(&[1234], 95), 1.2);
    }

    /// Every call spends; a fast poll fits only while the budget has room, and the room comes back at its rate.
    #[test]
    fn the_budget_leaves_no_room_for_fast_polls_once_spent_and_refills() {
        let t0 = Instant::now();
        let mut b = CallBudget::new(60.0, 30.0, t0);
        for _ in 0..30 {
            assert!(b.room(t0));
            b.spend(t0);
        }
        assert!(!b.room(t0), "the burst is spent");
        // Calls go on (a spend never refuses), and only take the room further away, down to minus the burst.
        for _ in 0..100 {
            b.spend(t0);
        }
        assert!(!b.room(at(t0, 500)), "30 owed, 30 back: still no room");
        assert!(b.room(at(t0, 1100)), "back at 60 a second");
    }

    /// A key is timed from when it was typed to the first output after it; one typed while output flowed is not
    /// sampled, and one with no output within the wait counts as no echo.
    #[test]
    fn a_key_is_timed_to_the_first_output_after_it_and_only_after_a_quiet_poll() {
        let t0 = Instant::now();
        let mut w = LinkWatch::new(t0);
        w.output(true, at(t0, 0));
        w.sent(at(t0, 1), at(t0, 5));
        assert!(
            !w.output(true, at(t0, 9)),
            "output was flowing: not sampled"
        );
        assert!(!w.output(false, at(t0, 20)));
        w.sent(at(t0, 21), at(t0, 30));
        w.sent(at(t0, 31), at(t0, 35));
        assert!(!w.output(false, at(t0, 40)));
        assert!(w.output(true, at(t0, 61)), "the echo");
        assert_eq!(
            w.echoes,
            vec![40_000],
            "timed from the first key, typed at 21 ms"
        );
        w.output(false, at(t0, 90));
        w.sent(at(t0, 100), at(t0, 110));
        assert!(!w.output(false, at(t0, 1000)));
        assert!(!w.output(false, at(t0, 2111)), "past the wait");
        assert_eq!(w.no_echo, 1);
        assert!(!w.output(true, at(t0, 2200)), "nothing waits any more");
    }

    /// The summary goes every 10 s of typing or after 200 keys, holds numbers only, and a window with no key in it is
    /// dropped with no line.
    #[test]
    fn the_summary_goes_every_ten_seconds_or_two_hundred_keys_and_never_without_a_key() {
        let t0 = Instant::now();
        let mut w = LinkWatch::new(t0);
        assert!(w.report(t0).is_none(), "nothing measured");
        w.call(Method::RemoteOutput, Duration::from_millis(4), t0);
        assert!(w.report(at(t0, 9_000)).is_none(), "not due");
        assert!(
            w.report(at(t0, 10_000)).is_none(),
            "due, but no key: dropped"
        );
        assert!(w.report(at(t0, 30_000)).is_none(), "and forgotten");

        let t1 = at(t0, 40_000);
        w.call(Method::RemoteInput, Duration::from_millis(12), t1);
        w.call(Method::RemoteOutput, Duration::from_millis(8), t1);
        w.output(false, t1);
        w.sent(t1, t1);
        w.fast_poll();
        w.output(true, at(t1, 30));
        let f = w.report(at(t1, 10_000)).expect("ten seconds of typing");
        assert_eq!(f["keys"], json!(1));
        assert_eq!(f["noEcho"], json!(0));
        assert_eq!(f["echoP50Ms"], json!(30.0));
        assert_eq!(f["echoMaxMs"], json!(30.0));
        assert_eq!(f["fastPolls"], json!(1));
        assert_eq!(f["inputN"], json!(1));
        assert_eq!(f["inputP50Ms"], json!(12.0));
        assert_eq!(f["outputP95Ms"], json!(8.0));
        assert_eq!(f["spanS"], json!(10));
        assert!(
            f.values().all(|v| v.is_number()),
            "numbers only, never what was typed: {f:?}"
        );
        assert!(w.report(at(t1, 30_000)).is_none(), "a new window");

        let t2 = at(t1, 60_000);
        for n in 0..REPORT_KEYS as u64 {
            w.call(
                Method::RemoteOutput,
                Duration::from_millis(1),
                at(t2, n * 10),
            );
            w.output(false, at(t2, n * 10));
            w.sent(at(t2, n * 10), at(t2, n * 10));
            w.output(true, at(t2, n * 10 + 5));
        }
        let f = w.report(at(t2, 2_000)).expect("two hundred keys");
        assert_eq!(f["keys"], json!(REPORT_KEYS));
        assert_eq!(f["echoP95Ms"], json!(5.0));
    }

    /// A new link starts the budget afresh and forgets the key that waited on the old one.
    #[test]
    fn a_new_link_starts_the_budget_again_and_drops_the_waiting_key() {
        let t0 = Instant::now();
        let mut w = LinkWatch::new(t0);
        for _ in 0..(BUDGET_BURST as usize + 5) {
            w.call(Method::RemoteOutput, Duration::ZERO, t0);
        }
        assert!(!w.fast_room(t0));
        w.output(false, t0);
        w.sent(t0, t0);
        w.new_link(t0);
        assert!(w.fast_room(t0));
        assert!(!w.output(true, at(t0, 5)), "the key was for the old link");
    }
}
