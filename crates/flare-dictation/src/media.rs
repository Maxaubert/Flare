//! Media pause through the system media session manager (Task 15).
//!
//! Pause what is PLAYING while the user dictates and afterwards resume exactly that. NOT the
//! play/pause media key (it toggles, so on a machine where nothing played it would start music).
//! Windows knows what is playing (`GlobalSystemMediaTransportControlsSessionManager`, the thing
//! behind the volume flyout's media card), so this asks it, pauses only the sessions whose status
//! is Playing and hands back their ids as the token. Resume plays those ids and nothing else.
//!
//! Everything fails soft: if the worker cannot start, dictation works and the option does nothing
//! (the token is empty and resume does nothing). Nothing here panics into the caller and nothing
//! blocks the caller: `pause` returns a receiver at once. One worker thread (MTA) owns the manager;
//! it starts on the first pause.

// Off Windows the WinRT wait and the paused set wait for phase 7 of the port plan.
#![cfg_attr(not(windows), allow(dead_code, unused_macros))]
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// How long one request may take before its caller is answered with nothing.
pub const REQUEST_TIMEOUT: Duration = Duration::from_millis(3000);
/// Each await on a WinRT operation is bounded at this long, so one player that never answers
/// cannot wedge the worker every later request waits behind.
pub const AWAIT_BOUND: Duration = Duration::from_millis(2000);
/// Two unanswered requests in a row and the worker is taken to be wedged, not slow.
const TIMEOUTS_BEFORE_DROP: u32 = 2;

/// What a pause hands back: the ids of the sessions it stopped. Empty means nothing was paused.
/// An id is one whole string; a space or a line break inside it never splits it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Token(pub Vec<String>);

impl Token {
    /// True when nothing was paused.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The session list the worker talks to. The real one is Windows' manager; tests use a stand-in.
/// Both calls block (the real one for at most `AWAIT_BOUND` per session) and run on the worker.
pub trait MediaPort: Send {
    /// Pauses every session that is Playing and returns the ids of those that said they paused.
    fn pause_playing(&mut self) -> Vec<String>;
    /// Plays the sessions with these ids that are still Paused, and no others.
    fn resume(&mut self, ids: &[String]);
}

/// Builds the port ON the worker thread (COM is initialised there). `None` is a failed start.
pub type PortFactory = Box<dyn Fn() -> Option<Box<dyn MediaPort>> + Send + Sync>;

enum Req {
    Pause(Sender<Vec<String>>),
    Resume(Vec<String>),
}

struct Worker {
    tx: Sender<Req>,
    stopped: Arc<AtomicBool>,
}

impl Worker {
    fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct Inner {
    worker: Option<Worker>,
    next: u64,
    timeouts_in_a_row: u32,
    /// Callers still waiting for their token, by request number.
    waiting: HashMap<u64, Sender<Token>>,
}

/// Pauses and resumes what is playing. Cheap to clone; clones share one worker.
#[derive(Clone)]
pub struct MediaPause {
    inner: Arc<Mutex<Inner>>,
    factory: Arc<PortFactory>,
    timeout: Duration,
}

fn lock(inner: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    inner.lock().unwrap_or_else(|e| e.into_inner())
}

/// Drops empty and repeated ids: an answer is a set of whole ids.
fn clean(ids: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for id in ids {
        if !id.is_empty() && !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

impl MediaPause {
    /// The real thing, through Windows' media session manager.
    pub fn new() -> Self {
        Self::with_factory(Box::new(real_port), REQUEST_TIMEOUT)
    }

    /// With a port factory and a request timeout (tests).
    pub fn with_factory(factory: PortFactory, timeout: Duration) -> Self {
        MediaPause {
            inner: Arc::new(Mutex::new(Inner::default())),
            factory: Arc::new(factory),
            timeout,
        }
    }

    /// Starts the worker if none runs (nothing starts until the first request).
    fn ensure_worker(&self, inner: &mut Inner) -> Sender<Req> {
        if let Some(w) = &inner.worker {
            return w.tx.clone();
        }
        let (tx, rx) = channel::<Req>();
        let stopped = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stopped);
        let factory = Arc::clone(&self.factory);
        let spawned = std::thread::Builder::new()
            .name("flare-media".into())
            .spawn(move || run_worker(&factory, &rx, &flag));
        if spawned.is_err() {
            // The receiver went with the failed spawn; every send fails and the caller is answered empty.
            stopped.store(true, Ordering::SeqCst);
        }
        inner.worker = Some(Worker {
            tx: tx.clone(),
            stopped,
        });
        tx
    }

    /// Sends a request, starting (or restarting a dead) worker; false when no worker could take it.
    fn send(&self, req: Req) -> bool {
        let mut req = req;
        for _ in 0..2 {
            let tx = {
                let mut inner = lock(&self.inner);
                self.ensure_worker(&mut inner)
            };
            match tx.send(req) {
                Ok(()) => return true,
                Err(e) => {
                    req = e.0;
                    // The worker is gone (its start failed or it crashed): forget it, try a fresh one.
                    let mut inner = lock(&self.inner);
                    if let Some(w) = inner.worker.take() {
                        w.stop();
                    }
                }
            }
        }
        false
    }

    /// Pauses what is playing. The receiver gets exactly one token: the ids paused, or an empty
    /// token for every way of not getting an answer in time. Never blocks, never fails.
    pub fn pause(&self) -> Receiver<Token> {
        let (out_tx, out_rx) = channel::<Token>();
        let (res_tx, res_rx) = channel::<Vec<String>>();
        let n = {
            let mut inner = lock(&self.inner);
            inner.next += 1;
            let n = inner.next;
            inner.waiting.insert(n, out_tx);
            n
        };
        if !self.send(Req::Pause(res_tx)) {
            let _ = self.deliver(n, Token::default());
            return out_rx;
        }
        let me = self.clone();
        let spawned = std::thread::Builder::new()
            .name("flare-media-wait".into())
            .spawn(move || me.watch(n, &res_rx));
        if spawned.is_err() {
            let _ = self.deliver(n, Token::default());
        }
        out_rx
    }

    /// Waits for request `n`'s answer; answers the caller by the clock if it does not come, and
    /// undoes a pause that arrives after its caller was told nothing was paused.
    fn watch(&self, n: u64, res_rx: &Receiver<Vec<String>>) {
        match res_rx.recv_timeout(self.timeout) {
            Ok(ids) => {
                lock(&self.inner).timeouts_in_a_row = 0;
                let ids = clean(ids);
                if !self.deliver(n, Token(ids.clone())) && !ids.is_empty() {
                    // Disposed while the pause was in flight: nobody holds the token, so undo it.
                    self.undo(ids);
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                let _ = self.deliver(n, Token::default());
            }
            Err(RecvTimeoutError::Timeout) => {
                {
                    let mut inner = lock(&self.inner);
                    inner.timeouts_in_a_row += 1;
                    if inner.timeouts_in_a_row >= TIMEOUTS_BEFORE_DROP {
                        // Wedged, not slow: forget the worker; the next request starts a fresh one.
                        if let Some(w) = inner.worker.take() {
                            w.stop();
                        }
                        inner.timeouts_in_a_row = 0;
                    }
                }
                let _ = self.deliver(n, Token::default());
                // THE DANGEROUS HALF OF A TIMEOUT: the worker was only slow, the music DID stop,
                // and no token exists that would start it again. Undo it as soon as it says so.
                if let Ok(ids) = res_rx.recv() {
                    // A slow worker that answers is alive: it does not count towards a drop.
                    lock(&self.inner).timeouts_in_a_row = 0;
                    let ids = clean(ids);
                    if !ids.is_empty() {
                        self.undo(ids);
                    }
                }
            }
        }
    }

    /// Undoes a pause nobody holds a token for. Goes to the live worker if there is one; otherwise
    /// (disposed, or the worker was dropped) a one-shot port does it and is gone again, so a late
    /// answer never leaves a media worker parked while dictation is off.
    fn undo(&self, ids: Vec<String>) {
        let live = lock(&self.inner).worker.as_ref().map(|w| w.tx.clone());
        let ids = match live {
            Some(tx) => match tx.send(Req::Resume(ids)) {
                Ok(()) => return,
                Err(e) => match e.0 {
                    Req::Resume(ids) => ids,
                    Req::Pause(_) => return,
                },
            },
            None => ids,
        };
        let factory = Arc::clone(&self.factory);
        let _ = std::thread::Builder::new()
            .name("flare-media-undo".into())
            .spawn(move || {
                if let Some(mut port) = factory() {
                    port.resume(&ids);
                }
            });
    }
    /// Hands the token to caller `n`; false when that caller was already answered (by dispose).
    fn deliver(&self, n: u64, token: Token) -> bool {
        let tx = lock(&self.inner).waiting.remove(&n);
        match tx {
            Some(tx) => {
                let _ = tx.send(token);
                true
            }
            None => false,
        }
    }

    /// Plays the sessions of the token that are still paused. An empty token does nothing and
    /// starts nothing. Does not wait.
    pub fn resume(&self, token: Token) {
        let ids = clean(token.0);
        if ids.is_empty() {
            return;
        }
        let _ = self.send(Req::Resume(ids));
    }

    /// Stands the worker down and answers everybody still waiting with an empty token. Not final:
    /// a later pause starts a fresh worker.
    pub fn dispose(&self) {
        let (worker, waiting) = {
            let mut inner = lock(&self.inner);
            (inner.worker.take(), std::mem::take(&mut inner.waiting))
        };
        if let Some(w) = worker {
            w.stop();
        }
        for tx in waiting.into_values() {
            let _ = tx.send(Token::default());
        }
    }
}

impl Default for MediaPause {
    fn default() -> Self {
        Self::new()
    }
}

fn run_worker(factory: &PortFactory, rx: &Receiver<Req>, stopped: &AtomicBool) {
    let Some(mut port) = factory() else {
        return;
    };
    while let Ok(req) = rx.recv() {
        if stopped.load(Ordering::SeqCst) {
            // Stood down: a queued request is dropped (its reply sender with it).
            continue;
        }
        match req {
            Req::Pause(reply) => {
                let ids = port.pause_playing();
                let _ = reply.send(ids);
            }
            Req::Resume(ids) => port.resume(&ids),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The real port: Windows' media session manager.

#[cfg(windows)]
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
#[cfg(windows)]
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

/// `Windows.Foundation.AsyncStatus` values.
const ASYNC_STARTED: i32 = 0;
const ASYNC_COMPLETED: i32 = 1;

/// Waits for a WinRT operation for at most `AWAIT_BOUND`; cancels it and answers `None` after.
/// The operation type lives in a crate we do not name, so its three calls come in as closures.
fn await_op<T>(
    status: impl Fn() -> Option<i32>,
    cancel: impl Fn(),
    results: impl FnOnce() -> Option<T>,
) -> Option<T> {
    let step = Duration::from_millis(5);
    let mut waited = Duration::ZERO;
    loop {
        match status() {
            Some(ASYNC_COMPLETED) => return results(),
            Some(ASYNC_STARTED) => {}
            _ => return None,
        }
        if waited >= AWAIT_BOUND {
            cancel();
            return None;
        }
        std::thread::sleep(step);
        waited += step;
    }
}

/// Awaits a WinRT `IAsyncOperation` (named by the expression, not by type).
macro_rules! bounded {
    ($op:expr) => {{
        let op = $op;
        await_op(
            || op.Status().ok().map(|s| s.0),
            || {
                let _ = op.Cancel();
            },
            || op.GetResults().ok(),
        )
    }};
}

/// The sessions this module itself paused, each under a token id of its own. Two sessions can share one
/// AppUserModelId (two windows of a browser), so the id is the AUMID plus a number, and resume plays only
/// the sessions it holds: a sibling the user paused by hand is never started (audit L36).
struct PausedSet<S> {
    next: u64,
    held: Vec<(String, S)>,
}

/// Most sessions held at once; the oldest go first (a token nobody resumed).
const HELD_MAX: usize = 64;

impl<S> PausedSet<S> {
    fn new() -> Self {
        Self {
            next: 1,
            held: Vec::new(),
        }
    }

    /// Keeps `session` and returns the id its token carries.
    fn add(&mut self, aumid: &str, session: S) -> String {
        let id = format!("{aumid}#{}", self.next);
        self.next += 1;
        self.held.push((id.clone(), session));
        if self.held.len() > HELD_MAX {
            self.held.remove(0);
        }
        id
    }

    /// The sessions of exactly these ids, forgotten as they are handed back.
    fn take(&mut self, ids: &[String]) -> Vec<S> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.held.len() {
            if ids.contains(&self.held[i].0) {
                out.push(self.held.remove(i).1);
            } else {
                i += 1;
            }
        }
        out
    }
}

#[cfg(windows)]
struct RealPort {
    manager: Option<Manager>,
    paused: PausedSet<Session>,
}

#[cfg(windows)]
impl RealPort {
    /// The manager, loaded on the first request and retried on the next if that failed.
    fn sessions(&mut self) -> Vec<Session> {
        if self.manager.is_none() {
            self.manager = Manager::RequestAsync().ok().and_then(|op| bounded!(op));
        }
        let Some(m) = &self.manager else {
            return Vec::new();
        };
        m.GetSessions()
            .map(|v| v.into_iter().collect())
            .unwrap_or_default()
    }
}

#[cfg(windows)]
fn is_status(s: &Session, want: Status) -> bool {
    s.GetPlaybackInfo()
        .and_then(|i| i.PlaybackStatus())
        .is_ok_and(|st| st == want)
}

#[cfg(windows)]
fn session_id(s: &Session) -> Option<String> {
    s.SourceAppUserModelId()
        .ok()
        .map(|h| h.to_string())
        .filter(|id| !id.is_empty())
}

#[cfg(windows)]
impl MediaPort for RealPort {
    fn pause_playing(&mut self) -> Vec<String> {
        let mut found: Vec<String> = Vec::new();
        for s in self.sessions() {
            if !is_status(&s, Status::Playing) {
                continue;
            }
            // Counted only when TryPauseAsync said true: the token is a promise to start
            // something again and must not name a session that never stopped.
            let paused = s
                .TryPauseAsync()
                .ok()
                .and_then(|op| bounded!(op))
                .unwrap_or(false);
            if paused {
                if let Some(aumid) = session_id(&s) {
                    found.push(self.paused.add(&aumid, s));
                }
            }
        }
        found
    }

    fn resume(&mut self, ids: &[String]) {
        for s in self.paused.take(ids) {
            if is_status(&s, Status::Paused) {
                if let Ok(op) = s.TryPlayAsync() {
                    let _ = bounded!(op);
                }
            }
        }
    }
}

#[cfg(windows)]
fn real_port() -> Option<Box<dyn MediaPort>> {
    // SAFETY: CoInitializeEx takes no pointers we own; this thread is the worker and is made MTA.
    // A repeat call on a thread already initialised is harmless (S_FALSE) and the result is ignored.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    Some(Box::new(RealPort {
        manager: None,
        paused: PausedSet::new(),
    }))
}

/// macOS: no port. The media pause is phase 7 of the port plan, and only if a public API allows it
/// (MediaRemote is locked since macOS 15.4). Nothing is ever paused until then.
#[cfg(not(windows))]
fn real_port() -> Option<Box<dyn MediaPort>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;

    #[derive(Clone, PartialEq, Debug)]
    enum St {
        Playing,
        Paused,
        Stopped,
    }

    #[derive(Default)]
    struct World {
        sessions: Vec<(String, St)>,
        log: Vec<String>,
        /// Pause calls that have started (a gate for tests that hang the port).
        pause_calls: usize,
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Mode {
        Normal,
        /// Every pause sleeps this long (ms) before it answers.
        Delay(u64),
        /// The first pause panics (the worker dies mid-call).
        CrashOnce,
        /// Every pause blocks until the gate opens.
        Hang,
    }

    struct Fake {
        world: Arc<Mutex<World>>,
        mode: Mode,
        crashed: Arc<AtomicBool>,
        gate: Arc<AtomicBool>,
    }

    impl MediaPort for Fake {
        fn pause_playing(&mut self) -> Vec<String> {
            self.world.lock().unwrap().pause_calls += 1;
            match self.mode {
                Mode::Delay(ms) => std::thread::sleep(Duration::from_millis(ms)),
                Mode::CrashOnce if !self.crashed.swap(true, Ordering::SeqCst) => {
                    panic!("fake crash")
                }
                Mode::Hang => {
                    while !self.gate.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
                _ => {}
            }
            let mut w = self.world.lock().unwrap();
            let mut ids = Vec::new();
            for (id, st) in &mut w.sessions {
                if *st == St::Playing {
                    *st = St::Paused;
                    ids.push(id.clone());
                }
            }
            w.log.push(format!("pause {}", ids.join("|")));
            ids
        }
        fn resume(&mut self, ids: &[String]) {
            let mut w = self.world.lock().unwrap();
            for (id, st) in &mut w.sessions {
                if ids.contains(id) && *st == St::Paused {
                    *st = St::Playing;
                }
            }
            w.log.push(format!("resume {}", ids.join("|")));
        }
    }

    struct Rig {
        media: MediaPause,
        world: Arc<Mutex<World>>,
        workers: Arc<AtomicUsize>,
        gate: Arc<AtomicBool>,
    }

    fn rig(sessions: &[(&str, St)], mode: Mode, timeout_ms: u64) -> Rig {
        let world = Arc::new(Mutex::new(World {
            sessions: sessions
                .iter()
                .map(|(i, s)| ((*i).to_string(), s.clone()))
                .collect(),
            ..World::default()
        }));
        let workers = Arc::new(AtomicUsize::new(0));
        let crashed = Arc::new(AtomicBool::new(false));
        let gate = Arc::new(AtomicBool::new(false));
        let (w, n, c, g) = (world.clone(), workers.clone(), crashed, gate.clone());
        let media = MediaPause::with_factory(
            Box::new(move || {
                n.fetch_add(1, Ordering::SeqCst);
                Some(Box::new(Fake {
                    world: w.clone(),
                    mode,
                    crashed: c.clone(),
                    gate: g.clone(),
                }) as Box<dyn MediaPort>)
            }),
            Duration::from_millis(timeout_ms),
        );
        Rig {
            media,
            world,
            workers,
            gate,
        }
    }

    fn get(rx: &Receiver<Token>) -> Token {
        rx.recv_timeout(Duration::from_secs(5))
            .expect("a token within five seconds")
    }

    fn until(test: impl Fn() -> bool) {
        let end = Instant::now() + Duration::from_secs(5);
        while !test() {
            assert!(Instant::now() < end, "waited too long");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn two_sessions_with_one_aumid_resume_only_the_one_that_was_paused() {
        let mut set = PausedSet::new();
        let mine = set.add("Chrome", "window-a");
        // window-b has the same AUMID but the user paused it by hand: it was never added.
        assert_eq!(set.take(&["Chrome".to_string()]), Vec::<&str>::new());
        assert_eq!(set.take(std::slice::from_ref(&mine)), vec!["window-a"]);
        assert!(
            set.take(&[mine]).is_empty(),
            "a session is handed back once"
        );
    }

    #[test]
    fn same_aumid_twice_gets_two_ids_and_each_resumes_its_own() {
        let mut set = PausedSet::new();
        let a = set.add("Chrome", 1);
        let b = set.add("Chrome", 2);
        assert_ne!(a, b);
        assert_eq!(set.take(&[b]), vec![2]);
        assert_eq!(set.take(&[a]), vec![1]);
    }

    #[test]
    fn the_held_sessions_are_capped() {
        let mut set = PausedSet::new();
        let first = set.add("X", 0);
        for i in 1..=HELD_MAX {
            set.add("X", i);
        }
        assert!(set.take(&[first]).is_empty());
    }

    fn tok(ids: &[&str]) -> Token {
        Token(ids.iter().map(|s| (*s).to_string()).collect())
    }

    fn status(r: &Rig, id: &str) -> St {
        r.world
            .lock()
            .unwrap()
            .sessions
            .iter()
            .find(|(i, _)| i == id)
            .unwrap()
            .1
            .clone()
    }

    #[test]
    fn starts_nothing_until_the_first_pause_then_keeps_one_worker_for_every_call() {
        let r = rig(&[("spotify", St::Playing)], Mode::Normal, 3000);
        assert_eq!(r.workers.load(Ordering::SeqCst), 0);
        let t = get(&r.media.pause());
        assert_eq!(t, tok(&["spotify"]));
        r.media.resume(t);
        until(|| status(&r, "spotify") == St::Playing);
        assert_eq!(get(&r.media.pause()), tok(&["spotify"]));
        assert_eq!(r.workers.load(Ordering::SeqCst), 1);
        assert_eq!(
            r.world.lock().unwrap().log,
            ["pause spotify", "resume spotify", "pause spotify"]
        );
    }

    #[test]
    fn brings_back_only_what_was_playing_and_nothing_the_second_time() {
        let r = rig(
            &[
                ("spotify", St::Playing),
                ("chrome", St::Paused),
                ("vlc", St::Playing),
                ("edge", St::Stopped),
            ],
            Mode::Normal,
            3000,
        );
        let t = get(&r.media.pause());
        assert_eq!(t, tok(&["spotify", "vlc"]));
        assert_eq!(get(&r.media.pause()), Token::default());
        r.media.resume(t);
        until(|| status(&r, "vlc") == St::Playing);
        // The one the user had paused by hand stays paused.
        assert_eq!(status(&r, "chrome"), St::Paused);
        assert_eq!(status(&r, "edge"), St::Stopped);
    }

    #[test]
    fn resumes_exactly_the_ids_of_the_token_spaces_in_an_id_included() {
        let r = rig(
            &[
                ("Spotify AB.Music!App", St::Playing),
                ("chrome", St::Playing),
            ],
            Mode::Normal,
            3000,
        );
        let t = get(&r.media.pause());
        assert_eq!(t, tok(&["Spotify AB.Music!App", "chrome"]));
        r.media.resume(t);
        until(|| r.world.lock().unwrap().log.len() == 2);
        assert_eq!(
            r.world.lock().unwrap().log[1],
            "resume Spotify AB.Music!App|chrome"
        );
    }

    #[test]
    fn ids_with_spaces_and_a_newline_are_one_id() {
        let r = rig(&[("Some App\nline two", St::Playing)], Mode::Normal, 3000);
        let t = get(&r.media.pause());
        assert_eq!(t, tok(&["Some App\nline two"]));
        r.media.resume(t);
        until(|| r.world.lock().unwrap().log.len() == 2);
        assert_eq!(r.world.lock().unwrap().log[1], "resume Some App\nline two");
        assert_eq!(status(&r, "Some App\nline two"), St::Playing);
    }

    #[test]
    fn does_nothing_and_starts_nothing_for_an_empty_token() {
        let r = rig(&[("spotify", St::Playing)], Mode::Normal, 3000);
        r.media.resume(Token::default());
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(r.workers.load(Ordering::SeqCst), 0);
        assert!(r.world.lock().unwrap().log.is_empty());
    }

    #[test]
    fn a_token_cannot_be_made_to_say_a_second_thing() {
        // A token is data, never a request: an id with a line break in it is one id that matches
        // nothing, and no second request comes out of it.
        let r = rig(&[("spotify", St::Paused)], Mode::Normal, 3000);
        r.media.resume(tok(&["spotify\npause 9"]));
        until(|| r.world.lock().unwrap().log.len() == 1);
        assert_eq!(r.world.lock().unwrap().log, ["resume spotify\npause 9"]);
        assert_eq!(status(&r, "spotify"), St::Paused);
    }

    #[test]
    fn gives_an_empty_token_when_the_worker_does_not_answer_in_time_and_resume_still_returns() {
        let r = rig(&[("spotify", St::Playing)], Mode::Hang, 150);
        let began = Instant::now();
        assert_eq!(get(&r.media.pause()), Token::default());
        assert!(began.elapsed() < Duration::from_secs(2));
        r.media.resume(tok(&["spotify"]));
        r.gate.store(true, Ordering::SeqCst);
        r.media.dispose();
    }

    #[test]
    fn undoes_a_pause_whose_answer_arrives_after_its_caller_was_told_nothing_was_paused() {
        let r = rig(&[("spotify", St::Playing)], Mode::Delay(400), 150);
        assert_eq!(get(&r.media.pause()), Token::default());
        until(|| {
            r.world
                .lock()
                .unwrap()
                .log
                .iter()
                .any(|l| l == "resume spotify")
        });
        until(|| status(&r, "spotify") == St::Playing);
    }

    #[test]
    fn stands_a_worker_down_after_two_timeouts_in_a_row_and_starts_a_fresh_one() {
        let r = rig(&[], Mode::Hang, 100);
        let _ = get(&r.media.pause());
        let _ = get(&r.media.pause());
        // The second timeout drops the wedged worker; the next request starts another.
        until(|| lock(&r.media.inner).worker.is_none());
        let rx = r.media.pause();
        r.gate.store(true, Ordering::SeqCst);
        let _ = get(&rx);
        assert_eq!(r.workers.load(Ordering::SeqCst), 2);
        r.media.dispose();
    }

    #[test]
    fn ids_that_are_blank_or_repeated_are_not_answers() {
        struct Noisy;
        impl MediaPort for Noisy {
            fn pause_playing(&mut self) -> Vec<String> {
                vec![String::new(), "a".into(), "a".into(), String::new()]
            }
            fn resume(&mut self, _: &[String]) {}
        }
        let m = MediaPause::with_factory(
            Box::new(|| Some(Box::new(Noisy) as Box<dyn MediaPort>)),
            Duration::from_secs(3),
        );
        assert_eq!(get(&m.pause()), tok(&["a"]));
        m.resume(tok(&["", ""]));
        m.dispose();
    }

    #[test]
    fn matches_an_answer_to_its_call_not_to_the_order_they_come_back_in() {
        // Two pauses in flight together: the first request takes the playing session, the second
        // finds nothing left; each caller gets the answer of ITS request.
        let r = rig(&[("spotify", St::Playing)], Mode::Delay(100), 3000);
        let (a, b) = (r.media.pause(), r.media.pause());
        assert_eq!(get(&b), Token::default());
        assert_eq!(get(&a), tok(&["spotify"]));
    }

    #[test]
    fn answers_empty_when_the_worker_dies_mid_call_and_starts_another_on_the_next_pause() {
        let r = rig(&[("spotify", St::Playing)], Mode::CrashOnce, 3000);
        let began = Instant::now();
        assert_eq!(get(&r.media.pause()), Token::default());
        // The crash is what ended the call, not the three-second clock.
        assert!(began.elapsed() < Duration::from_millis(2500));
        assert_eq!(get(&r.media.pause()), tok(&["spotify"]));
        assert_eq!(r.workers.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn never_fails_when_the_worker_cannot_be_started_at_all() {
        let m = MediaPause::with_factory(Box::new(|| None), Duration::from_millis(500));
        assert_eq!(get(&m.pause()), Token::default());
        m.resume(tok(&["spotify"]));
        m.dispose();
    }

    #[test]
    fn never_fails_when_the_factory_itself_panics() {
        let m = MediaPause::with_factory(Box::new(|| panic!("no")), Duration::from_millis(500));
        assert_eq!(get(&m.pause()), Token::default());
        m.resume(tok(&["spotify"]));
        m.dispose();
    }

    #[test]
    fn dispose_answers_whoever_was_waiting_and_leaves_no_worker() {
        let r = rig(&[("spotify", St::Playing)], Mode::Hang, 3000);
        let rx = r.media.pause();
        until(|| r.world.lock().unwrap().pause_calls == 1);
        r.media.dispose();
        assert_eq!(get(&rx), Token::default());
        assert!(lock(&r.media.inner).worker.is_none());
        r.gate.store(true, Ordering::SeqCst);
        // Disposing twice, or with nothing running, is nothing.
        r.media.dispose();
    }

    #[test]
    fn can_be_used_again_after_dispose_the_next_pause_starts_a_fresh_worker() {
        let r = rig(&[("spotify", St::Playing)], Mode::Normal, 3000);
        assert_eq!(get(&r.media.pause()), tok(&["spotify"]));
        r.media.dispose();
        r.media.resume(tok(&["spotify"]));
        until(|| status(&r, "spotify") == St::Playing);
        assert_eq!(get(&r.media.pause()), tok(&["spotify"]));
        assert_eq!(r.workers.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_late_answer_after_dispose_is_undone_without_leaving_a_worker_behind() {
        let r = rig(&[("spotify", St::Playing)], Mode::Hang, 3000);
        let rx = r.media.pause();
        until(|| r.world.lock().unwrap().pause_calls == 1);
        r.media.dispose();
        assert_eq!(get(&rx), Token::default());
        r.gate.store(true, Ordering::SeqCst);
        // The pause lands after dispose: it is undone, and dictation being off means no worker.
        until(|| status(&r, "spotify") == St::Playing);
        assert!(lock(&r.media.inner).worker.is_none());
        std::thread::sleep(Duration::from_millis(100));
        assert!(lock(&r.media.inner).worker.is_none());
    }

    #[test]
    fn a_late_answer_after_the_worker_was_dropped_leaves_no_new_worker() {
        let r = rig(&[("spotify", St::Playing)], Mode::Delay(500), 100);
        let _ = get(&r.media.pause());
        let _ = get(&r.media.pause());
        until(|| lock(&r.media.inner).worker.is_none());
        until(|| status(&r, "spotify") == St::Playing && r.world.lock().unwrap().log.len() >= 2);
        std::thread::sleep(Duration::from_millis(100));
        assert!(lock(&r.media.inner).worker.is_none());
    }

    #[test]
    fn a_slow_but_alive_worker_is_not_dropped_when_its_late_answer_comes() {
        // Cold first pause: slower than the clock, answers late. The next slow pause must not count
        // as the second timeout in a row, or a merely slow worker is replaced by a second one.
        let r = rig(&[("spotify", St::Playing)], Mode::Delay(400), 150);
        assert_eq!(get(&r.media.pause()), Token::default());
        until(|| {
            r.world
                .lock()
                .unwrap()
                .log
                .iter()
                .any(|l| l == "resume spotify")
        });
        assert_eq!(get(&r.media.pause()), Token::default());
        assert!(lock(&r.media.inner).worker.is_some());
        assert_eq!(r.workers.load(Ordering::SeqCst), 1);
        until(|| status(&r, "spotify") == St::Playing);
        r.media.dispose();
    }

    #[test]
    fn never_sends_the_play_pause_key() {
        // The key toggles: on a machine where nothing played it would START music. No file in the
        // crate may name a media-key VK; only this file may not synthesise keys at all (the
        // keyboard hook has its own file and its own use of key injection).
        // Names are joined so this file passes.
        let media_vk = ["VK_MEDIA", "_"].concat();
        let synth = [["Send", "Input"].concat(), ["keybd", "_event"].concat()];
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![src];
        let mut scanned = 0;
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir)
                .expect("read the source folder")
                .flatten()
            {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).expect("read a source file");
                    assert!(
                        !text.contains(media_vk.as_str()),
                        "{} names a media-key VK",
                        path.display()
                    );
                    if path.file_name().is_some_and(|n| n == "media.rs") {
                        for word in &synth {
                            assert!(
                                !text.contains(word.as_str()),
                                "{} names {word}",
                                path.display()
                            );
                        }
                    }
                    scanned += 1;
                }
            }
        }
        assert!(scanned > 10, "the scan saw only {scanned} files");
    }
}
