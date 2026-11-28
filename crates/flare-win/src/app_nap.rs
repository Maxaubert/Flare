//! No App Nap while this Flare serves a remote session (rule 1392). macOS naps an app it thinks nobody uses (a host Mac
//! whose Flare sits behind other windows, or hidden with `--start-hidden`): its timers are put off by seconds and its
//! I/O runs at background priority, so a remote viewer's keys echo late. While at least one tab is followed by a remote
//! viewer, the host holds one activity of the kind "user initiated, latency critical" that still lets the Mac sleep when
//! idle (`NSActivityUserInitiatedAllowingIdleSystemSleep | NSActivityLatencyCritical`): whether it stays awake is the
//! keep-awake's call alone (rules 296, 1302). The last follower gone ends the activity.
//!
//! [`NoNap::hold`] gives a [`NapHold`]; the activity lives while any hold does. Elsewhere than macOS nothing is held.
//! [`FakeNoNap`] counts holds for the tests; it never asks the system. A hold stays on the thread that took it (the
//! window's): an activity token is no `Send`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// What the system is told while a hold lives (`pmset -g assertions` and Activity Monitor show it).
pub const REASON: &str = "Flare: serving a remote session";

/// Something that can keep this app out of App Nap.
pub trait NoNap: Send + Sync {
    /// Keeps the app out of App Nap until the returned [`NapHold`] is dropped.
    fn hold(&self) -> NapHold;
}

/// One hold off App Nap. Dropping it lets go.
pub struct NapHold {
    release: Option<Box<dyn FnOnce()>>,
}

impl NapHold {
    /// A hold whose drop runs `release` once.
    pub fn new(release: impl FnOnce() + 'static) -> Self {
        Self {
            release: Some(Box::new(release)),
        }
    }
}

impl Drop for NapHold {
    fn drop(&mut self) {
        if let Some(r) = self.release.take() {
            r();
        }
    }
}

impl std::fmt::Debug for NapHold {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NapHold")
    }
}

/// The system's: an `NSProcessInfo` activity on macOS, nothing elsewhere.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemNoNap;

impl NoNap for SystemNoNap {
    fn hold(&self) -> NapHold {
        sys::begin()
    }
}

/// A test's: counts the holds alive and taken; never asks the system.
#[derive(Clone, Default)]
pub struct FakeNoNap {
    held: Arc<AtomicUsize>,
    taken: Arc<AtomicUsize>,
}

impl FakeNoNap {
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

impl NoNap for FakeNoNap {
    fn hold(&self) -> NapHold {
        self.held.fetch_add(1, Ordering::SeqCst);
        self.taken.fetch_add(1, Ordering::SeqCst);
        let held = Arc::clone(&self.held);
        NapHold::new(move || {
            held.fetch_sub(1, Ordering::SeqCst);
        })
    }
}

#[cfg(target_os = "macos")]
mod sys {
    use super::{NapHold, REASON};
    use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};

    /// User initiated and latency critical, with idle system sleep still allowed (rules 296, 1302).
    pub fn options() -> NSActivityOptions {
        NSActivityOptions(
            NSActivityOptions::UserInitiatedAllowingIdleSystemSleep.0
                | NSActivityOptions::LatencyCritical.0,
        )
    }

    pub fn begin() -> NapHold {
        let token = NSProcessInfo::processInfo()
            .beginActivityWithOptions_reason(options(), &NSString::from_str(REASON));
        NapHold::new(move || {
            // SAFETY: the token came from `beginActivityWithOptions:reason:` of this process and is ended once.
            unsafe { NSProcessInfo::processInfo().endActivity(&token) };
        })
    }
}

#[cfg(not(target_os = "macos"))]
mod sys {
    use super::NapHold;

    /// No App Nap here.
    pub fn begin() -> NapHold {
        NapHold::new(|| {})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fake_counts_holds_and_a_drop_lets_go() {
        let f = FakeNoNap::new();
        let k: Arc<dyn NoNap> = Arc::new(f.clone());
        let a = k.hold();
        let b = k.hold();
        assert_eq!((f.held(), f.taken()), (2, 2));
        drop(a);
        assert_eq!(f.held(), 1);
        drop(b);
        assert_eq!((f.held(), f.taken()), (0, 2));
    }

    /// The real one, taken and let go: it never fails or blocks.
    #[test]
    fn the_system_hold_takes_and_lets_go_without_waiting() {
        let h = SystemNoNap.hold();
        drop(h);
    }

    /// Rule 1392 with rules 296 and 1302: the activity is latency critical and user initiated, and never holds off
    /// idle system sleep (the keep-awake alone decides that).
    #[cfg(target_os = "macos")]
    #[test]
    fn the_activity_is_latency_critical_and_lets_the_mac_sleep() {
        use objc2_foundation::NSActivityOptions as O;
        let o = sys::options().0;
        assert_eq!(o & O::LatencyCritical.0, O::LatencyCritical.0);
        assert_eq!(
            o & O::UserInitiatedAllowingIdleSystemSleep.0,
            O::UserInitiatedAllowingIdleSystemSleep.0
        );
        assert_eq!(o & O::IdleSystemSleepDisabled.0, 0);
    }
}
