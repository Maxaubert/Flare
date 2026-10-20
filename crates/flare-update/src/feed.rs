//! M9 Task 9: the feed trait, the fake feed and the offer rules.
//!
//! The engine only ever sees a [`Feed`]. The real GitHub backend lives in `velopack_feed.rs` (behind
//! the `live-updates` feature); everything else here touches no disk and no network: [`FakeFeed`]
//! is a scripted double, [`NullFeed`] is what a build without a live source holds, and
//! [`RefusingFeed`] models Prism Terminal's `PT_E2E_UPDATE_OFFER` (an offer whose installer is
//! refused before any request is sent).
//!
//! Ported from Prism Terminal v0.55.6 `src/main/update.ts` (`latestUpdate`).

use std::fmt;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use crate::gate::CancelToken;
use crate::notes::MAX_BODY_CHARS;
use crate::types::Offer;
use crate::version::newer_version;

/// What a check found: the tag without its `v`, and the raw release body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub version: String,
    pub notes: String,
}

/// Why a feed call failed. The message is for the log only; the window says INSTALL_FAILED.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedError {
    Offline(String),
    Cancelled,
    Failed(String),
}

impl fmt::Display for FeedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FeedError::Offline(m) | FeedError::Failed(m) => f.write_str(m),
            FeedError::Cancelled => f.write_str("cancelled"),
        }
    }
}

impl std::error::Error for FeedError {}

/// A one-shot gate a test can hold a download behind: `wait` blocks until `open` is called.
#[derive(Clone, Default)]
pub struct Latch(Arc<(Mutex<bool>, Condvar)>);

impl Latch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(&self) {
        let (lock, cv) = &*self.0;
        *lock_it(lock) = true;
        cv.notify_all();
    }

    pub fn wait(&self) {
        let (lock, cv) = &*self.0;
        let mut open = lock_it(lock);
        while !*open {
            open = cv.wait(open).unwrap_or_else(|e| e.into_inner()); // sync-ok: test double, runs on the download's own worker
        }
    }
}

fn lock_it<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The seam between the engine and a release source.
pub trait Feed: Send + Sync {
    /// `Ok(None)`: the remote is empty or nothing is newer. An `Ok(None)` or an `Err` does NOT clear
    /// a stored earlier find.
    fn check(&self) -> Result<Option<Found>, FeedError>;
    /// Downloads the stored find ONLY when its version is `version` (else `Failed("not offered")`);
    /// `Failed("busy")` while an abandoned download still runs. Progress is whole percent.
    fn download(
        &self,
        version: &str,
        progress: &mut dyn FnMut(u8),
        cancel: &CancelToken,
    ) -> Result<(), FeedError>;
    /// Same version rule. Starts the updater, which waits for this process to exit; `Err` when it
    /// could not start.
    fn apply(&self, version: &str, restart_args: &[String]) -> Result<(), FeedError>;
    /// Removes whatever a download wrote. Never fails loudly; idempotent.
    fn discard(&self);
}

/// Turns a find into an offer. Strips one leading `v` or `V`; `None` when the version is empty or
/// not newer than `current`. The notes are the head of the body (at most [`MAX_BODY_CHARS`] UTF-16
/// units, no ellipsis). The token is the counter, incremented first, so the first offer is token 1.
pub fn offer_from(found: &Found, current: &str, next_token: &mut u64) -> Option<Offer> {
    let version = found
        .version
        .strip_prefix(['v', 'V'])
        .unwrap_or(&found.version);
    if version.is_empty() || !newer_version(version, current) {
        return None;
    }
    *next_token += 1;
    Some(Offer {
        version: version.to_string(),
        notes: head_utf16(&found.notes, MAX_BODY_CHARS),
        mock: false,
        token: *next_token,
    })
}

/// The head of `s` in at most `n` UTF-16 units. A surrogate pair that would straddle the cut is
/// dropped whole, so the result is never split inside a character.
fn head_utf16(s: &str, n: usize) -> String {
    let mut used = 0;
    let mut end = 0;
    for (i, c) in s.char_indices() {
        let w = c.len_utf16();
        if used + w > n {
            break;
        }
        used += w;
        end = i + c.len_utf8();
    }
    s[..end].to_string()
}

#[derive(Clone)]
enum Check {
    Find(Option<Found>),
    Error(String),
}

#[derive(Default)]
struct Script {
    check: Option<Check>,
    slow_check: Option<Duration>,
    progress: Option<Vec<u8>>,
    fail_download_at: Option<u8>,
    hold: Option<Latch>,
    fail_apply: bool,
    stored: Option<Found>,
    busy: bool,
    files_left: bool,
    checks: usize,
    downloads: usize,
    applies: usize,
    discards: usize,
    refused: usize,
    applied_args: Option<Vec<String>>,
}

/// Test double: touches no disk and no network. Clones share one script, so a test keeps a clone
/// to read the counters while the engine holds another.
#[derive(Clone, Default)]
pub struct FakeFeed(Arc<Mutex<Script>>);

const DEFAULT_PROGRESS: [u8; 21] = [
    0, 5, 10, 15, 20, 25, 30, 35, 40, 45, 50, 55, 60, 65, 70, 75, 80, 85, 90, 95, 100,
];

impl FakeFeed {
    pub fn new() -> Self {
        Self::default()
    }

    fn with<R>(&self, f: impl FnOnce(&mut Script) -> R) -> R {
        f(&mut lock_it(&self.0))
    }

    /// Builder: what the next checks find.
    pub fn offering(self, version: &str, notes: &str) -> Self {
        self.next_offering(version, notes);
        self
    }

    /// What every later check finds (until changed). Usable on a shared clone mid-test.
    pub fn next_offering(&self, version: &str, notes: &str) {
        self.with(|s| {
            s.check = Some(Check::Find(Some(Found {
                version: version.into(),
                notes: notes.into(),
            })));
        });
    }

    /// Builder: every check finds nothing newer.
    pub fn nothing(self) -> Self {
        self.with(|s| s.check = Some(Check::Find(None)));
        self
    }

    /// Builder: every check fails with `msg`.
    pub fn check_error(self, msg: &str) -> Self {
        self.with(|s| s.check = Some(Check::Error(msg.into())));
        self
    }

    /// Builder: the progress steps a download reports (default 0, 5, ..., 100).
    pub fn progress(self, steps: &[u8]) -> Self {
        self.with(|s| s.progress = Some(steps.to_vec()));
        self
    }

    /// Builder: a download fails once it reaches `pct` (before reporting that step).
    pub fn fail_download_at(self, pct: u8) -> Self {
        self.with(|s| s.fail_download_at = Some(pct));
        self
    }

    /// Builder: each download waits on `latch` before every progress step.
    pub fn hold_download(self, latch: &Latch) -> Self {
        self.with(|s| s.hold = Some(latch.clone()));
        self
    }

    /// Builder: `apply` fails.
    pub fn fail_apply(self) -> Self {
        self.with(|s| s.fail_apply = true);
        self
    }

    /// Builder: every check sleeps for `d` first.
    pub fn slow_check(self, d: Duration) -> Self {
        self.with(|s| s.slow_check = Some(d));
        self
    }

    pub fn checks(&self) -> usize {
        self.with(|s| s.checks)
    }

    pub fn downloads(&self) -> usize {
        self.with(|s| s.downloads)
    }

    pub fn applies(&self) -> usize {
        self.with(|s| s.applies)
    }

    pub fn discards(&self) -> usize {
        self.with(|s| s.discards)
    }

    /// Download and apply calls refused for a version the feed did not find.
    pub fn refused(&self) -> usize {
        self.with(|s| s.refused)
    }

    pub fn applied_args(&self) -> Option<Vec<String>> {
        self.with(|s| s.applied_args.clone())
    }

    /// Models the real feed's joiner thread, which removes an abandoned download's files itself.
    pub fn join_abandoned(&self) {
        self.with(|s| s.files_left = false);
    }

    /// True between a download that started writing and the next `discard`.
    pub fn files_left(&self) -> bool {
        self.with(|s| s.files_left)
    }
}

impl Feed for FakeFeed {
    fn check(&self) -> Result<Option<Found>, FeedError> {
        let (slow, step) = self.with(|s| {
            s.checks += 1;
            (s.slow_check, s.check.clone())
        });
        if let Some(d) = slow {
            std::thread::sleep(d); // sync-ok: test double, runs on the engine's worker
        }
        match step {
            None => Ok(None),
            Some(Check::Error(m)) => Err(FeedError::Failed(m)),
            Some(Check::Find(found)) => {
                if let Some(f) = &found {
                    self.with(|s| s.stored = Some(f.clone()));
                }
                Ok(found)
            }
        }
    }

    fn download(
        &self,
        version: &str,
        progress: &mut dyn FnMut(u8),
        cancel: &CancelToken,
    ) -> Result<(), FeedError> {
        let (steps, fail_at, hold) = self.with(|s| {
            if s.busy {
                return Err(FeedError::Failed("busy".into()));
            }
            if s.stored.as_ref().map(|f| f.version.as_str()) != Some(version) {
                s.refused += 1;
                return Err(FeedError::Failed("not offered".into()));
            }
            s.busy = true;
            s.downloads += 1;
            s.files_left = true;
            let steps = s
                .progress
                .clone()
                .unwrap_or_else(|| DEFAULT_PROGRESS.to_vec());
            Ok((steps, s.fail_download_at, s.hold.clone()))
        })?;
        let result = (|| {
            for step in steps {
                if let Some(latch) = &hold {
                    latch.wait(); // sync-ok: test double hold, runs on the download worker
                }
                if cancel.is_cancelled() {
                    return Err(FeedError::Cancelled);
                }
                if fail_at.is_some_and(|p| step >= p) {
                    return Err(FeedError::Failed("download failed".into()));
                }
                progress(step);
            }
            Ok(())
        })();
        self.with(|s| s.busy = false);
        result
    }

    fn apply(&self, version: &str, restart_args: &[String]) -> Result<(), FeedError> {
        self.with(|s| {
            if s.stored.as_ref().map(|f| f.version.as_str()) != Some(version) {
                s.refused += 1;
                return Err(FeedError::Failed("not offered".into()));
            }
            if s.fail_apply {
                return Err(FeedError::Failed("apply failed".into()));
            }
            s.applies += 1;
            s.applied_args = Some(restart_args.to_vec());
            Ok(())
        })
    }

    fn discard(&self) {
        self.with(|s| {
            s.discards += 1;
            s.files_left = false;
        });
    }
}

/// What a build with no live source holds: it checks nothing and touches nothing.
#[derive(Clone, Copy, Debug, Default)]
pub struct NullFeed;

impl Feed for NullFeed {
    fn check(&self) -> Result<Option<Found>, FeedError> {
        Ok(None)
    }

    fn download(
        &self,
        _version: &str,
        _progress: &mut dyn FnMut(u8),
        _cancel: &CancelToken,
    ) -> Result<(), FeedError> {
        Err(FeedError::Failed("off".into()))
    }

    fn apply(&self, _version: &str, _restart_args: &[String]) -> Result<(), FeedError> {
        Err(FeedError::Failed("off".into()))
    }

    fn discard(&self) {}
}

/// Models Prism Terminal's `PT_E2E_UPDATE_OFFER`: it offers 99.0.0, and its installer is refused
/// before any progress or request. Built only under `--e2e` (Task 32).
#[derive(Clone, Debug)]
pub struct RefusingFeed {
    notes: String,
}

impl RefusingFeed {
    pub fn new(notes: &str) -> Self {
        Self {
            notes: notes.to_string(),
        }
    }
}

impl Feed for RefusingFeed {
    fn check(&self) -> Result<Option<Found>, FeedError> {
        Ok(Some(Found {
            version: "99.0.0".into(),
            notes: self.notes.clone(),
        }))
    }

    fn download(
        &self,
        _version: &str,
        _progress: &mut dyn FnMut(u8),
        _cancel: &CancelToken,
    ) -> Result<(), FeedError> {
        Err(FeedError::Failed("refused".into()))
    }

    fn apply(&self, _version: &str, _restart_args: &[String]) -> Result<(), FeedError> {
        Err(FeedError::Failed("refused".into()))
    }

    fn discard(&self) {}
}
