//! FitHold: holds the terminal size while the sidebar slides (Task 11).
//!
//! Source: Prism Terminal's core `fitHold.ts`. While a host animates the room a terminal has, every
//! terminal keeps the columns it has; a fit that arrives meanwhile waits and runs once when the hold
//! ends. Time is an argument, never read.

use std::panic::{AssertUnwindSafe, catch_unwind};

/// The id of one waiting fit (a terminal's, in the glue).
pub type FitId = u32;

#[derive(Debug, Default)]
pub struct FitHold {
    until: Option<u64>,
    waiting: Vec<FitId>,
}

impl FitHold {
    pub fn new() -> Self {
        Self::default()
    }

    /// Hold every terminal's size for `ms` from `now`; a second hold extends the first (it
    /// replaces the end).
    pub fn hold(&mut self, ms: u64, now: u64) {
        self.until = Some(now + ms);
    }

    pub fn held(&self, now: u64) -> bool {
        self.until.is_some_and(|end| end > now)
    }

    /// Run `id` when the hold ends; the same id twice runs once.
    pub fn when_released(&mut self, id: FitId) {
        if !self.waiting.contains(&id) {
            self.waiting.push(id);
        }
    }

    /// Once the hold has ended: the waiting ids, once each, in order. Empty while held.
    pub fn poll(&mut self, now: u64) -> Vec<FitId> {
        if self.held(now) {
            return Vec::new();
        }
        self.until = None;
        std::mem::take(&mut self.waiting)
    }

    /// End the hold at once; runs each waiter, a panic in one never stops the next. Returns the ids
    /// in the order they were run.
    pub fn release_now(&mut self, run: impl FnMut(FitId)) -> Vec<FitId> {
        self.until = None;
        let waiters = std::mem::take(&mut self.waiting);
        let mut run = run;
        for &id in &waiters {
            // One fit that panics is that terminal's problem; the rest still run.
            let _ = catch_unwind(AssertUnwindSafe(|| run(id)));
        }
        waiters
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_hold_runs_each_waiter_once_when_it_ends() {
        let mut f = FitHold::new();
        f.hold(280, 0);
        assert!(f.held(0));
        f.when_released(7);
        f.when_released(7);
        assert_eq!(f.poll(279), Vec::<FitId>::new());
        assert!(f.held(279));
        assert_eq!(f.poll(280), vec![7]);
        assert!(!f.held(280));
        assert_eq!(f.poll(300), Vec::<FitId>::new());
    }

    #[test]
    fn a_second_hold_extends_the_first() {
        let mut f = FitHold::new();
        f.hold(200, 0);
        assert_eq!(f.poll(150), Vec::<FitId>::new());
        f.hold(200, 150);
        f.when_released(7);
        assert_eq!(f.poll(300), Vec::<FitId>::new());
        assert_eq!(f.poll(350), vec![7]);
    }

    #[test]
    fn a_panicking_fit_does_not_stop_the_next() {
        let mut f = FitHold::new();
        f.hold(100, 0);
        f.when_released(1);
        f.when_released(2);
        let mut ran = Vec::new();
        let done = f.release_now(|id| {
            if id == 1 {
                panic!("gone");
            }
            ran.push(id);
        });
        assert_eq!(done, vec![1, 2]);
        assert_eq!(ran, vec![2]);
        assert!(!f.held(0));
    }
}
