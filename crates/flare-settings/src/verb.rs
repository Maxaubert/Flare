//! The Explorer verb seam: the menu row's state, and the pending implementation until M9 (M7 Task 29).
//!
//! Task 19 adds the state the page reads and the diagnostics seam the page drives. Task 29 adds
//! the `ExplorerVerb` trait and its two implementations below these lines.

use std::cell::Cell;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

/// The state of the Explorer menu row. `Busy` is Prism Terminal's "Checking with Windows." (disabled).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerbState {
    Busy,
    On,
    Off,
}

/// The diagnostics control: the verbose flag file and the log folder (M7 implements it, M9 owns
/// the writer; `mark` forwards to M6's `DiagSink`).
pub trait DiagControlApi {
    fn verbose(&self) -> bool;
    /// Returns whether the flag was stored.
    fn set_verbose(&self, on: bool) -> bool;
    fn mark(&self);
    fn dir(&self) -> PathBuf;
}

/// A test double for `DiagControlApi`: counts marks, keeps the flag in a cell, and can refuse writes.
pub struct FakeDiag {
    verbose: Cell<bool>,
    marks: Cell<usize>,
    reads: Cell<usize>,
    dir: PathBuf,
    refuse: Cell<bool>,
}

impl FakeDiag {
    /// A fake that stores the flag and accepts every write.
    pub fn new(dir: impl Into<PathBuf>, verbose: bool) -> Self {
        Self {
            verbose: Cell::new(verbose),
            marks: Cell::new(0),
            reads: Cell::new(0),
            dir: dir.into(),
            refuse: Cell::new(false),
        }
    }

    /// Makes every later `set_verbose` refuse (the flag keeps its old value).
    pub fn refusing(&self) {
        self.refuse.set(true);
    }

    /// How many times `verbose` was read (the real one reads a file).
    pub fn reads(&self) -> usize {
        self.reads.get()
    }

    /// How many times `mark` ran.
    pub fn marks(&self) -> usize {
        self.marks.get()
    }
}

impl DiagControlApi for FakeDiag {
    fn verbose(&self) -> bool {
        self.reads.set(self.reads.get() + 1);
        self.verbose.get()
    }

    fn set_verbose(&self, on: bool) -> bool {
        if self.refuse.get() {
            return false;
        }
        self.verbose.set(on);
        true
    }

    fn mark(&self) {
        self.marks.set(self.marks.get() + 1);
    }

    fn dir(&self) -> PathBuf {
        self.dir.clone()
    }
}

/// The Explorer verb seam: the menu row reads `status` and asks for a change with `set`.
pub trait ExplorerVerb: Send + Sync {
    fn status(&self) -> VerbState;
    fn set(&self, on: bool) -> VerbState;
}

/// M7's production backend. Until M9 it answers `Busy`, so the row reads "Checking with Windows."
/// and stays disabled.
#[derive(Clone, Copy, Debug, Default)]
pub struct PendingVerb;

impl ExplorerVerb for PendingVerb {
    fn status(&self) -> VerbState {
        VerbState::Busy
    }

    fn set(&self, _on: bool) -> VerbState {
        VerbState::Busy
    }
}

/// A test double that keeps its state in a mutex and answers the state it just set.
#[derive(Debug)]
pub struct FakeVerb {
    state: Mutex<VerbState>,
}

impl FakeVerb {
    pub fn new(state: VerbState) -> Self {
        Self {
            state: Mutex::new(state),
        }
    }
}

impl ExplorerVerb for FakeVerb {
    fn status(&self) -> VerbState {
        *self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn set(&self, on: bool) -> VerbState {
        let next = if on { VerbState::On } else { VerbState::Off };
        *self.state.lock().unwrap_or_else(PoisonError::into_inner) = next;
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fake_stores_the_flag_and_counts_marks() {
        let d = FakeDiag::new("x", false);
        assert!(!d.verbose());
        assert!(d.set_verbose(true));
        assert!(d.verbose());
        d.mark();
        d.mark();
        assert_eq!(d.marks(), 2);
        assert_eq!(d.dir(), PathBuf::from("x"));
    }

    #[test]
    fn a_refusing_fake_keeps_the_old_flag() {
        let d = FakeDiag::new("x", false);
        d.refusing();
        assert!(!d.set_verbose(true));
        assert!(!d.verbose());
    }

    #[test]
    fn the_pending_verb_is_busy_and_stays_busy() {
        let v = PendingVerb;
        assert_eq!(v.status(), VerbState::Busy);
        assert_eq!(v.set(true), VerbState::Busy);
        assert_eq!(v.status(), VerbState::Busy);
        assert_eq!(v.set(false), VerbState::Busy);
        assert_eq!(v.status(), VerbState::Busy);
    }

    #[test]
    fn the_fake_verb_flips_and_answers_the_new_state() {
        let v = FakeVerb::new(VerbState::Off);
        assert_eq!(v.status(), VerbState::Off);
        assert_eq!(v.set(true), VerbState::On);
        assert_eq!(v.status(), VerbState::On);
        assert_eq!(v.set(false), VerbState::Off);
        assert_eq!(v.status(), VerbState::Off);
    }
}
