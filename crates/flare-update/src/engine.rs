//! M9 Task 10: the update engine (modes, check loop, install run).
//!
//! The engine only ever sees a [`Feed`]; nothing here names Velopack. It owns the one offer, the
//! check schedule (at launch, then every four hours) and the install run, which goes through the
//! [`Gate`]: only the offer on the table is installed, one run at a time, and a cancel wins even
//! after the last byte. The close question is pre-answered at the quit, after the installer was
//! handed off, never at the start of the download.
//!
//! Ported from Prism Terminal v0.55.6 `src/main/update.ts` (`watchForUpdates`, `installUpdate`,
//! `calls`) and `src/main/index.ts` (modes, gate wiring, the second-launch preview).

use std::cell::Cell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, TryLockError};
use std::time::{Duration, Instant};

use crate::feed::{Feed, FeedError, offer_from};
use crate::gate::{CancelToken, Gate, GateDeps};
use crate::handoff::restart_args;
use crate::preview::{PREVIEW_MS, Waiter, preview_update, run_preview_install};
use crate::types::Offer;

/// What the engine does for this launch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Off,
    Preview,
    Live,
}

/// D11: `--preview-update` (or an unpackaged non-e2e build that is not in an app bundle) previews; a
/// packaged, non-e2e build with the `live-updates` feature checks for real; everything else is off.
/// `bundled` is a run from a macOS `.app` that Velopack did not make (rule 1128): it is off, so the
/// ad hoc app shows no fake update chip. It is always false on Windows.
pub fn mode_for(
    packaged: bool,
    bundled: bool,
    e2e: bool,
    preview_flag: bool,
    live_feature: bool,
) -> Mode {
    if preview_flag || (!packaged && !bundled && !e2e) {
        Mode::Preview
    } else if packaged && !e2e && live_feature {
        Mode::Live
    } else {
        Mode::Off
    }
}

/// How often a live build looks again.
pub const CHECK_EVERY_MS: u64 = 4 * 60 * 60 * 1000;
/// The app quits this long after a successful hand-off.
pub const QUIT_AFTER_MS: u64 = 400;

/// How often the engine asked the network, and how often it ran a real install.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Calls {
    pub checks: u64,
    pub installs: u64,
}

/// A crumb field value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fld {
    Bool(bool),
    Num(u64),
    Str(String),
}

/// What the engine tells the app. The glue implements it; a recorder does in tests.
pub trait Events: Send + Sync {
    /// A new offer exists (the glue posts it to the UI thread).
    fn offer(&self, o: Offer);
    /// Whole percent for the flow.
    fn progress(&self, pct: u8);
    /// Forwarded to the diagnostics log.
    fn crumb(&self, name: &str, fields: Vec<(String, Fld)>);
    /// The app quits in `ms` (the glue posts the quit).
    fn quit_in(&self, ms: u64);
    fn rss_mb(&self) -> Option<u64>;
}

/// Sleeps between checks. Production: a Condvar wait; tests: a manual one.
pub trait Sleeper: Send + Sync {
    /// Sleeps `ms`; false when stopped.
    fn sleep(&self, ms: u64) -> bool;
    /// Wakes a sleeping thread so `sleep` returns false. The default does nothing.
    fn stop(&self) {}
}

/// The production [`Sleeper`]: a timed Condvar wait that `stop` interrupts.
#[derive(Default)]
pub struct CondvarSleeper {
    stopped: Mutex<bool>,
    cv: Condvar,
}

impl CondvarSleeper {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Sleeper for CondvarSleeper {
    fn sleep(&self, ms: u64) -> bool {
        let deadline = Instant::now() + Duration::from_millis(ms);
        let mut stopped = lock(&self.stopped);
        while !*stopped {
            let now = Instant::now();
            if now >= deadline {
                return true;
            }
            stopped = self
                .cv
                .wait_timeout(stopped, deadline - now) // sync-ok: the update thread, never the UI
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        false
    }

    fn stop(&self) {
        *lock(&self.stopped) = true;
        self.cv.notify_all();
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Default)]
struct State {
    offer: Option<Offer>,
    next_token: u64,
    calls: Calls,
}

struct Inner {
    mode: Mode,
    feed: Arc<dyn Feed>,
    current: String,
    user_data_dir: Option<PathBuf>,
    events: Arc<dyn Events>,
    sleeper: Arc<dyn Sleeper>,
    waiter: Arc<dyn Waiter + Send + Sync>,
    state: Mutex<State>,
    stopped: AtomicBool,
    /// True while an install or a preview run is current (no check runs meanwhile, D2).
    running: AtomicBool,
    /// Held by a check for its whole length and by an install for its whole length, so an install
    /// never starts under a check that is already on the wire (that check would swap the feed's
    /// stored find and the offer in the middle of the run).
    checking: Mutex<()>,
}

thread_local! {
    /// The caller's pre-answer for the close question, valid only inside `Engine::install`.
    static AGREE: Cell<Option<*const (dyn Fn() + 'static)>> = const { Cell::new(None) };
}

/// Clears the thread's pre-answer when dropped.
struct AgreeScope;

impl AgreeScope {
    fn set(agree: &dyn Fn()) -> Self {
        let p: *const (dyn Fn() + '_) = agree;
        // SAFETY: only the lifetime is erased. The pointer is read on this same thread, and only
        // while the scope guard lives (it is dropped before `Engine::install` returns), so the
        // closure it points to is alive for every read.
        let p: *const (dyn Fn() + 'static) = unsafe { std::mem::transmute(p) };
        AGREE.with(|c| c.set(Some(p)));
        AgreeScope
    }
}

impl Drop for AgreeScope {
    fn drop(&mut self) {
        AGREE.with(|c| c.set(None));
    }
}

/// Flips `running` for the length of a run, also on a panic.
struct RunGuard<'a>(&'a AtomicBool);

impl<'a> RunGuard<'a> {
    fn new(flag: &'a AtomicBool) -> Self {
        flag.store(true, Ordering::SeqCst);
        RunGuard(flag)
    }
}

impl Drop for RunGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl Inner {
    fn tick(&self) {
        if self.mode != Mode::Live || self.running.load(Ordering::SeqCst) {
            return;
        }
        let _checking = match self.checking.try_lock() {
            Ok(g) => g,
            Err(TryLockError::Poisoned(p)) => p.into_inner(),
            // A check or an install holds it: this tick is not needed.
            Err(TryLockError::WouldBlock) => return,
        };
        if self.running.load(Ordering::SeqCst) {
            return;
        }
        lock(&self.state).calls.checks += 1;
        let started = Instant::now();
        let result = self.feed.check();
        let fetch_ms = started.elapsed().as_millis() as u64;
        let fetched = Instant::now();
        let mut offered = false;
        if let Ok(Some(found)) = result {
            let version = found
                .version
                .strip_prefix(['v', 'V'])
                .unwrap_or(&found.version)
                .to_string();
            let fresh = {
                let mut st = lock(&self.state);
                let already = st
                    .offer
                    .as_ref()
                    .is_some_and(|o| !o.mock && o.version == version);
                if already {
                    offered = true;
                    None
                } else {
                    let mut next = st.next_token;
                    let made = offer_from(&found, &self.current, &mut next);
                    if let Some(o) = &made {
                        st.next_token = next;
                        st.offer = Some(o.clone());
                        offered = true;
                    }
                    made
                }
            };
            if let Some(o) = fresh {
                self.events.offer(o);
            }
        }
        let offer_ms = fetched.elapsed().as_millis() as u64;
        let mut fields = vec![
            ("offered".to_string(), Fld::Bool(offered)),
            ("fetchMs".to_string(), Fld::Num(fetch_ms)),
            ("offerMs".to_string(), Fld::Num(offer_ms)),
        ];
        if let Some(rss) = self.events.rss_mb() {
            fields.push(("rssMb".to_string(), Fld::Num(rss)));
        }
        self.events.crumb("update-check", fields);
    }

    fn set_offer(&self, mut o: Offer) {
        {
            let mut st = lock(&self.state);
            st.next_token += 1;
            o.token = st.next_token;
            st.offer = Some(o.clone());
        }
        self.events.offer(o);
    }

    fn preview_if_none(&self) {
        let o = {
            let mut st = lock(&self.state);
            if st.offer.is_some() {
                return;
            }
            let o = preview_update(&self.current);
            st.offer = Some(o.clone());
            o
        };
        self.events.offer(o);
    }

    fn run_install(&self, version: &str, cancel: &CancelToken, agree: &dyn Fn()) -> bool {
        lock(&self.state).calls.installs += 1;
        let events = &self.events;
        let downloaded =
            self.feed
                .download(version, &mut |raw| events.progress(raw.min(99)), cancel);
        match downloaded {
            // An abandoned download still runs and owns its files: discard nothing of its own.
            // A cancelled download is abandoned the same way: its joiner removes its files.
            Err(FeedError::Failed(m)) if m == "busy" => return false,
            Err(FeedError::Cancelled) => return false,
            Err(_) => {
                self.feed.discard();
                return false;
            }
            Ok(()) => {}
        }
        if cancel.is_cancelled() {
            self.feed.discard();
            return false;
        }
        self.events.progress(100);
        let args = restart_args(self.user_data_dir.as_deref());
        if self.feed.apply(version, &args).is_err() {
            self.feed.discard();
            return false;
        }
        agree();
        self.events.quit_in(QUIT_AFTER_MS);
        true
    }
}

impl GateDeps for Inner {
    fn offer(&self) -> Option<Offer> {
        lock(&self.state).offer.clone()
    }

    fn preview(&self, cancelled: &dyn Fn() -> bool) -> bool {
        let _run = RunGuard::new(&self.running);
        let events = &self.events;
        run_preview_install(
            &mut |p| events.progress(p),
            PREVIEW_MS,
            &*self.waiter,
            cancelled,
        )
    }

    fn install(&self, token: u64, cancel: &CancelToken, agree: &dyn Fn()) -> bool {
        let _run = RunGuard::new(&self.running);
        // Waits for a check already in flight (on the install worker, never the UI thread).
        let _checking = lock(&self.checking); // sync-ok: install worker
        let version = match lock(&self.state).offer.clone() {
            Some(o) if o.token == token && !o.mock => o.version,
            _ => return false,
        };
        self.run_install(&version, cancel, agree)
    }

    fn agree(&self) {
        if let Some(p) = AGREE.with(Cell::get) {
            // SAFETY: set by `AgreeScope` on this thread and cleared before its closure dies.
            unsafe { (*p)() };
        }
    }
}

/// The update engine. One per app; share it as an `Arc`.
pub struct Engine {
    inner: Arc<Inner>,
    gate: Gate,
}

impl Engine {
    /// `user_data_dir`: the `--user-data-dir` the running copy was started with (None for the
    /// default profile), for the restart arguments. `waiter` drives the preview install.
    pub fn new(
        mode: Mode,
        feed: Arc<dyn Feed>,
        current: &str,
        user_data_dir: Option<PathBuf>,
        events: Arc<dyn Events>,
        sleeper: Arc<dyn Sleeper>,
        waiter: Arc<dyn Waiter + Send + Sync>,
    ) -> Arc<Engine> {
        let inner = Arc::new(Inner {
            mode,
            feed,
            current: current.to_string(),
            user_data_dir,
            events,
            sleeper,
            waiter,
            state: Mutex::new(State::default()),
            stopped: AtomicBool::new(false),
            running: AtomicBool::new(false),
            checking: Mutex::new(()),
        });
        let gate = Gate::new(inner.clone());
        Arc::new(Engine { inner, gate })
    }

    /// Preview: offers the preview once (never replacing a real offer). Live: spawns the thread
    /// `flare-update`, which checks now and then every [`CHECK_EVERY_MS`]. Off: nothing.
    pub fn start(self: &Arc<Self>) {
        match self.inner.mode {
            Mode::Off => {}
            Mode::Preview => self.inner.preview_if_none(),
            Mode::Live => {
                let inner = self.inner.clone();
                // Detached on purpose: the thread never keeps the process alive.
                let spawned = std::thread::Builder::new()
                    .name("flare-update".into())
                    .spawn(move || {
                        loop {
                            if inner.stopped.load(Ordering::SeqCst) {
                                break;
                            }
                            inner.tick();
                            if !inner.sleeper.sleep(CHECK_EVERY_MS) // sync-ok: the update thread paces its own checks, never the UI
                                || inner.stopped.load(Ordering::SeqCst)
                            {
                                break;
                            }
                        }
                    });
                if let Err(e) = spawned {
                    log::warn!("update thread did not start: {e}");
                }
            }
        }
    }

    /// One check (Live only). Returns at once while an install runs.
    pub fn tick(&self) {
        self.inner.tick();
    }

    /// The e2e seam: stores `o` under a fresh token, emits the offer, counts no check.
    pub fn set_offer(&self, o: Offer) {
        self.inner.set_offer(o);
    }

    /// The second launch with the flag: a preview offer unless a real one is on the table.
    pub fn preview_if_none(&self) {
        self.inner.preview_if_none();
    }

    pub fn offer(&self) -> Option<Offer> {
        lock(&self.inner.state).offer.clone()
    }

    /// Blocking: the glue runs it on a worker. `agree` pre-answers the close question; it is
    /// called after the installer was handed off and right before the quit is scheduled.
    pub fn install(&self, token: Option<u64>, agree: &dyn Fn()) -> bool {
        let _scope = AgreeScope::set(agree);
        self.gate.install(token)
    }

    pub fn cancel(&self) {
        self.gate.cancel();
    }

    pub fn calls(&self) -> Calls {
        lock(&self.inner.state).calls
    }

    /// Ends the check thread (it exits as soon as its sleep returns).
    pub fn stop(&self) {
        self.inner.stopped.store(true, Ordering::SeqCst);
        self.inner.sleeper.stop();
    }
}
