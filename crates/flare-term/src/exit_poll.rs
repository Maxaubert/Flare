//! ExitPoller: reads a killed shell's exit code fast at first, then slowly, and says once when a
//! kill is stuck.

use std::time::Duration;

/// The fast (20 ms) polling lasts this long after the kill, then it slows down.
pub const EXIT_POLL_FAST_MS: u64 = 5000;
/// The interval while the exit is expected soon.
pub const FAST_INTERVAL: Duration = Duration::from_millis(20);
/// The interval once the kill is stuck.
pub const SLOW_INTERVAL: Duration = Duration::from_millis(1000);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PollStep {
    Done,
    Wait(Duration),
    Stuck(Duration),
}

pub struct ExitPoller {
    fast: bool,
}

impl ExitPoller {
    pub fn new() -> Self {
        Self { fast: true }
    }

    /// Called once per read of the exit code, `elapsed` since the kill. exited -> Done. else if fast && elapsed >= 5000 ms ->
    /// fast = false and Stuck(SLOW_INTERVAL) (the caller logs "kill-stuck" once and waits SLOW_INTERVAL). else Wait(FAST or SLOW interval by `fast`).
    pub fn step(&mut self, elapsed: Duration, exited: bool) -> PollStep {
        if exited {
            return PollStep::Done;
        }
        if self.fast && elapsed >= Duration::from_millis(EXIT_POLL_FAST_MS) {
            self.fast = false;
            return PollStep::Stuck(SLOW_INTERVAL);
        }
        PollStep::Wait(if self.fast {
            FAST_INTERVAL
        } else {
            SLOW_INTERVAL
        })
    }
}

impl Default for ExitPoller {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Virtual-time driver: reads at `next`, follows the steps, stops at `until` or once done.
    struct Sim {
        p: ExitPoller,
        next: Duration,
        reads: u32,
        stuck: u32,
        done: u32,
    }

    impl Sim {
        fn run(&mut self, until: Duration, exited: bool) {
            while self.next <= until && self.done == 0 {
                self.reads += 1;
                match self.p.step(self.next, exited) {
                    PollStep::Done => {
                        self.done += 1;
                        return;
                    }
                    PollStep::Wait(d) => self.next += d,
                    PollStep::Stuck(d) => {
                        self.stuck += 1;
                        self.next += d;
                    }
                }
            }
        }
    }

    #[test]
    fn polls_fast_then_slow_says_stuck_once_and_hears_a_late_exit() {
        let mut s = Sim {
            p: ExitPoller::new(),
            next: ms(20),
            reads: 0,
            stuck: 0,
            done: 0,
        };
        s.run(ms(5000), false);
        assert_eq!(s.reads, 250);
        assert_eq!(s.stuck, 1);
        s.reads = 0;
        s.run(ms(65_000), false);
        assert_eq!(s.reads, 60);
        assert_eq!(s.stuck, 1);
        s.run(ms(66_000), true);
        assert_eq!(s.done, 1);
        s.reads = 0;
        s.run(ms(76_000), true);
        assert_eq!(s.reads, 0);
    }

    #[test]
    fn exited_is_done_at_once() {
        let mut p = ExitPoller::new();
        assert_eq!(p.step(ms(0), true), PollStep::Done);
    }
}
