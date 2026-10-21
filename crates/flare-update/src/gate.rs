//! M9 Task 8: the install gate.
//!
//! What an install may do (regression rule 3: the app installs only what it offered).
//!
//! - WHAT IS ON OFFER decides, never what the caller sent. While the offer is a preview (a mock),
//!   nothing is downloaded, whatever token is handed in: the preview runs its own fake and answers
//!   false, "nothing was installed".
//! - WHAT WAS OFFERED is installed, and only when the caller names that offer's token. No offer, or
//!   any other token, answers false and touches nothing.
//! - ONE AT A TIME. `cancel` aborts whatever run is current. A run's slot is cleared when the run
//!   ends, also when it panics, so the gate is usable again.
//! - The close question is pre-answered at the quit, not before: `agree` is handed to the installer,
//!   which calls it at the moment it quits.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::types::Offer;

/// A cancellation flag shared by one install run and whoever may cancel it.
#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// What the gate asks of the outside world. The real one is the engine; tests use a fake.
pub trait GateDeps: Send + Sync {
    /// The offer currently on the table, if any.
    fn offer(&self) -> Option<Offer>;
    /// Run the preview (a mock offer): downloads nothing; `cancelled` says when to stop.
    fn preview(&self, cancelled: &dyn Fn() -> bool) -> bool;
    /// Download and install the offer `token`; `agree` is called when the installer quits the app.
    fn install(&self, token: u64, cancel: &CancelToken, agree: &dyn Fn()) -> bool;
    /// Pre-answer the close question.
    fn agree(&self);
}

/// The install gate: one run at a time, every decision made from the offer on the table.
pub struct Gate {
    deps: Arc<dyn GateDeps>,
    run: Mutex<Option<CancelToken>>,
}

impl Gate {
    pub fn new(deps: Arc<dyn GateDeps>) -> Self {
        Self {
            deps,
            run: Mutex::new(None),
        }
    }

    /// Install the offer named by `token`. False when a run is already current, when there is no
    /// offer, when the token is not the offer's, or when the preview (a mock offer) installs nothing.
    pub fn install(&self, token: Option<u64>) -> bool {
        let mine = CancelToken::default();
        if !self.claim(&mine) {
            return false;
        }
        // Cleared on every exit from here, a panic included.
        let _clear = ClearRun(&self.run);
        match self.deps.offer() {
            Some(offer) if offer.mock => self.deps.preview(&|| mine.is_cancelled()),
            Some(offer) if token == Some(offer.token) => {
                self.deps.install(offer.token, &mine, &|| self.deps.agree())
            }
            _ => false,
        }
    }

    /// Abort the current run, if there is one.
    pub fn cancel(&self) {
        let current = lock(&self.run).clone();
        if let Some(token) = current {
            token.cancel();
        }
    }

    /// Is a run current?
    pub fn is_running(&self) -> bool {
        lock(&self.run).is_some()
    }

    /// Make `mine` the current run; false when one is already current.
    fn claim(&self, mine: &CancelToken) -> bool {
        let mut run = lock(&self.run);
        if run.is_some() {
            return false;
        }
        *run = Some(mine.clone());
        true
    }
}

/// Locks the run slot. Poisoning is ignored: the slot holds no invariant a panic could break.
fn lock(run: &Mutex<Option<CancelToken>>) -> MutexGuard<'_, Option<CancelToken>> {
    run.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Empties the run slot when dropped.
struct ClearRun<'a>(&'a Mutex<Option<CancelToken>>);

impl Drop for ClearRun<'_> {
    fn drop(&mut self) {
        *lock(self.0) = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::thread;
    use std::time::Duration;

    fn offer(token: u64, mock: bool) -> Offer {
        Offer {
            version: "1.2.3".to_string(),
            notes: String::new(),
            mock,
            token,
        }
    }

    /// A fake the tests script: an offer, a record of every call, and switches that make install
    /// hold until cancelled or panic.
    #[derive(Default)]
    struct Fake {
        offer: Mutex<Option<Offer>>,
        installs: Mutex<Vec<u64>>,
        previews: AtomicUsize,
        agrees: AtomicUsize,
        started: AtomicBool,
        hold: AtomicBool,
        panic_next: AtomicBool,
    }

    impl Fake {
        fn with_offer(o: Option<Offer>) -> Arc<Self> {
            Arc::new(Self {
                offer: Mutex::new(o),
                ..Self::default()
            })
        }
    }

    impl GateDeps for Fake {
        fn offer(&self) -> Option<Offer> {
            self.offer
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }

        fn preview(&self, _cancelled: &dyn Fn() -> bool) -> bool {
            self.previews.fetch_add(1, Ordering::SeqCst);
            false
        }

        fn install(&self, token: u64, cancel: &CancelToken, agree: &dyn Fn()) -> bool {
            self.installs
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(token);
            if self.panic_next.swap(false, Ordering::SeqCst) {
                panic!("the installer fell over");
            }
            self.started.store(true, Ordering::SeqCst);
            if self.hold.load(Ordering::SeqCst) {
                while !cancel.is_cancelled() {
                    thread::sleep(Duration::from_millis(5));
                }
                return false;
            }
            agree();
            true
        }

        fn agree(&self) {
            self.agrees.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn installs_the_offer_it_made_handing_the_installer_the_close_pre_answer() {
        let fake = Fake::with_offer(Some(offer(7, false)));
        let gate = Gate::new(fake.clone());
        assert!(gate.install(Some(7)));
        assert_eq!(*fake.installs.lock().unwrap(), vec![7]);
        // The installer called `agree` (the gate passes `deps.agree`) at the quit.
        assert_eq!(fake.agrees.load(Ordering::SeqCst), 1);
        assert!(!gate.is_running());
    }

    #[test]
    fn refuses_any_other_token_a_none_and_everything_with_no_offer() {
        let fake = Fake::with_offer(Some(offer(7, false)));
        let gate = Gate::new(fake.clone());
        assert!(!gate.install(Some(8)));
        assert!(!gate.install(None));
        assert!(fake.installs.lock().unwrap().is_empty());

        let empty = Fake::with_offer(None);
        let gate = Gate::new(empty.clone());
        assert!(!gate.install(Some(7)));
        assert!(!gate.install(None));
        assert!(empty.installs.lock().unwrap().is_empty());
    }

    #[test]
    fn a_preview_downloads_nothing_whatever_token_it_is_handed() {
        let fake = Fake::with_offer(Some(offer(0, true)));
        let gate = Gate::new(fake.clone());
        assert!(!gate.install(Some(999)));
        assert_eq!(fake.previews.load(Ordering::SeqCst), 1);
        assert!(fake.installs.lock().unwrap().is_empty());
    }

    #[test]
    fn runs_one_at_a_time_and_cancel_aborts_the_one_running() {
        let fake = Fake::with_offer(Some(offer(7, false)));
        fake.hold.store(true, Ordering::SeqCst);
        let gate = Gate::new(fake.clone());
        thread::scope(|s| {
            let first = s.spawn(|| gate.install(Some(7)));
            while !fake.started.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(1));
            }
            assert!(gate.is_running());
            // A second install while one blocks is refused and reaches nothing.
            assert!(!gate.install(Some(7)));
            assert_eq!(fake.installs.lock().unwrap().len(), 1);
            gate.cancel();
            assert!(!first.join().unwrap());
        });
        assert!(!gate.is_running());
        // After the cancelled run ends, a new install is allowed.
        fake.hold.store(false, Ordering::SeqCst);
        assert!(gate.install(Some(7)));
    }

    #[test]
    fn a_panic_in_install_clears_the_run() {
        let fake = Fake::with_offer(Some(offer(7, false)));
        fake.panic_next.store(true, Ordering::SeqCst);
        let gate = Gate::new(fake.clone());
        let caught =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| gate.install(Some(7))));
        assert!(caught.is_err());
        assert!(!gate.is_running());
        assert!(gate.install(Some(7)));
    }
}
