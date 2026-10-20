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
}
