//! The reaping job off Windows. macOS: phase 7 of the port plan (a process group per child and a reaper).
//! Until then no job can be made and every `assign` fails, so no engine child is ever started outside a
//! reaper: the callers refuse to run a child they cannot reap.

use std::io;
use std::process::Child;

fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "the reaping job is not built on this platform yet",
    )
}

/// A job that holds nothing (the same shape as on Windows).
pub struct Job {
    _private: (),
}

impl Job {
    /// Refused until phase 7.
    pub fn new() -> io::Result<Self> {
        Err(unsupported())
    }

    /// Refused until phase 7: the caller then kills the child.
    pub fn assign(&self, _child: &Child) -> io::Result<()> {
        Err(unsupported())
    }

    /// No process is in a job here.
    pub fn contains(&self, _pid: u32) -> bool {
        false
    }

    /// A job every `assign` fails on: for tests of the failure path only.
    #[doc(hidden)]
    pub fn unusable_for_tests() -> Self {
        Job { _private: () }
    }
}

/// None until phase 7.
pub fn shared() -> Option<&'static Job> {
    None
}
