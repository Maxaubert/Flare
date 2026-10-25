//! The limit of a plugin (design 13, plugins phase 3): one per protocol 2 plugin that is on, shared by every page it
//! has (its background page, its panels and bars) and later its helper, so a plugin with many pages has no more room
//! than a plugin with one. It holds 50 messages a second (a burst of 100) and 2 MB a second, and no message may be
//! larger than 256 KB.
//!
//! A message is spent where it first arrives: a page's in the window, as it is handed over (the browser glue looks
//! at [`Limiter::has_room`] before it even copies a message out of the page, so a page that is over its budget costs
//! nothing), a pipe connection's in the dispatcher on the channel's worker. Protocol 1 mods have no limiter and
//! keep the limits they always had.
//!
//! Pure apart from the clock, which every call is given, so the tests never sleep.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Messages one plugin may send in a second, all its pages together.
pub const CALLS_PER_SEC: f64 = 50.0;
/// The most messages one plugin may send at once after a quiet spell.
pub const CALL_BURST: f64 = 100.0;
/// Bytes one plugin may send in a second, all its pages together (and the most at once).
pub const BYTES_PER_SEC: f64 = 2.0 * 1024.0 * 1024.0;
/// The largest message a plugin may send.
pub const MAX_MESSAGE: usize = 256 * 1024;

/// Why a message was not taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Over {
    /// The message is larger than [`MAX_MESSAGE`].
    TooLarge,
    /// The plugin sent more messages, or more bytes, than its budget allows right now.
    Budget,
}

impl Over {
    /// One sentence for a log or an error.
    pub fn why(self) -> &'static str {
        match self {
            Over::TooLarge => "the message is larger than 256 KB",
            Over::Budget => "the plugin sends more than 50 messages or 2 MB a second",
        }
    }
}

#[derive(Debug)]
struct Bucket {
    tokens: f64,
    last: Instant,
    rate: f64,
    burst: f64,
}

impl Bucket {
    fn new(rate: f64, burst: f64, now: Instant) -> Self {
        Self {
            tokens: burst,
            last: now,
            rate,
            burst,
        }
    }

    fn fill(&mut self, now: Instant) {
        let dt = now.saturating_duration_since(self.last).as_secs_f64();
        self.last = self.last.max(now);
        self.tokens = (self.tokens + dt * self.rate).min(self.burst);
    }
}

#[derive(Debug)]
struct Budget {
    calls: Bucket,
    bytes: Bucket,
    /// Messages refused since the plugin was turned on (diagnostics and tests).
    refused: u64,
}

/// One plugin's budget.
#[derive(Debug)]
pub struct Limiter {
    inner: Mutex<Budget>,
}

impl Limiter {
    pub fn new(now: Instant) -> Self {
        Self {
            inner: Mutex::new(Budget {
                calls: Bucket::new(CALLS_PER_SEC, CALL_BURST, now),
                bytes: Bucket::new(BYTES_PER_SEC, BYTES_PER_SEC, now),
                refused: 0,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Budget> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether one more message could be taken now, without spending anything: what the browser glue asks
    /// before it copies a message out of the page.
    pub fn has_room(&self, now: Instant) -> bool {
        let mut b = self.lock();
        b.calls.fill(now);
        b.bytes.fill(now);
        let room = b.calls.tokens >= 1.0 && b.bytes.tokens >= 1.0;
        if !room {
            b.refused += 1;
        }
        room
    }

    /// Spends one message of `bytes` bytes, or says why it is refused (nothing is spent then).
    pub fn admit(&self, bytes: usize, now: Instant) -> Result<(), Over> {
        let mut b = self.lock();
        if bytes > MAX_MESSAGE {
            b.refused += 1;
            return Err(Over::TooLarge);
        }
        b.calls.fill(now);
        b.bytes.fill(now);
        let size = bytes as f64;
        if b.calls.tokens < 1.0 || b.bytes.tokens < size {
            b.refused += 1;
            return Err(Over::Budget);
        }
        b.calls.tokens -= 1.0;
        b.bytes.tokens -= size;
        Ok(())
    }

    /// Messages refused so far.
    pub fn refused(&self) -> u64 {
        self.lock().refused
    }
}

/// The limiter of every plugin that is on, by id. The hub keeps it in step with the switches; the window and the
/// browser glue hold the same `Arc`.
#[derive(Debug, Default)]
pub struct Limits {
    by_id: Mutex<HashMap<String, Arc<Limiter>>>,
}

impl Limits {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Limiter>>> {
        self.by_id.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The plugin's limiter; None for a protocol 1 mod, or a plugin that is off.
    pub fn of(&self, id: &str) -> Option<Arc<Limiter>> {
        self.lock().get(id).cloned()
    }

    /// Makes the set of limited plugins `ids`: a new one starts with a full budget, one that stays keeps what it
    /// spent (turning a plugin off and on again starts it afresh).
    pub fn keep_only(&self, ids: &[String], now: Instant) {
        let mut g = self.lock();
        g.retain(|id, _| ids.contains(id));
        for id in ids {
            g.entry(id.clone())
                .or_insert_with(|| Arc::new(Limiter::new(now)));
        }
    }

    /// The ids limited now, sorted (tests).
    pub fn ids(&self) -> Vec<String> {
        let mut v: Vec<String> = self.lock().keys().cloned().collect();
        v.sort();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_burst_of_one_hundred_then_fifty_a_second() {
        let t0 = Instant::now();
        let l = Limiter::new(t0);
        for i in 0..100 {
            assert_eq!(l.admit(10, t0), Ok(()), "message {i}");
        }
        assert_eq!(l.admit(10, t0), Err(Over::Budget));
        assert!(!l.has_room(t0));
        // A second later there is room for fifty more, and no more.
        let t1 = t0 + Duration::from_secs(1);
        assert!(l.has_room(t1));
        for _ in 0..50 {
            assert_eq!(l.admit(10, t1), Ok(()));
        }
        assert_eq!(l.admit(10, t1), Err(Over::Budget));
        assert_eq!(l.refused(), 3);
    }

    #[test]
    fn two_megabytes_a_second_and_no_message_over_256_kb() {
        let t0 = Instant::now();
        let l = Limiter::new(t0);
        assert_eq!(l.admit(MAX_MESSAGE + 1, t0), Err(Over::TooLarge));
        // Eight of the largest are 2 MB: the ninth waits for the next half second.
        for _ in 0..8 {
            assert_eq!(l.admit(MAX_MESSAGE, t0), Ok(()));
        }
        assert_eq!(l.admit(MAX_MESSAGE, t0), Err(Over::Budget));
        assert!(!l.has_room(t0), "not even a byte is left");
        assert_eq!(
            l.admit(MAX_MESSAGE, t0 + Duration::from_millis(130)),
            Ok(())
        );
        assert!(Over::TooLarge.why().contains("256 KB"));
    }

    #[test]
    fn a_refused_message_spends_nothing_and_a_late_clock_is_no_gift() {
        let t0 = Instant::now();
        let l = Limiter::new(t0);
        for _ in 0..100 {
            l.admit(1, t0).unwrap();
        }
        assert!(l.admit(1, t0).is_err());
        // A clock read before the last one (two threads) adds nothing.
        assert!(l.admit(1, t0 - Duration::from_millis(1)).is_err());
        assert!(l.admit(1, t0 + Duration::from_millis(20)).is_ok());
    }

    #[test]
    fn the_set_follows_the_switches_and_a_plugin_back_on_starts_afresh() {
        let t0 = Instant::now();
        let limits = Limits::default();
        limits.keep_only(&["a".into(), "b".into()], t0);
        assert_eq!(limits.ids(), ["a", "b"]);
        let a = limits.of("a").unwrap();
        for _ in 0..100 {
            a.admit(1, t0).unwrap();
        }
        // Still on: the same budget.
        limits.keep_only(&["a".into()], t0);
        assert!(limits.of("a").unwrap().admit(1, t0).is_err());
        assert!(limits.of("b").is_none());
        // Off and on again: a full one.
        limits.keep_only(&[], t0);
        limits.keep_only(&["a".into()], t0);
        assert!(limits.of("a").unwrap().admit(1, t0).is_ok());
    }
}
