//! The engine job queue (Task 13): one pass at a time, finals before partials, at most one waiting
//! partial. Ported from the queue rules of Prism Terminal's `dictationEngine.ts` (e5848cb).
//!
//! A FINAL is what gets pasted, so it is never dropped and goes ahead of any partial. A PARTIAL is only
//! the pill's live text, so at most one waits: whatever arrives (a newer partial or a final) makes the
//! waiting one out of date, and `push` hands it back so the caller can answer it `Superseded` without it
//! ever being sent.

use std::collections::VecDeque;

/// The waiting jobs of one engine. `J` is whatever the engine queues.
#[derive(Debug)]
pub struct Queue<J> {
    finals: VecDeque<J>,
    partial: Option<J>,
}

impl<J> Default for Queue<J> {
    fn default() -> Self {
        Queue {
            finals: VecDeque::new(),
            partial: None,
        }
    }
}

impl<J> Queue<J> {
    /// An empty queue.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a job. Returns the partial that was waiting and is now out of date, if any: the caller
    /// answers it `Superseded`.
    #[must_use]
    pub fn push(&mut self, job: J, is_final: bool) -> Option<J> {
        let superseded = self.partial.take();
        if is_final {
            self.finals.push_back(job);
        } else {
            self.partial = Some(job);
        }
        superseded
    }

    /// Adds a final WITHOUT touching the waiting partial: the warm-up pass goes this way (it is a
    /// final-queue job, but it is not a clip, so it covers nothing).
    pub fn push_final_keeping_partial(&mut self, job: J) {
        self.finals.push_back(job);
    }

    /// The next job to run: the oldest final, else the waiting partial.
    pub fn pop(&mut self) -> Option<J> {
        self.finals.pop_front().or_else(|| self.partial.take())
    }

    /// Everything waiting, finals first, emptying the queue (used by stop).
    pub fn drain(&mut self) -> Vec<J> {
        let mut all: Vec<J> = self.finals.drain(..).collect();
        all.extend(self.partial.take());
        all
    }

    /// Nothing is waiting.
    pub fn is_empty(&self) -> bool {
        self.finals.is_empty() && self.partial.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_newer_partial_takes_the_waiting_partials_place() {
        let mut q = Queue::new();
        assert_eq!(q.push("b", false), None);
        assert_eq!(q.push("c", false), Some("b"));
        assert_eq!(q.pop(), Some("c"));
        assert_eq!(q.pop(), None);
    }

    #[test]
    fn a_final_jumps_ahead_and_drops_the_waiting_partial() {
        let mut q = Queue::new();
        assert_eq!(q.push("p1", false), None);
        assert_eq!(q.push("f1", true), Some("p1"));
        assert_eq!(q.push("p2", false), None);
        assert_eq!(q.push("f2", true), Some("p2"));
        assert_eq!(q.push("p3", false), None);
        assert_eq!(q.pop(), Some("f1"));
        assert_eq!(q.pop(), Some("f2"));
        assert_eq!(q.pop(), Some("p3"));
        assert!(q.is_empty());
    }

    #[test]
    fn drain_empties_the_queue_finals_first() {
        let mut q = Queue::new();
        assert_eq!(q.push("f1", true), None);
        assert_eq!(q.push("p", false), None);
        assert_eq!(q.drain(), vec!["f1", "p"]);
        assert!(q.is_empty());
    }
}
