//! The one wait primitive, `until`, and its clock seam (Task 13).
//!
//! Every wait in a scenario goes through `Ctx::until`, which polls this function. A test passes a
//! fake clock, so the timeout logic is checked without real sleeps.

use std::thread;
use std::time::{Duration, Instant};

/// The poll interval of `until`.
pub const POLL_MS: u64 = 25;

/// The clock `wait_on` reads and sleeps on. Tests pass a fake one.
pub trait Clock {
    /// Milliseconds since the wait began.
    fn elapsed_ms(&self) -> u64;
    /// Sleep for `ms` (or, for a fake clock, advance the reported time).
    fn sleep_ms(&mut self, ms: u64);
}

/// The real clock: wall time from its creation, and a real sleep.
pub struct SystemClock {
    start: Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn elapsed_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    fn sleep_ms(&mut self, ms: u64) {
        thread::sleep(Duration::from_millis(ms));
    }
}

/// Polls `cond` every 25 ms until it holds (true) or `timeout_ms` has passed (false). It records
/// nothing: `Ctx::until` turns a false into the one FAIL.
pub fn wait_on(clock: &mut dyn Clock, timeout_ms: u64, cond: &mut dyn FnMut() -> bool) -> bool {
    loop {
        if cond() {
            return true;
        }
        let elapsed = clock.elapsed_ms();
        if elapsed >= timeout_ms {
            return false;
        }
        clock.sleep_ms(POLL_MS.min(timeout_ms - elapsed));
    }
}
