//! Keep-awake (remote sessions PR 4, rule 296): the machine that runs a remote session stays awake while that session
//! works, and sleeps as usual once none does (owner decision 2026-11-12).
//!
//! [`KeepAwake::hold`] gives a [`Hold`]; the system is held awake while any hold lives, and let go when the last one is
//! dropped. Every exit path lets go: a drop, the window's end (the hold goes with it), and the process's end, which the
//! system takes as a release on its own (a thread's execution state ends with the thread; a power assertion with the
//! process). Only system sleep is held off: the display may still turn off, and the screen still locks.
//!
//! - Windows: one long-lived thread asks `SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED)` on the first
//!   hold and `SetThreadExecutionState(ES_CONTINUOUS)` after the last (the state is per thread, so one thread owns it).
//! - macOS: `IOPMAssertionCreateWithName` (`PreventUserIdleSystemSleep`) on the first hold, `IOPMAssertionRelease` after
//!   the last (`macos/keep_awake.rs`).
//! - Elsewhere: nothing is held (no such system yet).
//!
//! [`FakeAwake`] counts holds for the tests; it never asks the system.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// Something that can hold the system awake.
pub trait KeepAwake: Send + Sync {
    /// Holds the system awake until the returned [`Hold`] is dropped. Holds nest.
    fn hold(&self) -> Hold;
}

/// One hold on the system's sleep. Dropping it lets go of it.
pub struct Hold {
    release: Option<Box<dyn FnOnce() + Send>>,
}

impl Hold {
    /// A hold whose drop runs `release` once.
    pub fn new(release: impl FnOnce() + Send + 'static) -> Self {
        Self {
            release: Some(Box::new(release)),
        }
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        if let Some(r) = self.release.take() {
            r();
        }
    }
}

impl std::fmt::Debug for Hold {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Hold")
    }
}

/// What asks the system: on at the first hold, off after the last.
trait SystemSleep: Send + 'static {
    fn set(&mut self, awake: bool);
}

struct Counted<S: SystemSleep> {
    holds: usize,
    sys: S,
}

/// The system's keep-awake, counted: the system is asked once on the first hold and once after the last.
pub struct SystemAwake {
    inner: Arc<Mutex<Counted<sys::Awake>>>,
}

fn lk<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl SystemAwake {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Counted {
                holds: 0,
                sys: sys::Awake::new(),
            })),
        }
    }
}

impl Default for SystemAwake {
    fn default() -> Self {
        Self::new()
    }
}

fn counted_hold<S: SystemSleep>(inner: &Arc<Mutex<Counted<S>>>) -> Hold {
    {
        let mut c = lk(inner);
        c.holds += 1;
        if c.holds == 1 {
            c.sys.set(true);
        }
    }
    let inner = Arc::clone(inner);
    Hold::new(move || {
        let mut c = lk(&inner);
        c.holds = c.holds.saturating_sub(1);
        if c.holds == 0 {
            c.sys.set(false);
        }
    })
}

impl KeepAwake for SystemAwake {
    fn hold(&self) -> Hold {
        counted_hold(&self.inner)
    }
}

/// A test's keep-awake: counts the holds alive and the times the system would have been asked; never asks it.
#[derive(Clone, Default)]
pub struct FakeAwake {
    held: Arc<AtomicUsize>,
    taken: Arc<AtomicUsize>,
}

impl FakeAwake {
    pub fn new() -> Self {
        Self::default()
    }

    /// Holds alive now.
    pub fn held(&self) -> usize {
        self.held.load(Ordering::SeqCst)
    }

    /// Holds taken so far.
    pub fn taken(&self) -> usize {
        self.taken.load(Ordering::SeqCst)
    }
}

impl KeepAwake for FakeAwake {
    fn hold(&self) -> Hold {
        self.held.fetch_add(1, Ordering::SeqCst);
        self.taken.fetch_add(1, Ordering::SeqCst);
        let held = Arc::clone(&self.held);
        Hold::new(move || {
            held.fetch_sub(1, Ordering::SeqCst);
        })
    }
}

#[cfg(windows)]
mod sys {
    use std::sync::mpsc;

    const ES_CONTINUOUS: u32 = 0x8000_0000;
    const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetThreadExecutionState(flags: u32) -> u32;
    }

    /// The thread that owns the execution state, started at the first hold and kept for the process's life.
    pub struct Awake {
        tx: Option<mpsc::Sender<bool>>,
    }

    impl Awake {
        pub fn new() -> Self {
            Self { tx: None }
        }

        fn start(&mut self) -> Option<&mpsc::Sender<bool>> {
            if self.tx.is_none() {
                let (tx, rx) = mpsc::channel::<bool>();
                let spawned = std::thread::Builder::new()
                    .name("flare-keep-awake".into())
                    .spawn(move || {
                        // sync-ok: the keep-awake's own thread waits for the next hold or release
                        while let Ok(awake) = rx.recv() {
                            let flags = if awake {
                                ES_CONTINUOUS | ES_SYSTEM_REQUIRED
                            } else {
                                ES_CONTINUOUS
                            };
                            // SAFETY: plain flags; it changes only this thread's execution state.
                            if unsafe { SetThreadExecutionState(flags) } == 0 {
                                log::warn!("keep-awake: the system refused the execution state");
                            }
                        }
                        // The sender is gone: let go before the thread ends (its end lets go too).
                        // SAFETY: as above.
                        unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
                    });
                match spawned {
                    Ok(_) => self.tx = Some(tx),
                    Err(e) => log::warn!("keep-awake: the thread did not start: {e}"),
                }
            }
            self.tx.as_ref()
        }
    }

    impl super::SystemSleep for Awake {
        fn set(&mut self, awake: bool) {
            if !awake && self.tx.is_none() {
                return;
            }
            if let Some(tx) = self.start() {
                let _ = tx.send(awake);
            }
        }
    }
}

/// The tests that ask the real system take turns (`cargo test` runs them in one process, whose assertions they read).
#[cfg(test)]
static REAL: Mutex<()> = Mutex::new(());

#[cfg(target_os = "macos")]
#[path = "macos/keep_awake.rs"]
mod sys;

#[cfg(not(any(windows, target_os = "macos")))]
mod sys {
    /// No system keep-awake here.
    pub struct Awake;

    impl Awake {
        pub fn new() -> Self {
            Awake
        }
    }

    impl super::SystemSleep for Awake {
        fn set(&mut self, _awake: bool) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Recorder(Arc<Mutex<Vec<bool>>>);

    impl SystemSleep for Recorder {
        fn set(&mut self, awake: bool) {
            lk(&self.0).push(awake);
        }
    }

    /// Rule 296: the system is asked once on the first hold and let go once after the last, whatever order the holds
    /// go in.
    #[test]
    fn the_system_is_held_from_the_first_hold_to_the_last_drop() {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let inner = Arc::new(Mutex::new(Counted {
            holds: 0,
            sys: Recorder(Arc::clone(&asked)),
        }));
        let a = counted_hold(&inner);
        let b = counted_hold(&inner);
        assert_eq!(*lk(&asked), [true]);
        drop(a);
        assert_eq!(*lk(&asked), [true], "one hold still lives");
        drop(b);
        assert_eq!(*lk(&asked), [true, false]);
        let c = counted_hold(&inner);
        drop(c);
        assert_eq!(*lk(&asked), [true, false, true, false]);
    }

    #[test]
    fn the_fake_counts_holds_and_a_drop_lets_go() {
        let f = FakeAwake::new();
        let k: Arc<dyn KeepAwake> = Arc::new(f.clone());
        let h = k.hold();
        assert_eq!((f.held(), f.taken()), (1, 1));
        drop(h);
        assert_eq!((f.held(), f.taken()), (0, 1));
    }

    /// The real one, taken and let go: it never fails or blocks (on Windows its thread starts at the first hold).
    #[test]
    fn the_system_keep_awake_takes_and_lets_go_without_waiting() {
        let _turn = lk(&REAL);
        let k = SystemAwake::new();
        let h1 = k.hold();
        let h2 = k.hold();
        drop(h1);
        drop(h2);
        assert_eq!(lk(&k.inner).holds, 0);
    }
}
