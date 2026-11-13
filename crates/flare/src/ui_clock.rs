//! The UI's clock: milliseconds on the monotonic `Instant` clock, counted from the first reading.
//! Every UI timer (the badges, the tips, the marks, the help index) reads this. Wall-clock time
//! (`SystemTime`) steps back when the clock is set or synced, and a badge that read an earlier
//! time than its copy was held on screen, while a timer could drop its due time altogether.

use std::sync::OnceLock;
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();

/// Milliseconds since the first call. It never goes back, whatever the wall clock does.
pub fn monotonic_ms() -> u64 {
    let start = START.get_or_init(Instant::now);
    start.elapsed().as_millis() as u64
}

/// `at` on this clock: milliseconds from the first reading to `at` (0 for an instant before it). An
/// `Instant` taken on another thread (a tab's cursor stop) is compared with key times this way (rule 1206).
pub fn ms_at(at: Instant) -> u64 {
    let start = START.get_or_init(Instant::now);
    at.saturating_duration_since(*start).as_millis() as u64
}

/// The same clock to the fraction of a millisecond (the sidebar's slide reads its curve on it).
pub fn monotonic_ms_f64() -> f64 {
    let start = START.get_or_init(Instant::now);
    start.elapsed().as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reading_never_goes_back() {
        let mut last = monotonic_ms();
        for _ in 0..1000 {
            let now = monotonic_ms();
            assert!(now >= last, "{now} went back from {last}");
            last = now;
        }
    }

    #[test]
    fn an_instant_reads_on_the_same_clock() {
        let before = monotonic_ms();
        let at = ms_at(Instant::now());
        let after = monotonic_ms();
        assert!(before <= at && at <= after, "{before} {at} {after}");
        let early = START
            .get()
            .and_then(|s| s.checked_sub(std::time::Duration::from_millis(1)));
        if let Some(early) = early {
            assert_eq!(ms_at(early), 0);
        }
    }
}
