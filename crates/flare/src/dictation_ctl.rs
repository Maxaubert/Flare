//! The dictation controller on the UI thread (M8 Task 22).
//!
//! `flare-dictation` holds the rules (the key reducer, the controller state machine, the service) and the
//! machinery (engine, store, microphone, media, hook, link). This module is the glue between them and the
//! window, in three layers:
//!
//! - [`Dict`] owns the state machines and executes their effects. It touches the OS only through two ports:
//!   [`DictHost`] (the window: the front tab, a paste, the clock, timers, the diag log) and [`Parts`] (the
//!   machinery: microphone, engine, media, cues, hook, link). Tests stand in for both.
//! - [`RealParts`] and [`AppHost`] are the real ports. [`Parts`] are created only while dictation is on:
//!   with it off there is no engine, no store, no hook, no media worker and no link (a switch-off drops them
//!   all, and the Job Object kills the children).
//! - [`install`], [`feed_key`](DictationKeys::feed_key), [`on_blur`], [`stop_all`] and [`view`] are the call
//!   sites Task 1 pre-wired.
//!
//! WHAT RUNS WHERE. The UI thread runs the machines. Everything that waits (the microphone opening, a pass,
//! a media pause, the hook's lines, a link answer) is a worker that posts a [`DictMsg`] back; a message that
//! finds the controller busy is queued again, never lost. The link's handlers run on the link's own threads
//! and reach the UI thread through [`Bridge`], which posts a call and waits there (never the other way).
//!
//! **[seam]** `DictationKeys::feed_key` takes the front tab as a closure (`appctl.rs` holds the borrow of
//! itself while it dispatches a key, so dictation cannot ask it), and the plan's `names` stay in
//! `flare_win::names`.
#![allow(dead_code)] // the widget (Task 26) and the page (Tasks 24, 25) read what is not read yet

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flare_dictation::catalog::language_for;
use flare_dictation::controller::{
    Controller, Effect, Event, OUTSIDE, PARTIAL_EVERY_MS, Phase, Role, View,
};
use flare_dictation::cues::Cue;
use flare_dictation::engine::{Engine, EngineConfig, Reason, TranscribeRequest, TranscribeResult};
use flare_dictation::hotkey::{
    HOST_TICK_MS, KeyChord, KeyConfig, KeyEvt, KeyState, Phase as KeyPhase, reduce_key,
};
use flare_dictation::job::Job;
use flare_dictation::keyfeed::HookEvent;
use flare_dictation::keyhook::KeyHook;
use flare_dictation::link::{self, LinkClient, LinkService};
use flare_dictation::media::{MediaPause, Token};
use flare_dictation::mic::{self, Capture, CaptureFailure, DoneFn, FrameFn, MicSource};
use flare_dictation::prefs::{self, DictPrefs, read as read_prefs};
use flare_dictation::route::{RemoteCopy, SendDecision, SendTab, merge_tabs};
use flare_dictation::service::{
    AnywhereConfig, ClientHost, LivePhase, LiveView, OverlayState, Service, ServiceOpts, SvcEffect,
    SvcEvent, TimerId, dictation_answers,
};
use flare_dictation::store::{Store, default_root};
use flare_e2e_proto::RouteResult as WireRouteResult;
use flare_term::keys::KeyEvent;
use flare_win::names;
use serde_json::{Value, json};

use slint::ComponentHandle;

use crate::AppWindow;
use crate::appctl::AppCtl;
use crate::config::Args;

// ---------------------------------------------------------------------------------------------------------------------
// What the pill, the tab mark and the widget read
// ---------------------------------------------------------------------------------------------------------------------

/// What the pill and the tab mark draw. Task 23 reads it through [`view`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DictView {
    /// A recording exists (listening or transcribing): the tab mark shows.
    pub active: bool,
    /// Listening (false while idle or transcribing).
    pub listening: bool,
    /// Transcribing the clip.
    pub transcribing: bool,
    /// The session being dictated into (`OUTSIDE` for a recording from another app).
    pub session: Option<String>,
    /// The provisional words while listening.
    pub live_text: String,
    /// The speech engine is still starting.
    pub starting: bool,
    /// A short line the pill says when something needs saying.
    pub message: Option<String>,
    /// The meter, 0 to 1.
    pub level: f32,
}

thread_local! {
    static DICT: RefCell<Option<Dict>> = const { RefCell::new(None) };
    static VIEW: RefCell<DictView> = RefCell::new(DictView::default());
    static VIEW_EPOCH: Cell<u64> = const { Cell::new(0) };
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static SUBS: RefCell<Vec<flare_settings::kv::Subscription>> = const { RefCell::new(Vec::new()) };
}

/// What the pill and the mark show now.
pub fn view() -> DictView {
    VIEW.with(|v| v.borrow().clone())
}

/// Counts every change of the view, so a drawing loop redraws only when it moved.
pub fn view_epoch() -> u64 {
    VIEW_EPOCH.with(Cell::get)
}

/// Whether dictation is switched on and this window reads the key (the cheap answer the key path asks).
pub fn armed() -> bool {
    ARMED.with(Cell::get)
}

/// Whether the Dictation page is wired (Settings' find index and the page env read it). Task 24 turns it on
/// together with the page.
pub fn wired() -> bool {
    false
}

// ---------------------------------------------------------------------------------------------------------------------
// Messages from workers
// ---------------------------------------------------------------------------------------------------------------------

/// A worker's message to the UI thread.
pub enum DictMsg {
    /// A stored dictation key changed: read them again.
    Prefs,
    /// The microphone opened (or could not).
    MicDone {
        epoch: u64,
        result: Result<Box<dyn Capture>, CaptureFailure>,
    },
    /// Frames are waiting in the open microphone's accumulator.
    Frames { epoch: u64 },
    /// The engine answered the pass `id`.
    Heard { id: u64, result: TranscribeResult },
    /// A line of the keyboard hook.
    Hook(HookEvent),
    /// The media pause number `seq` finished.
    Media { seq: u64, token: Token },
    /// Every linked copy's tabs, answering the service's question.
    Tabs(RemoteTabs),
    /// A checked send finished.
    Sent(SendDecision),
    /// The link client connected to a service (or lost it).
    LinkState(bool),
    /// Words to paste into the front tab (another copy routed them back).
    PasteFront(String),
    /// A paste that found the window busy, to try again.
    Paste { session: String, text: String },
    /// A row clicked in the widget (Task 26).
    Pick(String),
    /// The widget dismissed (Task 26).
    Dismiss,
    /// A call a link thread wants run on the UI thread.
    Call(Box<dyn FnOnce(&mut Dict) + Send>),
}

/// The stored text of a dictation key, or nothing.
pub type KvFn = Rc<dyn Fn(&str) -> Option<String>>;
/// Every linked copy's tabs.
pub type RemoteTabs = Vec<(RemoteCopy, Vec<SendTab>)>;
/// Told when the link client connects to a service or loses it.
pub type StateFn = Box<dyn Fn(bool) + Send + Sync>;

/// Hands a message to the UI thread.
pub type Post = Arc<dyn Fn(DictMsg) + Send + Sync>;

/// The production post: a message hops onto the event loop and lands on the one [`Dict`].
fn make_post() -> Post {
    Arc::new(|msg: DictMsg| {
        if slint::invoke_from_event_loop(move || deliver(msg)).is_err() {
            log::debug!("dictation: event loop closed");
        }
    })
}

/// On the UI thread: runs `f` on the dictation state. `None` when there is none, or when it is busy (a
/// re-entrant moment).
fn with_dict<R>(f: impl FnOnce(&mut Dict) -> R) -> Option<R> {
    DICT.with(|cell| {
        let mut g = cell.try_borrow_mut().ok()?;
        g.as_mut().map(f)
    })
}

fn deliver(msg: DictMsg) {
    let busy = DICT.with(|cell| cell.try_borrow_mut().is_err());
    if busy {
        // A message must not be lost: it goes to the back of the queue.
        if slint::invoke_from_event_loop(move || deliver(msg)).is_err() {
            log::debug!("dictation: event loop closed");
        }
        return;
    }
    with_dict(|d| d.on_msg(msg));
}

// ---------------------------------------------------------------------------------------------------------------------
// The two ports
// ---------------------------------------------------------------------------------------------------------------------

/// A timer the controller sets by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TimerKey {
    /// The window reducer's tick while a bare-modifier press is deciding (210 ms).
    WindowKey,
    /// The 350 ms tick of partials and of the pill's message.
    Partial,
    /// The engine warm-up.
    Warm,
    /// The service's key tick.
    SvcKey,
    /// The picker's thirty seconds.
    SvcPick,
    /// The widget's hide.
    WidgetHide,
}

impl TimerKey {
    /// The name in the timer registry.
    pub fn name(self) -> &'static str {
        match self {
            TimerKey::WindowKey => "dictation.key-tick",
            TimerKey::Partial => "dictation.partial-tick",
            TimerKey::Warm => "dictation.warm",
            TimerKey::SvcKey => "dictation.svc-key-tick",
            TimerKey::SvcPick => "dictation.pick",
            TimerKey::WidgetHide => "dictation.widget-hide",
        }
    }

    fn all() -> [TimerKey; 6] {
        [
            TimerKey::WindowKey,
            TimerKey::Partial,
            TimerKey::Warm,
            TimerKey::SvcKey,
            TimerKey::SvcPick,
            TimerKey::WidgetHide,
        ]
    }
}

/// How a paste into a tab ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasteOutcome {
    Done,
    /// The tab or its session is gone.
    Gone,
    /// The window was busy: try again on the next turn.
    Busy,
}

/// The window, as dictation needs it. UI thread only.
pub trait DictHost {
    /// Milliseconds on a clock that never goes back.
    fn now_ms(&self) -> f64;
    /// The shell tab in front with a live session, if there is one.
    fn front_session(&self) -> Option<String>;
    /// Pastes `text` (already sanitised) at the cursor of the tab `session`, as the program asked.
    fn paste(&self, session: &str, text: &str) -> PasteOutcome;
    /// Writes `text` as is to a tab's shell (the checked send's bracketed paste and its Enter).
    fn write_tab(&self, tab: &str, text: &str) -> bool;
    /// This copy's tabs, as the send rules see them.
    fn send_tabs(&self) -> Vec<SendTab>;
    fn set_timer(&self, key: TimerKey, ms: u64, repeat: bool);
    fn clear_timer(&self, key: TimerKey);
    /// A bare Alt key-up was swallowed: the window menu it would open must not open.
    fn swallow_alt_up(&self);
    /// A diag crumb.
    fn crumb(&self, name: &str, fields: &[(&str, Value)]);
    /// The pill or the mark has something new to draw.
    fn view_changed(&self) {}
    /// The widget's face (Task 26 draws it); `None` hides it.
    fn widget(&self, _face: Option<&OverlayState>) {}
}

/// What dictation owns that touches the OS. Created only while dictation is on. UI thread only (a real one
/// holds threads, never shares them).
pub trait Parts {
    /// Opens the microphone off the UI thread; `done` runs on a worker.
    fn open_mic(&mut self, device: Option<String>, on_frame: FrameFn, done: DoneFn);
    fn transcribe(&mut self, req: TranscribeRequest) -> Receiver<TranscribeResult>;
    fn warm(&mut self, model: &str, language: &str) -> Receiver<TranscribeResult>;
    fn stop_engine(&mut self);
    fn pause_media(&mut self) -> Receiver<Token>;
    fn resume_media(&mut self, token: Token);
    /// Plays a token's media again. Unlike `resume_media` it outlives the parts: a pause still in flight when
    /// dictation goes off or the app quits is undone through it when it answers (parity 867).
    fn media_resumer(&self) -> Arc<dyn Fn(Token) + Send + Sync>;
    fn cue(&mut self, cue: Cue);
    /// Starts the keyboard hook; its lines arrive on `tx`. Never called under `--e2e`.
    fn hook_start(&mut self, tx: Sender<HookEvent>);
    fn hook_stop(&mut self);
    fn hook_keys(&mut self, vks: Vec<u16>);
    fn hook_mask(&mut self, on: bool);
    fn hook_pick(&mut self, on: bool);
    /// Whether this copy has a link at all (a key and a pipe).
    fn link_available(&self) -> bool;
    fn link_service(&mut self);
    fn link_client(&mut self, handlers: Arc<dyn link::ClientHandlers>, on_state: StateFn);
    fn links_stop(&mut self);
    /// Whether a link service is accepting copies.
    fn link_serving(&self) -> bool;
    fn link_tabs(&mut self, timeout: Duration) -> Receiver<RemoteTabs>;
    fn link_send(
        &mut self,
        pid: u32,
        id: &str,
        text: &str,
        timeout: Duration,
    ) -> Receiver<SendDecision>;
    fn link_paste(&mut self, pid: u32, text: &str) -> bool;
    /// The model store, for the Dictation page.
    fn store(&self) -> Option<Store> {
        None
    }
    /// Ends everything it started (children first). Called once, before the parts are dropped.
    fn shutdown(&mut self);
}

/// What Windows says runs under a tab's shell. Called on link threads.
pub trait AgentProbe: Send + Sync {
    fn agent_under(&self, tab: &str) -> Option<String>;
}

/// What the machines need to know about this run.
#[derive(Clone, Debug)]
pub struct DictOpts {
    /// Under `--e2e`: no real hook is ever started (N16).
    pub e2e: bool,
    /// This copy's process id.
    pub self_pid: u32,
    /// This copy's name on the link ("Flare", "Flare (Dev)").
    pub me_name: String,
}

/// Everything [`Dict`] is built from.
pub struct DictDeps {
    pub host: Rc<dyn DictHost>,
    pub post: Post,
    pub make_parts: Box<dyn Fn() -> Box<dyn Parts>>,
    /// The stored text of a dictation key.
    pub kv: KvFn,
    pub probe: Arc<dyn AgentProbe>,
    pub opts: DictOpts,
}

// ---------------------------------------------------------------------------------------------------------------------
// The link's half that lives on other threads
// ---------------------------------------------------------------------------------------------------------------------

/// Reaches the UI thread from a link thread: posts a call and waits there (never the other way).
///
/// `gate` is closed by the UI thread BEFORE it joins the link threads (a switch-off, a role flip, a quit), so a
/// thread waiting here lets go at once instead of holding the window for up to `UI_CALL_MS`.
struct Bridge {
    post: Post,
    probe: Arc<dyn AgentProbe>,
    gate: Arc<AtomicBool>,
    wait: Duration,
}

/// How long a link thread waits for the UI thread before it gives up and answers nothing.
const UI_CALL_MS: u64 = 3000;
/// How often a waiting link thread looks at its gate.
const UI_SLICE_MS: u64 = 20;

/// A posted call that no one has taken yet.
const CALL_PENDING: u8 = 0;
/// The UI thread is running it (the caller then waits for its answer, however long the wait has been).
const CALL_TAKEN: u8 = 1;
/// The caller gave up first: the UI thread skips it, so nothing runs outside the window the caller meant.
const CALL_CANCELLED: u8 = 2;

impl Bridge {
    fn new(post: Post, probe: Arc<dyn AgentProbe>, gate: Arc<AtomicBool>) -> Self {
        Bridge {
            post,
            probe,
            gate,
            wait: Duration::from_millis(UI_CALL_MS),
        }
    }

    fn call<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Dict) -> T + Send + 'static,
    ) -> Option<T> {
        if self.gate.load(Ordering::SeqCst) {
            return None;
        }
        let (tx, rx) = channel();
        let state = Arc::new(AtomicU8::new(CALL_PENDING));
        let theirs = Arc::clone(&state);
        (self.post)(DictMsg::Call(Box::new(move |d| {
            // A call the caller already gave up on does not run: a late write of the words or of the Enter
            // would land outside the check that was made right before it.
            if theirs
                .compare_exchange(CALL_PENDING, CALL_TAKEN, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                let _ = tx.send(f(d));
            }
        })));
        let end = Instant::now() + self.wait;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            let slice = left.min(Duration::from_millis(UI_SLICE_MS));
            // sync-ok: runs on a link or worker thread, never the UI thread, and gives up after the wait
            match rx.recv_timeout(slice) {
                Ok(v) => return Some(v),
                Err(RecvTimeoutError::Disconnected) => return None,
                Err(RecvTimeoutError::Timeout) => {
                    if left > slice && !self.gate.load(Ordering::SeqCst) {
                        continue;
                    }
                    if state
                        .compare_exchange(
                            CALL_PENDING,
                            CALL_CANCELLED,
                            Ordering::SeqCst,
                            Ordering::SeqCst,
                        )
                        .is_ok()
                    {
                        return None;
                    }
                    // The UI thread has it in hand right now, so its answer is moments away.
                    // sync-ok: runs on a link or worker thread; the UI thread is inside the call
                    return rx.recv().ok();
                }
            }
        }
    }
}
impl ClientHost for Bridge {
    fn tabs(&self) -> Vec<SendTab> {
        self.call(|d| d.host.send_tabs()).unwrap_or_default()
    }

    fn agent_under(&self, id: &str) -> Option<String> {
        self.probe.agent_under(id)
    }

    fn pause(&self, ms: u64) {
        // The wait before the Enter, in slices: a closed gate ends it (the Enter then finds no tab to go to).
        let end = Instant::now() + Duration::from_millis(ms);
        while !self.gate.load(Ordering::SeqCst) {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            // sync-ok: runs on the link's own handler thread or a send worker, never the UI thread
            std::thread::sleep(left.min(Duration::from_millis(UI_SLICE_MS)));
        }
    }

    fn write(&self, id: &str, data: &str) -> bool {
        let (id, data) = (id.to_string(), data.to_string());
        self.call(move |d| d.host.write_tab(&id, &data))
            .unwrap_or(false)
    }

    fn paste(&self, text: &str) {
        (self.post)(DictMsg::PasteFront(text.to_string()));
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Dict
// ---------------------------------------------------------------------------------------------------------------------

/// The frames a capture thread has handed over and the UI thread has not taken yet.
#[derive(Default)]
struct Acc {
    samples: Vec<f32>,
    level: f32,
    posted: bool,
}

#[derive(Default)]
struct MicState {
    /// Bumped on every open and every close, so a late answer or frame of an older open is dropped.
    epoch: u64,
    /// A recording wants the microphone (open or opening).
    wanted: bool,
    cap: Option<Box<dyn Capture>>,
    acc: Option<Arc<Mutex<Acc>>>,
}

/// A pause asked for and not answered yet.
struct PendingPause {
    /// Resume as soon as it answers.
    resume_now: bool,
    /// Set when dictation went off first: the waiting worker then undoes the pause itself (parity 867).
    abandoned: Arc<AtomicBool>,
}

#[derive(Default)]
struct MediaState {
    seq: u64,
    pending: HashMap<u64, PendingPause>,
    /// The token of the pause that is in force.
    held: Option<Token>,
    /// The workers waiting for a pause's answer (a quit waits for them).
    waiters: Vec<std::thread::JoinHandle<()>>,
}

/// The state machines and their executor. See the module comment.
pub struct Dict {
    host: Rc<dyn DictHost>,
    post: Post,
    make_parts: Box<dyn Fn() -> Box<dyn Parts>>,
    kv: KvFn,
    probe: Arc<dyn AgentProbe>,
    opts: DictOpts,
    prefs: DictPrefs,
    ctl: Controller,
    svc: Option<Service>,
    parts: Option<Box<dyn Parts>>,
    key: KeyState,
    held: HashSet<String>,
    mic: MicState,
    media: MediaState,
    /// Plays a token again after the parts are gone (a pause that answered late).
    resumer: Option<Arc<dyn Fn(Token) + Send + Sync>>,
    /// Closed before the link threads are joined; see [`Bridge`].
    gate: Arc<AtomicBool>,
    partial_on: bool,
    view: DictView,
    level: f32,
    live_sent_ms: f64,
    widget: Option<OverlayState>,
    stopped: bool,
}

/// Prism Terminal's second lock against an Enter (`pasteSpokenText`, parity 576): every run of line breaks (CR, LF,
/// U+2028, U+2029) becomes one space. An unbracketed paste would run a line at its first break.
fn one_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_break = false;
    for c in text.chars() {
        if matches!(c, '\r' | '\n' | '\u{2028}' | '\u{2029}') {
            if !in_break {
                out.push(' ');
            }
            in_break = true;
        } else {
            in_break = false;
            out.push(c);
        }
    }
    out
}

/// The level reaches the widget at most this often (N20).
const LEVEL_EVERY_MS: f64 = 66.0;

/// Flare's winit physical key names are W3C's except the Windows key, which winit calls Super.
pub fn w3c_code(code: &str) -> &str {
    match code {
        "SuperLeft" => "MetaLeft",
        "SuperRight" => "MetaRight",
        other => other,
    }
}

fn phase_of(p: Phase) -> (bool, bool) {
    (p == Phase::Listening, p == Phase::Transcribing)
}

impl Dict {
    pub fn new(d: DictDeps) -> Self {
        let prefs = read_prefs(&|_| None, &[]);
        Dict {
            host: d.host,
            post: d.post,
            make_parts: d.make_parts,
            kv: d.kv,
            probe: d.probe,
            opts: d.opts,
            ctl: Controller::new(prefs.clone()),
            prefs,
            svc: None,
            parts: None,
            key: KeyState::initial(),
            held: HashSet::new(),
            mic: MicState::default(),
            media: MediaState::default(),
            resumer: None,
            gate: Arc::new(AtomicBool::new(false)),
            partial_on: false,
            view: DictView::default(),
            level: 0.0,
            live_sent_ms: f64::MIN,
            widget: None,
            stopped: false,
        }
    }

    // ---- state the page and the probes read ----

    pub fn prefs(&self) -> &DictPrefs {
        &self.prefs
    }

    /// Whether any machinery exists (false while dictation is off).
    pub fn has_parts(&self) -> bool {
        self.parts.is_some()
    }

    pub fn role(&self) -> Role {
        self.ctl.role()
    }

    /// The widget's face now (Task 26 draws it).
    pub fn widget_face(&self) -> Option<&OverlayState> {
        self.widget.as_ref()
    }

    // ---- preferences ----

    /// Reads the stored keys and brings the machinery up or down to match.
    pub fn apply_prefs(&mut self) {
        if self.stopped {
            return;
        }
        let kv = Rc::clone(&self.kv);
        let p = read_prefs(&*kv, &[]);
        self.prefs = p.clone();
        if p.enabled {
            self.bring_up();
        }
        let fx = self.ctl_on(Event::Prefs(p.clone()));
        self.run(fx);
        self.svc_on(SvcEvent::Config(AnywhereConfig {
            on: p.enabled && p.anywhere,
            hotkey: p.hotkey.clone(),
            mode: p.mode,
        }));
        if !p.enabled {
            self.tear_down(false);
        }
        if !self.window_reads_key() {
            self.key = KeyState::initial();
            self.held.clear();
            self.host.clear_timer(TimerKey::WindowKey);
        }
        self.sync_flags();
    }

    fn bring_up(&mut self) {
        if self.parts.is_some() {
            return;
        }
        let parts = (self.make_parts)();
        let mut svc = Service::new(ServiceOpts {
            e2e: self.opts.e2e,
            link: parts.link_available(),
            self_pid: self.opts.self_pid,
        });
        let fx = svc.start();
        self.parts = Some(parts);
        self.svc = Some(svc);
        self.ctl.set_sink(true);
        self.run_svc(fx);
    }

    /// Drops the machinery.
    fn tear_down(&mut self, quit: bool) {
        // First: the link threads stop waiting on this thread, so nothing below waits for them.
        self.rotate_gate();
        for p in self.media.pending.values() {
            p.abandoned.store(true, Ordering::SeqCst);
        }
        self.svc_on(SvcEvent::Stop);
        self.svc = None;
        self.ctl.set_sink(false);
        for k in TimerKey::all() {
            self.host.clear_timer(k);
        }
        self.partial_on = false;
        self.widget = None;
        self.host.widget(None);
        self.close_mic();
        // A pause that has not answered is undone by its own worker when it does (the media is not left paused).
        let waiters = std::mem::take(&mut self.media.waiters);
        self.media.pending.clear();
        self.media.held = None;
        // The window reads the key itself again once no one else does.
        let fx = self.ctl_on(Event::Role(Role::None));
        self.run(fx);
        if let Some(mut p) = self.parts.take() {
            // Everything is stopped by now, so dropping what is left waits for nothing.
            p.shutdown();
            drop(p);
        }
        if quit {
            for w in waiters {
                // A pause still in flight answers within the media layer's own 3 s limit, and the music is
                // played again before the process goes.
                // sync-ok: quit only
                let _ = w.join();
            }
        }
    }

    /// Tells every thread that waits on the UI thread to let go, and hands out a fresh gate for the next ones.
    fn rotate_gate(&mut self) {
        self.gate.store(true, Ordering::SeqCst);
        self.gate = Arc::new(AtomicBool::new(false));
    }

    /// Quit: the machinery stops before the shells are killed.
    pub fn stop(&mut self) {
        if self.stopped {
            return;
        }
        let fx = self.ctl_on(Event::Quit);
        self.run(fx);
        self.tear_down(true);
        self.stopped = true;
        self.sync_flags();
    }

    fn window_reads_key(&self) -> bool {
        self.prefs.enabled && self.ctl.page_keys() && self.parts.is_some()
    }

    fn sync_flags(&self) {
        ARMED.with(|a| a.set(self.window_reads_key()));
    }

    // ---- the key (D2) ----

    /// A key, offered first. `front` is the shell tab in front with a live session. Returns whether the
    /// key is swallowed (it then goes nowhere else).
    pub fn on_key(&mut self, ev: &KeyEvent, up: bool, front: Option<String>) -> bool {
        if !self.window_reads_key() {
            // Another reader has the key, or dictation is off: nothing is held here either (N9).
            self.key = KeyState::initial();
            self.held.clear();
            return false;
        }
        // Only over a live shell; a press already under way still sees its release.
        if front.is_none() && self.key.phase == KeyPhase::Idle {
            return false;
        }
        let now = self.host.now_ms();
        let code = w3c_code(&ev.code).to_string();
        let repeat = if up {
            self.held.remove(&code);
            false
        } else {
            !self.held.insert(code.clone())
        };
        let fx = self.ctl_on(Event::Front(front));
        self.run(fx);
        let chord = KeyChord {
            code: code.clone(),
            ctrl: ev.ctrl,
            alt: ev.alt,
            shift: ev.shift,
            meta: ev.meta,
            repeat,
            at: now,
        };
        let evt = if up {
            KeyEvt::Up(chord)
        } else {
            KeyEvt::Down(chord)
        };
        let swallow = self.reduce(evt);
        if swallow && up && (code == "AltLeft" || code == "AltRight") {
            // The bare Alt tap would open the window menu on its way out.
            self.host.swallow_alt_up();
        }
        swallow
    }

    /// A tab has closed.
    pub fn on_tab_gone(&mut self, id: &str) {
        let fx = self.ctl_on(Event::TabGone(id.to_string()));
        self.run(fx);
    }

    /// The window lost the keyboard.
    pub fn on_blur(&mut self) {
        self.held.clear();
        if !self.window_reads_key() {
            return;
        }
        let at = self.host.now_ms();
        self.reduce(KeyEvt::Blur { at });
        let fx = self.ctl_on(Event::Blur);
        self.run(fx);
    }

    fn reduce(&mut self, evt: KeyEvt) -> bool {
        let cfg = KeyConfig {
            hotkey: self.prefs.hotkey.clone(),
            mode: self.prefs.mode,
            hold_ms: None,
        };
        let out = reduce_key(&self.key, &evt, &cfg);
        self.key = out.state;
        self.host.clear_timer(TimerKey::WindowKey);
        // The solo-hold rule needs a clock: nothing else happens while a key is simply held down.
        if self.key.phase == KeyPhase::Arming {
            self.host
                .set_timer(TimerKey::WindowKey, HOST_TICK_MS, false);
        }
        if let Some(a) = out.action {
            let fx = self.ctl_on(Event::Key(a));
            self.run(fx);
        }
        out.swallow
    }

    // ---- timers ----

    pub fn on_timer(&mut self, key: TimerKey) {
        match key {
            TimerKey::WindowKey => {
                if self.window_reads_key() {
                    let at = self.host.now_ms();
                    self.reduce(KeyEvt::Tick { at });
                }
            }
            TimerKey::Partial => {
                let fx = self.ctl_on(Event::PartialTick);
                self.run(fx);
            }
            TimerKey::Warm => {
                let model = self.prefs.model.clone();
                if !model.is_empty() && self.ctl.auto_warm() {
                    let lang = language_for(&model, &self.prefs.language);
                    if let Some(p) = self.parts.as_mut() {
                        // The answer is nobody's: the warm-up is the point.
                        drop(p.warm(&model, lang));
                    }
                }
            }
            TimerKey::SvcKey => self.svc_on(SvcEvent::Timer(TimerId::KeyTick)),
            TimerKey::SvcPick => self.svc_on(SvcEvent::Timer(TimerId::Pick)),
            TimerKey::WidgetHide => {
                self.widget = None;
                self.host.widget(None);
            }
        }
    }

    // ---- messages ----

    pub fn on_msg(&mut self, msg: DictMsg) {
        if self.stopped {
            match msg {
                DictMsg::MicDone {
                    result: Ok(cap), ..
                } => drop(cap.stop()),
                DictMsg::Media { token, .. } => self.resume_late(token),
                _ => {}
            }
            return;
        }
        match msg {
            DictMsg::Prefs => self.apply_prefs(),
            DictMsg::MicDone { epoch, result } => self.on_mic_done(epoch, result),
            DictMsg::Frames { epoch } => self.on_frames(epoch),
            DictMsg::Heard { id, result } => {
                let fx = self.ctl_on(Event::Heard { id, result });
                self.run(fx);
            }
            DictMsg::Hook(h) => self.svc_on(SvcEvent::Hook(h)),
            DictMsg::Media { seq, token } => self.on_media(seq, token),
            DictMsg::Tabs(remote) => {
                let all = self.merged_tabs(&remote);
                self.svc_on(SvcEvent::Tabs(all));
            }
            DictMsg::Sent(d) => self.svc_on(SvcEvent::Sent(d)),
            DictMsg::LinkState(on) => self.svc_on(SvcEvent::LinkState(on)),
            DictMsg::PasteFront(text) => {
                let fx = self.ctl_on(Event::Front(self.host.front_session()));
                self.run(fx);
                let fx = self.ctl_on(Event::PasteFront(text));
                self.run(fx);
            }
            DictMsg::Paste { session, text } => self.paste(&session, &text),
            DictMsg::Pick(id) => self.svc_on(SvcEvent::Pick(id)),
            DictMsg::Dismiss => self.svc_on(SvcEvent::Dismiss),
            DictMsg::Call(f) => f(self),
        }
    }

    // ---- the controller's effects ----

    /// Sends an event to the controller. The audio that has arrived is taken in first, so a stop never
    /// loses the last few milliseconds.
    fn ctl_on(&mut self, e: Event) -> Vec<Effect> {
        if matches!(
            e,
            Event::Key(_)
                | Event::Outside(_)
                | Event::PartialTick
                | Event::Blur
                | Event::Quit
                | Event::Prefs(_)
        ) {
            self.drain_frames();
        }
        let now = self.host.now_ms();
        self.ctl.on(e, now)
    }

    fn run(&mut self, fx: Vec<Effect>) {
        for e in fx {
            match e {
                Effect::OpenMic(device) => self.open_mic(device),
                Effect::CloseMic => self.close_mic(),
                Effect::Transcribe { id, req } => self.transcribe(id, req),
                Effect::Paste { session, text } => self.paste(&session, &text),
                Effect::Route(r) => self.svc_on(SvcEvent::Routed(r)),
                Effect::Cue(c) => {
                    if let Some(p) = self.parts.as_mut() {
                        p.cue(c);
                    }
                }
                Effect::PauseMedia => self.pause_media(),
                Effect::ResumeMedia => self.resume_media(),
                Effect::View(v) => self.set_view(v),
                Effect::Level(l) => self.set_level(l),
                Effect::Warm { after_ms } => {
                    self.host.clear_timer(TimerKey::Warm);
                    self.host.set_timer(TimerKey::Warm, after_ms, false);
                }
                Effect::StopEngine => {
                    self.host.clear_timer(TimerKey::Warm);
                    if let Some(p) = self.parts.as_mut() {
                        p.stop_engine();
                    }
                }
                Effect::Crumb(name) => self.host.crumb("dictation", &[("phase", json!(name))]),
            }
        }
        self.sync_partial_tick();
    }

    /// The 350 ms tick runs only while a recording listens or a message waits to expire.
    fn sync_partial_tick(&mut self) {
        let want = self.ctl.needs_tick();
        if want == self.partial_on {
            return;
        }
        self.partial_on = want;
        if want {
            self.host
                .set_timer(TimerKey::Partial, PARTIAL_EVERY_MS, true);
        } else {
            self.host.clear_timer(TimerKey::Partial);
        }
    }

    fn set_view(&mut self, v: View) {
        let (listening, transcribing) = phase_of(v.phase);
        let outside = v.session.as_deref() == Some(OUTSIDE);
        self.view = DictView {
            active: v.phase != Phase::Idle,
            listening,
            transcribing,
            session: v.session.clone(),
            live_text: v.live_text.clone(),
            starting: v.starting,
            message: v.message.clone(),
            level: if listening { self.level } else { 0.0 },
        };
        self.publish_view();
        if outside {
            self.send_live(&v, true);
        }
    }

    fn set_level(&mut self, l: f32) {
        self.level = l;
        if self.view.listening {
            self.view.level = l;
            self.publish_view();
        }
        if self.view.session.as_deref() == Some(OUTSIDE) {
            let v = self.ctl.view().clone();
            self.send_live(&v, false);
        }
    }

    /// The outside recording's face reaches the service; the level at most every 66 ms (N20).
    fn send_live(&mut self, v: &View, force: bool) {
        let now = self.host.now_ms();
        if !force && now - self.live_sent_ms < LEVEL_EVERY_MS {
            return;
        }
        self.live_sent_ms = now;
        let phase = match v.phase {
            Phase::Idle => LivePhase::Idle,
            Phase::Listening => LivePhase::Listening,
            Phase::Transcribing => LivePhase::Transcribing,
        };
        self.svc_on(SvcEvent::Live(LiveView {
            phase,
            text: v.live_text.clone(),
            level: self.level,
            starting: v.starting,
        }));
    }

    fn publish_view(&self) {
        let v = self.view.clone();
        VIEW.with(|c| *c.borrow_mut() = v);
        VIEW_EPOCH.with(|e| e.set(e.get() + 1));
        self.host.view_changed();
    }

    // ---- the microphone ----

    fn open_mic(&mut self, device: Option<String>) {
        if self.parts.is_none() {
            return;
        }
        self.mic.epoch += 1;
        self.mic.wanted = true;
        let epoch = self.mic.epoch;
        let acc = Arc::new(Mutex::new(Acc::default()));
        self.mic.acc = Some(Arc::clone(&acc));
        let post = Arc::clone(&self.post);
        let on_frame: FrameFn = Box::new(move |samples: &[f32]| {
            let mut a = acc
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            a.samples.extend_from_slice(samples);
            a.level = a.level.max(flare_dictation::wav::level(samples));
            if !a.posted {
                a.posted = true;
                drop(a);
                post(DictMsg::Frames { epoch });
            }
        });
        let post = Arc::clone(&self.post);
        let done: DoneFn = Box::new(move |result| post(DictMsg::MicDone { epoch, result }));
        if let Some(p) = self.parts.as_mut() {
            p.open_mic(device, on_frame, done);
        }
    }

    fn on_mic_done(&mut self, epoch: u64, result: Result<Box<dyn Capture>, CaptureFailure>) {
        if epoch != self.mic.epoch || !self.mic.wanted {
            // A late answer of an open nobody wants any more: let go of the microphone at once.
            if let Ok(cap) = result {
                // sync-ok: ending a capture joins its own thread, which stops within a frame (10 ms)
                drop(cap.stop());
            }
            return;
        }
        match result {
            Ok(cap) => {
                self.mic.cap = Some(cap);
                let fx = self.ctl_on(Event::MicOpened);
                self.run(fx);
            }
            Err(f) => {
                self.mic.wanted = false;
                self.mic.acc = None;
                let fx = self.ctl_on(Event::MicFailed(f));
                self.run(fx);
            }
        }
    }

    /// Takes in what the capture thread has handed over.
    fn drain_frames(&mut self) {
        let Some(acc) = self.mic.acc.clone() else {
            return;
        };
        let (samples, level) = {
            let mut a = acc
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            a.posted = false;
            (std::mem::take(&mut a.samples), std::mem::take(&mut a.level))
        };
        if samples.is_empty() || !self.mic.wanted {
            return;
        }
        let now = self.host.now_ms();
        let fx = self.ctl.on(Event::Frame { level, samples }, now);
        self.run(fx);
    }

    fn on_frames(&mut self, epoch: u64) {
        if epoch != self.mic.epoch {
            return;
        }
        self.drain_frames();
    }

    fn close_mic(&mut self) {
        self.mic.wanted = false;
        self.mic.epoch += 1;
        self.mic.acc = None;
        if let Some(cap) = self.mic.cap.take() {
            // sync-ok: ending a capture joins its own thread, which stops within a frame (10 ms)
            drop(cap.stop());
        }
    }

    // ---- the engine ----

    fn transcribe(&mut self, id: u64, req: TranscribeRequest) {
        let Some(p) = self.parts.as_mut() else {
            let fx = self.ctl_on(Event::Heard {
                id,
                result: TranscribeResult::No {
                    reason: Reason::NoEngine,
                    detail: None,
                },
            });
            self.run(fx);
            return;
        };
        let rx = p.transcribe(req);
        let post = Arc::clone(&self.post);
        let spawned = std::thread::Builder::new()
            .name("flare-dictation-wait".into())
            .spawn(move || {
                // sync-ok: a worker waits for the engine's answer; the UI thread never does
                let result = rx.recv().unwrap_or(TranscribeResult::No {
                    reason: Reason::EngineFailed,
                    detail: None,
                });
                post(DictMsg::Heard { id, result });
            });
        if spawned.is_err() {
            let fx = self.ctl_on(Event::Heard {
                id,
                result: TranscribeResult::No {
                    reason: Reason::EngineFailed,
                    detail: None,
                },
            });
            self.run(fx);
        }
    }

    // ---- the paste ----

    fn paste(&mut self, session: &str, text: &str) {
        // Typed as text: the transcript's controls and escapes never reach the shell as keystrokes.
        let clean = one_line(&flare_shells::paste::sanitize_paste(text));
        if clean.is_empty() {
            return;
        }
        match self.host.paste(session, &clean) {
            PasteOutcome::Done => {}
            PasteOutcome::Gone => {
                let fx = self.ctl_on(Event::TabGone(session.to_string()));
                self.run(fx);
            }
            PasteOutcome::Busy => (self.post)(DictMsg::Paste {
                session: session.to_string(),
                text: text.to_string(),
            }),
        }
    }

    // ---- media ----

    fn pause_media(&mut self) {
        let Some(p) = self.parts.as_mut() else {
            return;
        };
        self.media.seq += 1;
        let seq = self.media.seq;
        let abandoned = Arc::new(AtomicBool::new(false));
        self.media.pending.insert(
            seq,
            PendingPause {
                resume_now: false,
                abandoned: Arc::clone(&abandoned),
            },
        );
        let rx = p.pause_media();
        let resumer = p.media_resumer();
        self.resumer = Some(Arc::clone(&resumer));
        let post = Arc::clone(&self.post);
        let spawned = std::thread::Builder::new()
            .name("flare-dictation-media".into())
            .spawn(move || {
                // sync-ok: a worker waits for the media worker's token (bounded by its own timeout)
                let token = rx.recv().unwrap_or_default();
                if abandoned.load(Ordering::SeqCst) {
                    // Dictation went off while the pause was in flight: undo it here, nobody else will.
                    resumer(token);
                } else {
                    post(DictMsg::Media { seq, token });
                }
            });
        match spawned {
            Ok(h) => {
                self.media.waiters.retain(|w| !w.is_finished());
                self.media.waiters.push(h);
            }
            Err(_) => {
                self.media.pending.remove(&seq);
            }
        }
    }

    fn resume_media(&mut self) {
        if let Some(token) = self.media.held.take() {
            if let Some(p) = self.parts.as_mut() {
                p.resume_media(token);
            }
            return;
        }
        // The pause has not answered yet: whatever it paused is resumed the moment it does.
        if let Some((_, p)) = self.media.pending.iter_mut().max_by_key(|(seq, _)| **seq) {
            p.resume_now = true;
        }
    }

    fn on_media(&mut self, seq: u64, token: Token) {
        let Some(pending) = self.media.pending.remove(&seq) else {
            // Dictation went off after the worker posted: the parts are gone, the resumer is not.
            self.resume_late(token);
            return;
        };
        if pending.resume_now {
            match self.parts.as_mut() {
                Some(p) => p.resume_media(token),
                None => self.resume_late(token),
            }
        } else {
            self.media.held = Some(token);
        }
    }

    /// Plays a token's media again with no parts to ask.
    fn resume_late(&self, token: Token) {
        if let Some(r) = &self.resumer {
            r(token);
        }
    }
    // ---- the service's effects ----

    fn svc_on(&mut self, e: SvcEvent) {
        let now = self.host.now_ms();
        let Some(s) = self.svc.as_mut() else {
            return;
        };
        let fx = s.on(e, now);
        self.run_svc(fx);
    }

    fn merged_tabs(&self, remote: &[(RemoteCopy, Vec<SendTab>)]) -> Vec<SendTab> {
        merge_tabs(&self.host.send_tabs(), remote, &self.opts.me_name)
    }

    fn run_svc(&mut self, fx: Vec<SvcEffect>) {
        for e in fx {
            match e {
                SvcEffect::HookKeys(v) => {
                    if let Some(p) = self.parts.as_mut() {
                        p.hook_keys(v);
                    }
                }
                SvcEffect::HookMask(on) => {
                    if let Some(p) = self.parts.as_mut() {
                        p.hook_mask(on);
                    }
                }
                SvcEffect::HookPick(on) => {
                    if let Some(p) = self.parts.as_mut() {
                        p.hook_pick(on);
                    }
                }
                SvcEffect::HookStart => self.hook_start(),
                SvcEffect::HookStop => {
                    if let Some(p) = self.parts.as_mut() {
                        p.hook_stop();
                    }
                }
                SvcEffect::ToController(a) => {
                    let fx = self.ctl_on(Event::Outside(a));
                    self.run(fx);
                }
                SvcEffect::Widget(face) => {
                    self.host.clear_timer(TimerKey::WidgetHide);
                    self.host.widget(Some(&face));
                    self.widget = Some(face);
                }
                SvcEffect::WidgetHide { after_ms } => {
                    if after_ms == 0 {
                        self.host.clear_timer(TimerKey::WidgetHide);
                        self.widget = None;
                        self.host.widget(None);
                    } else {
                        self.host.set_timer(TimerKey::WidgetHide, after_ms, false);
                    }
                }
                SvcEffect::WidgetClose => {
                    self.host.clear_timer(TimerKey::WidgetHide);
                    self.widget = None;
                    self.host.widget(None);
                }
                SvcEffect::PastePage(text) => {
                    let fx = self.ctl_on(Event::Front(self.host.front_session()));
                    self.run(fx);
                    let fx = self.ctl_on(Event::PasteFront(text));
                    self.run(fx);
                }
                SvcEffect::PasteCopy { pid, text } => {
                    let ok = self
                        .parts
                        .as_mut()
                        .is_some_and(|p| p.link_paste(pid, &text));
                    self.svc_on(SvcEvent::Pasted(ok));
                }
                SvcEffect::AskTabs => self.ask_tabs(),
                SvcEffect::SendLocal { id, text } => self.send_local(id, text),
                SvcEffect::SendRemote { pid, id, text } => self.send_remote(pid, &id, &text),
                SvcEffect::Role(r) => {
                    let fx = self.ctl_on(Event::Role(r));
                    self.run(fx);
                    if !self.window_reads_key() {
                        self.key = KeyState::initial();
                        self.held.clear();
                        self.host.clear_timer(TimerKey::WindowKey);
                    }
                    self.sync_flags();
                }
                SvcEffect::StartLinkService => {
                    // A service closes the client first, and its threads may be waiting on this thread.
                    self.rotate_gate();
                    if let Some(p) = self.parts.as_mut() {
                        p.link_service();
                    }
                }
                SvcEffect::StartLinkClient => self.start_link_client(),
                SvcEffect::StopLinks => {
                    self.rotate_gate();
                    if let Some(p) = self.parts.as_mut() {
                        p.links_stop();
                    }
                }
                SvcEffect::Crumb(v) => {
                    let owned: Vec<(String, Value)> = match v {
                        Value::Object(m) => m.into_iter().collect(),
                        other => vec![("value".to_string(), other)],
                    };
                    let fields: Vec<(&str, Value)> =
                        owned.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
                    self.host.crumb("dictate-anywhere", &fields);
                }
                SvcEffect::SetTimer(id, ms) => {
                    let key = svc_timer(id);
                    self.host.clear_timer(key);
                    self.host.set_timer(key, ms, false);
                }
                SvcEffect::ClearTimer(id) => self.host.clear_timer(svc_timer(id)),
            }
        }
    }

    fn hook_start(&mut self) {
        // Never under --e2e (N16): the service does not ask, and this refuses as well.
        if self.opts.e2e {
            return;
        }
        let (tx, rx) = channel::<HookEvent>();
        let Some(p) = self.parts.as_mut() else {
            return;
        };
        p.hook_start(tx);
        let post = Arc::clone(&self.post);
        let spawned = std::thread::Builder::new()
            .name("flare-dictation-hook-lines".into())
            .spawn(move || {
                // sync-ok: a worker forwards the hook's lines; it ends when the hook lets go of the sender
                while let Ok(ev) = rx.recv() {
                    post(DictMsg::Hook(ev));
                }
            });
        if spawned.is_err() {
            log::debug!("dictation: no thread for the hook's lines");
        }
    }

    fn ask_tabs(&mut self) {
        let serving = self.parts.as_ref().is_some_and(|p| p.link_serving());
        if !serving {
            let all = self.merged_tabs(&[]);
            self.svc_on(SvcEvent::Tabs(all));
            return;
        }
        let Some(p) = self.parts.as_mut() else {
            return;
        };
        let rx = p.link_tabs(Duration::from_millis(link::TABS_MS));
        let post = Arc::clone(&self.post);
        let spawned = std::thread::Builder::new()
            .name("flare-dictation-tabs".into())
            .spawn(move || {
                // sync-ok: a worker waits for the other copies' tab lists (the link's own limit applies)
                let remote = rx.recv().unwrap_or_default();
                post(DictMsg::Tabs(remote));
            });
        if spawned.is_err() {
            let all = self.merged_tabs(&[]);
            self.svc_on(SvcEvent::Tabs(all));
        }
    }

    /// Types into one of this copy's tabs through the checked send, on a worker (it asks Windows, and waits
    /// 300 ms before the Enter).
    fn send_local(&mut self, id: String, text: String) {
        let handlers = dictation_answers(Arc::new(Bridge::new(
            Arc::clone(&self.post),
            Arc::clone(&self.probe),
            Arc::clone(&self.gate),
        )));
        let post = Arc::clone(&self.post);
        let spawned = std::thread::Builder::new()
            .name("flare-dictation-send".into())
            .spawn(move || {
                let d = handlers.send(&id, &text);
                post(DictMsg::Sent(d));
            });
        if spawned.is_err() {
            self.svc_on(SvcEvent::Sent(SendDecision::No(
                "the send could not start".into(),
            )));
        }
    }

    fn send_remote(&mut self, pid: u32, id: &str, text: &str) {
        let Some(p) = self.parts.as_mut() else {
            return;
        };
        let rx = p.link_send(pid, id, text, Duration::from_millis(link::SEND_MS));
        let post = Arc::clone(&self.post);
        let spawned = std::thread::Builder::new()
            .name("flare-dictation-send-remote".into())
            .spawn(move || {
                // sync-ok: a worker waits for the other copy's answer (the link's own limit applies)
                let got = rx.recv(); // sync-ok: a worker waits for the other copy's answer (the link's own limit applies)
                let d = got
                    .unwrap_or_else(|_| SendDecision::No("that copy of the app has closed".into()));
                post(DictMsg::Sent(d));
            });
        if spawned.is_err() {
            self.svc_on(SvcEvent::Sent(SendDecision::No(
                "the send could not start".into(),
            )));
        }
    }

    fn start_link_client(&mut self) {
        // Starting a client closes the service's threads first, and they may be waiting on this thread.
        self.rotate_gate();
        let handlers = dictation_answers(Arc::new(Bridge::new(
            Arc::clone(&self.post),
            Arc::clone(&self.probe),
            Arc::clone(&self.gate),
        )));
        let post = Arc::clone(&self.post);
        if let Some(p) = self.parts.as_mut() {
            p.link_client(handlers, Box::new(move |on| post(DictMsg::LinkState(on))));
        }
    }
}

fn svc_timer(id: TimerId) -> TimerKey {
    match id {
        TimerId::Pick => TimerKey::SvcPick,
        TimerId::KeyTick => TimerKey::SvcKey,
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// The call sites Task 1 pre-wired
// ---------------------------------------------------------------------------------------------------------------------

/// The key path's side of dictation, held by the window controller.
#[derive(Default)]
pub struct DictationKeys;

impl DictationKeys {
    /// Offers a key to dictation first (Prism Terminal's capture phase). `up` is true for a key
    /// release. `front` answers the shell tab in front with a live session. Returns whether dictation
    /// swallowed it; a swallowed key goes nowhere else.
    pub fn feed_key(&self, ev: &KeyEvent, up: bool, front: &dyn Fn() -> Option<String>) -> bool {
        if !armed() {
            return false;
        }
        let front = front();
        with_dict(|d| d.on_key(ev, up, front)).unwrap_or(false)
    }
}

/// A tab has closed: a recording that was to be pasted into it says so instead.
pub fn tab_gone(id: &str) {
    if !armed() {
        return;
    }
    with_dict(|d| d.on_tab_gone(id));
}

/// The window lost the keyboard focus.
pub fn on_blur() {
    if !armed() {
        return;
    }
    with_dict(Dict::on_blur);
}

/// Quit: stops the engine, the host, the hook and the link before the shells are killed.
pub fn stop_all() {
    with_dict(Dict::stop);
    ARMED.with(|a| a.set(false));
}

/// The model store, while dictation is on (the Dictation page reads it).
pub fn store() -> Option<Store> {
    with_dict(|d| d.parts.as_ref().and_then(|p| p.store())).flatten()
}

/// Builds the controller, the service and the widget. Nothing is built while dictation is off.
pub fn install(app: &AppWindow, ctl: &Rc<RefCell<AppCtl>>, args: &Args) {
    let inputs = match ctl.try_borrow() {
        Ok(c) => c.dictation_inputs(),
        Err(_) => return,
    };
    let prefs = inputs.prefs.clone();
    let host: Rc<dyn DictHost> = Rc::new(AppHost::new(Rc::downgrade(ctl), app.as_weak()));
    let post = make_post();
    let e2e = args.e2e;
    let exe = std::env::current_exe().unwrap_or_default(); // sync-ok: startup, before the window shows
    let cfg = RealCfg::new(e2e, &exe);
    let deps = DictDeps {
        host,
        post: Arc::clone(&post),
        make_parts: Box::new(move || Box::new(RealParts::new(&cfg))),
        kv: Rc::new(move |k| prefs.get(k)),
        probe: Arc::new(MgrProbe(inputs.mgr)),
        opts: DictOpts {
            e2e,
            self_pid: std::process::id(),
            me_name: link::copy_name(&exe),
        },
    };
    DICT.with(|c| *c.borrow_mut() = Some(Dict::new(deps)));
    let subs: Vec<flare_settings::kv::Subscription> = prefs::KEYS
        .iter()
        .map(|k| {
            let post = Arc::clone(&post);
            inputs.prefs.on_change(k, move || post(DictMsg::Prefs))
        })
        .collect();
    SUBS.with(|s| *s.borrow_mut() = subs);
    with_dict(Dict::apply_prefs);
}

/// The test hook's `DictLine`: one helper line, as if the keyboard hook had printed it. Task 27 fills it.
pub fn e2e_line(_line: &str) -> Result<(), String> {
    Err("the dictation test lines arrive with Task 27".into())
}

/// The test hook's `DictRoute`: `text` was heard with the window of process `front` in front.
pub fn e2e_route(_text: &str, _front: Option<u32>) -> Result<(), String> {
    Err("the dictation test lines arrive with Task 27".into())
}

/// The test hook's `DictRouted`: what the window hands main as heard.
pub fn e2e_routed(_result: &WireRouteResult) -> Result<(), String> {
    Err("the dictation test lines arrive with Task 27".into())
}

// ---------------------------------------------------------------------------------------------------------------------
// The real ports
// ---------------------------------------------------------------------------------------------------------------------

/// What `AppCtl` hands dictation at install.
pub struct DictInputs {
    pub prefs: flare_settings::kv::PrefStore,
    pub mgr: Arc<flare_term::session::SessionManager>,
}

/// The window, for real.
struct AppHost {
    ctl: std::rc::Weak<RefCell<AppCtl>>,
    app: slint::Weak<AppWindow>,
    t0: Instant,
    guarded: Cell<bool>,
}

impl AppHost {
    fn new(ctl: std::rc::Weak<RefCell<AppCtl>>, app: slint::Weak<AppWindow>) -> Self {
        AppHost {
            ctl,
            app,
            t0: Instant::now(),
            guarded: Cell::new(false),
        }
    }
}

impl DictHost for AppHost {
    fn now_ms(&self) -> f64 {
        self.t0.elapsed().as_secs_f64() * 1000.0
    }

    fn front_session(&self) -> Option<String> {
        let rc = self.ctl.upgrade()?;
        let c = rc.try_borrow().ok()?;
        c.dictation_front()
    }

    fn paste(&self, session: &str, text: &str) -> PasteOutcome {
        let Some(rc) = self.ctl.upgrade() else {
            return PasteOutcome::Gone;
        };
        let Ok(c) = rc.try_borrow() else {
            return PasteOutcome::Busy;
        };
        match c.handle_of(session) {
            Some(h) => {
                h.paste(text);
                PasteOutcome::Done
            }
            None => PasteOutcome::Gone,
        }
    }

    fn write_tab(&self, tab: &str, text: &str) -> bool {
        let Some(rc) = self.ctl.upgrade() else {
            return false;
        };
        let Ok(c) = rc.try_borrow() else {
            return false;
        };
        match c.handle_of(tab) {
            Some(h) => {
                h.send_text(text);
                true
            }
            None => false,
        }
    }

    fn send_tabs(&self) -> Vec<SendTab> {
        let Some(rc) = self.ctl.upgrade() else {
            return Vec::new();
        };
        match rc.try_borrow() {
            Ok(c) => c.dictation_tabs(),
            Err(_) => Vec::new(),
        }
    }

    fn set_timer(&self, key: TimerKey, ms: u64, repeat: bool) {
        crate::timer_registry::set(key.name(), ms, repeat, move || {
            with_dict(|d| d.on_timer(key));
        });
    }

    fn clear_timer(&self, key: TimerKey) {
        crate::timer_registry::clear(key.name());
    }

    fn swallow_alt_up(&self) {
        if !self.guarded.get() {
            let hwnd = self.app.upgrade().and_then(|a| crate::window::hwnd_of(&a));
            if let Some(h) = hwnd {
                self.guarded.set(flare_win::sysmenu::install_alt_guard(h));
            }
        }
        flare_win::sysmenu::swallow_next_alt_up();
    }

    fn crumb(&self, name: &str, fields: &[(&str, Value)]) {
        crate::diag_ctl::crumb(name, fields);
    }
}

/// Windows' process tree under a tab's shell.
struct MgrProbe(Arc<flare_term::session::SessionManager>);

impl AgentProbe for MgrProbe {
    fn agent_under(&self, tab: &str) -> Option<String> {
        let pid = self
            .0
            .live_pids()
            .into_iter()
            .find(|(id, _)| id == tab)
            .map(|(_, pid)| pid)?;
        let rows = flare_win::procs::snapshot_processes(&[pid], &flare_agent::detect::prefilter)?;
        let rows: Vec<flare_agent::detect::ProcRow> = rows
            .into_iter()
            .map(|e| flare_agent::detect::ProcRow {
                pid: e.pid,
                ppid: e.ppid,
                cmd: e.cmd,
            })
            .collect();
        use flare_agent::kinds::DetectedAgent;
        flare_agent::detect::tree_agent_kind(&rows, pid).map(|k| {
            match k {
                DetectedAgent::Claude => "claude",
                DetectedAgent::Codex => "codex",
                DetectedAgent::Other => "other",
            }
            .to_string()
        })
    }
}

/// How the real parts are made: the seams of an e2e run and the folders of a normal one.
#[derive(Clone)]
struct RealCfg {
    e2e: bool,
    mic: MicSource,
    root: PathBuf,
    engine_dir: PathBuf,
    host_exe: Option<PathBuf>,
    lock_dir: PathBuf,
    me: RemoteCopy,
    /// The link's pipe and key seams (`--e2e` only).
    seam_link: Option<(String, [u8; 32])>,
}

fn hex32(s: &str) -> Option<[u8; 32]> {
    let b = s.trim().as_bytes();
    if b.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, pair) in b.chunks(2).enumerate() {
        let h = std::str::from_utf8(pair).ok()?;
        out[i] = u8::from_str_radix(h, 16).ok()?;
    }
    Some(out)
}

impl RealCfg {
    fn new(e2e: bool, exe: &Path) -> Self {
        let g = crate::e2e::global();
        let mic = match g.env(names::SEAM_MIC) {
            Some(p) if !p.is_empty() => MicSource::File(PathBuf::from(p)),
            _ => MicSource::Device,
        };
        let dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
        let engine_dir = std::env::var_os(names::SEAM_WHISPER_DIR)
            .filter(|v| !v.is_empty())
            .map_or_else(|| dir.join(names::ENGINE_FOLDER), PathBuf::from);
        let host_exe = Some(dir.join(names::PARAKEET_HOST_EXE));
        let seam_link = match (g.env(names::SEAM_DICT_PIPE), g.env(names::SEAM_DICT_KEY)) {
            (Some(pipe), Some(key)) if !pipe.is_empty() => hex32(&key).map(|k| (pipe, k)),
            _ => None,
        };
        RealCfg {
            e2e,
            mic,
            root: default_root(),
            engine_dir,
            host_exe,
            lock_dir: std::env::temp_dir(),
            me: RemoteCopy {
                pid: std::process::id(),
                name: link::copy_name(exe),
            },
            seam_link,
        }
    }
}

/// The machinery, for real. Built only while dictation is on.
struct RealParts {
    cfg: RealCfg,
    store: Store,
    engine: Engine,
    media: MediaPause,
    hook: Option<KeyHook>,
    stoppers: Vec<std::thread::JoinHandle<()>>,
    link: Option<(String, [u8; 32])>,
    serving: Option<LinkService>,
    client: Option<LinkClient>,
}

impl RealParts {
    fn new(cfg: &RealCfg) -> Self {
        let store = Store::open(
            cfg.root.clone(),
            Box::new(flare_dictation::fetch_ureq::UreqFetch::new()),
        );
        let job = Job::new().unwrap_or_else(|e| {
            log::warn!("dictation: no job object ({e}); an engine will not be started");
            Job::unusable_for_tests()
        });
        let for_gpu = store.clone();
        let for_model = store.clone();
        let cpu = cfg.engine_dir.clone();
        let mut ec = EngineConfig::new(
            Box::new(move || cpu.is_dir().then(|| cpu.clone())),
            Box::new(move || for_gpu.gpu_engine_dir()),
            Box::new(move |id| for_model.model_path(id)),
        );
        // The app's engine never stands down by itself while dictation is on (N5, D11).
        ec.idle = None;
        ec.cuda_cache = Some(cfg.root.join("cuda-cache"));
        ec.parakeet_host = cfg.host_exe.clone().filter(|p| p.is_file());
        let engine = Engine::start(ec, Arc::new(job));
        // The first switch-on reads or makes the link key: one 64-byte file.
        let link = if cfg.e2e {
            cfg.seam_link.clone()
        } else {
            // sync-ok: the first switch-on reads or makes one 64-byte key file
            link::link_key(&cfg.root.join(names::DICTATION_LINK_KEY))
                .map(|k| (link::link_pipe(&k), k))
        };
        RealParts {
            cfg: cfg.clone(),
            store,
            engine,
            media: MediaPause::new(),
            hook: None,
            stoppers: Vec::new(),
            link,
            serving: None,
            client: None,
        }
    }
}

impl Parts for RealParts {
    fn open_mic(&mut self, device: Option<String>, on_frame: FrameFn, done: DoneFn) {
        mic::start_capture_async(self.cfg.mic.clone(), device, on_frame, done);
    }

    fn transcribe(&mut self, req: TranscribeRequest) -> Receiver<TranscribeResult> {
        self.engine.transcribe(req)
    }

    fn warm(&mut self, model: &str, language: &str) -> Receiver<TranscribeResult> {
        self.engine.warm(model, language)
    }

    fn stop_engine(&mut self) {
        self.engine.stop();
    }

    fn pause_media(&mut self) -> Receiver<Token> {
        self.media.pause()
    }

    fn resume_media(&mut self, token: Token) {
        self.media.resume(token);
    }

    fn media_resumer(&self) -> Arc<dyn Fn(Token) + Send + Sync> {
        let media = self.media.clone();
        Arc::new(move |t| media.resume(t))
    }

    fn cue(&mut self, cue: Cue) {
        flare_dictation::cues::play(cue);
    }

    fn hook_start(&mut self, tx: Sender<HookEvent>) {
        if self.cfg.e2e || self.hook.is_some() {
            return;
        }
        self.hook = Some(KeyHook::start(self.cfg.lock_dir.clone(), tx));
    }

    fn hook_stop(&mut self) {
        if let Some(h) = self.hook.take() {
            // The hook ends on its own thread: unhooking waits for its message pump.
            if let Ok(t) = std::thread::Builder::new()
                .name("flare-keyhook-stop".into())
                .spawn(move || h.stop())
            {
                self.stoppers.push(t);
            }
        }
    }

    fn hook_keys(&mut self, vks: Vec<u16>) {
        if let Some(h) = &self.hook {
            h.keys(vks);
        }
    }

    fn hook_mask(&mut self, on: bool) {
        if let Some(h) = &self.hook {
            h.mask(on);
        }
    }

    fn hook_pick(&mut self, on: bool) {
        if let Some(h) = &self.hook {
            h.pick(on);
        }
    }

    fn link_available(&self) -> bool {
        self.link.is_some()
    }

    fn link_service(&mut self) {
        let Some((pipe, key)) = self.link.clone() else {
            return;
        };
        // One side at a time: a service closes any client first.
        if let Some(c) = self.client.take() {
            c.close();
        }
        if self.serving.is_none() {
            self.serving = Some(LinkService::start(&pipe, key, |why| {
                log::debug!("dictation link: {why}");
            }));
        }
    }

    fn link_client(&mut self, handlers: Arc<dyn link::ClientHandlers>, on_state: StateFn) {
        let Some((pipe, key)) = self.link.clone() else {
            return;
        };
        if let Some(s) = self.serving.take() {
            s.close();
        }
        if self.client.is_none() {
            self.client = Some(LinkClient::start(
                &pipe,
                key,
                self.cfg.me.clone(),
                handlers,
                link::RETRY_MS,
                on_state,
            ));
        }
    }

    fn links_stop(&mut self) {
        if let Some(s) = self.serving.take() {
            s.close();
        }
        if let Some(c) = self.client.take() {
            c.close();
        }
    }

    fn link_serving(&self) -> bool {
        self.serving.is_some()
    }

    fn link_tabs(&mut self, timeout: Duration) -> Receiver<RemoteTabs> {
        match &self.serving {
            Some(s) => s.tabs(timeout),
            None => {
                let (tx, rx) = channel();
                let _ = tx.send(Vec::new());
                rx
            }
        }
    }

    fn link_send(
        &mut self,
        pid: u32,
        id: &str,
        text: &str,
        timeout: Duration,
    ) -> Receiver<SendDecision> {
        match &self.serving {
            Some(s) => s.send(pid, id, text, timeout),
            None => {
                let (tx, rx) = channel();
                let _ = tx.send(SendDecision::No("that copy of the app has closed".into()));
                rx
            }
        }
    }

    fn link_paste(&mut self, pid: u32, text: &str) -> bool {
        self.serving.as_ref().is_some_and(|s| s.paste(pid, text))
    }

    fn store(&self) -> Option<Store> {
        Some(self.store.clone())
    }

    fn shutdown(&mut self) {
        // Children first: the engine's server and host, then the hook and the link.
        // sync-ok: killing the engine's children, a few milliseconds each
        self.engine.stop();
        self.links_stop();
        self.hook_stop();
        for t in std::mem::take(&mut self.stoppers) {
            // sync-ok: quit and switch-off wait for the hook to let go of the lock (bounded at 2 s)
            let _ = t.join();
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use flare_dictation::hotkey::Hotkey;
    use std::collections::VecDeque;

    type Log = Rc<RefCell<Vec<String>>>;

    struct FakeHost {
        log: Log,
        now: Cell<f64>,
        front: RefCell<Option<String>>,
        timers: RefCell<HashMap<TimerKey, (u64, bool)>>,
        pastes: RefCell<Vec<(String, String)>>,
        paste_outcome: Cell<Option<PasteOutcome>>,
        alt_swallows: Cell<u32>,
        crumbs: RefCell<Vec<String>>,
        tabs: RefCell<Vec<SendTab>>,
    }

    impl FakeHost {
        fn new(log: Log) -> Rc<Self> {
            Rc::new(FakeHost {
                log,
                now: Cell::new(1000.0),
                front: RefCell::new(Some("t1".into())),
                timers: RefCell::default(),
                pastes: RefCell::default(),
                paste_outcome: Cell::new(None),
                alt_swallows: Cell::new(0),
                crumbs: RefCell::default(),
                tabs: RefCell::default(),
            })
        }
    }

    impl DictHost for FakeHost {
        fn now_ms(&self) -> f64 {
            self.now.get()
        }
        fn front_session(&self) -> Option<String> {
            self.front.borrow().clone()
        }
        fn paste(&self, session: &str, text: &str) -> PasteOutcome {
            if let Some(o) = self.paste_outcome.get() {
                return o;
            }
            self.pastes
                .borrow_mut()
                .push((session.to_string(), text.to_string()));
            PasteOutcome::Done
        }
        fn write_tab(&self, _tab: &str, _text: &str) -> bool {
            true
        }
        fn send_tabs(&self) -> Vec<SendTab> {
            self.tabs.borrow().clone()
        }
        fn set_timer(&self, key: TimerKey, ms: u64, repeat: bool) {
            self.timers.borrow_mut().insert(key, (ms, repeat));
        }
        fn clear_timer(&self, key: TimerKey) {
            self.timers.borrow_mut().remove(&key);
        }
        fn swallow_alt_up(&self) {
            self.alt_swallows.set(self.alt_swallows.get() + 1);
        }
        fn crumb(&self, name: &str, fields: &[(&str, Value)]) {
            self.crumbs.borrow_mut().push(format!("{name} {fields:?}"));
            let _ = &self.log;
        }
    }

    struct FakeCapture;
    impl Capture for FakeCapture {
        fn stop(self: Box<Self>) -> Vec<f32> {
            Vec::new()
        }
    }

    struct FakeProbe;
    impl AgentProbe for FakeProbe {
        fn agent_under(&self, _tab: &str) -> Option<String> {
            Some("claude".into())
        }
    }

    /// What the fake machinery was asked, shared with the test.
    #[derive(Default)]
    struct Shared {
        calls: RefCell<Vec<String>>,
        done: RefCell<Option<DoneFn>>,
        on_frame: RefCell<Option<FrameFn>>,
        transcribes: RefCell<Vec<(Vec<u8>, bool)>>,
        hook_tx: RefCell<Option<Sender<HookEvent>>>,
        made: Cell<u32>,
        /// A media pause that does not answer until the test sends its token.
        hold_pause: Cell<bool>,
        pause_tx: RefCell<Option<Sender<Token>>>,
        /// What the media resumer (which outlives the parts) was handed.
        late: Arc<Mutex<Vec<String>>>,
    }

    struct FakeParts {
        s: Rc<Shared>,
        link: bool,
    }

    impl FakeParts {
        fn note(&self, what: &str) {
            self.s.calls.borrow_mut().push(what.to_string());
        }
    }

    impl Parts for FakeParts {
        fn open_mic(&mut self, _d: Option<String>, on_frame: FrameFn, done: DoneFn) {
            self.note("open_mic");
            *self.s.on_frame.borrow_mut() = Some(on_frame);
            *self.s.done.borrow_mut() = Some(done);
        }
        fn transcribe(&mut self, req: TranscribeRequest) -> Receiver<TranscribeResult> {
            self.note(if req.final_ {
                "transcribe_final"
            } else {
                "transcribe_partial"
            });
            self.s.transcribes.borrow_mut().push((req.wav, req.final_));
            let (tx, rx) = channel();
            let _ = tx.send(TranscribeResult::Ok("hello there".into()));
            rx
        }
        fn warm(&mut self, _m: &str, _l: &str) -> Receiver<TranscribeResult> {
            self.note("warm");
            channel().1
        }
        fn stop_engine(&mut self) {
            self.note("stop_engine");
        }
        fn pause_media(&mut self) -> Receiver<Token> {
            self.note("pause_media");
            let (tx, rx) = channel();
            if self.s.hold_pause.get() {
                *self.s.pause_tx.borrow_mut() = Some(tx);
                return rx;
            }
            let _ = tx.send(Token(vec!["m".into()]));
            rx
        }
        fn media_resumer(&self) -> Arc<dyn Fn(Token) + Send + Sync> {
            let late = Arc::clone(&self.s.late);
            Arc::new(move |t: Token| late.lock().unwrap().push(t.0.join("|")))
        }
        fn resume_media(&mut self, _t: Token) {
            self.note("resume_media");
        }
        fn cue(&mut self, c: Cue) {
            self.note(&format!("cue_{c:?}"));
        }
        fn hook_start(&mut self, tx: Sender<HookEvent>) {
            self.note("hook_start");
            *self.s.hook_tx.borrow_mut() = Some(tx);
        }
        fn hook_stop(&mut self) {
            self.note("hook_stop");
        }
        fn hook_keys(&mut self, _v: Vec<u16>) {
            self.note("hook_keys");
        }
        fn hook_mask(&mut self, _on: bool) {
            self.note("hook_mask");
        }
        fn hook_pick(&mut self, on: bool) {
            self.note(&format!("hook_pick_{on}"));
        }
        fn link_available(&self) -> bool {
            self.link
        }
        fn link_service(&mut self) {
            self.note("link_service");
        }
        fn link_client(&mut self, _h: Arc<dyn link::ClientHandlers>, _s: StateFn) {
            self.note("link_client");
        }
        fn links_stop(&mut self) {
            self.note("links_stop");
        }
        fn link_serving(&self) -> bool {
            false
        }
        fn link_tabs(&mut self, _t: Duration) -> Receiver<RemoteTabs> {
            channel().1
        }
        fn link_send(
            &mut self,
            _p: u32,
            _i: &str,
            _t: &str,
            _d: Duration,
        ) -> Receiver<SendDecision> {
            channel().1
        }
        fn link_paste(&mut self, _p: u32, _t: &str) -> bool {
            false
        }
        fn shutdown(&mut self) {
            self.note("shutdown");
        }
    }

    struct Rig {
        d: Dict,
        host: Rc<FakeHost>,
        s: Rc<Shared>,
        store: Rc<RefCell<HashMap<String, String>>>,
        inbox: Arc<Mutex<VecDeque<DictMsg>>>,
    }

    impl Rig {
        fn new(e2e: bool) -> Rig {
            let log: Log = Rc::default();
            let host = FakeHost::new(log);
            let s: Rc<Shared> = Rc::default();
            let store: Rc<RefCell<HashMap<String, String>>> = Rc::default();
            let inbox: Arc<Mutex<VecDeque<DictMsg>>> = Arc::default();
            let q = Arc::clone(&inbox);
            let post: Post = Arc::new(move |m| q.lock().unwrap().push_back(m));
            let s2 = Rc::clone(&s);
            let st = Rc::clone(&store);
            let d = Dict::new(DictDeps {
                host: host.clone(),
                post,
                make_parts: Box::new(move || {
                    s2.made.set(s2.made.get() + 1);
                    Box::new(FakeParts {
                        s: Rc::clone(&s2),
                        link: false,
                    })
                }),
                kv: Rc::new(move |k| st.borrow().get(k).cloned()),
                probe: Arc::new(FakeProbe),
                opts: DictOpts {
                    e2e,
                    self_pid: 4242,
                    me_name: "Flare (Dev)".into(),
                },
            });
            Rig {
                d,
                host,
                s,
                store,
                inbox,
            }
        }

        fn set(&self, k: &str, v: &str) {
            self.store.borrow_mut().insert(k.into(), v.into());
        }

        fn on_with_model(e2e: bool) -> Rig {
            let mut r = Rig::new(e2e);
            r.set("flare.dictation.enabled", "1");
            r.set("flare.dictation.model", "tiny");
            r.set("flare.dictation.hotkey", r#"{"code":"AltRight"}"#);
            r.d.apply_prefs();
            r
        }

        fn calls(&self) -> Vec<String> {
            self.s.calls.borrow().clone()
        }

        fn count(&self, what: &str) -> usize {
            self.s.calls.borrow().iter().filter(|c| *c == what).count()
        }

        /// Delivers every message the workers posted, as the event loop would.
        fn pump(&mut self) {
            for _ in 0..50 {
                let m = self.inbox.lock().unwrap().pop_front();
                match m {
                    Some(m) => self.d.on_msg(m),
                    None => {
                        std::thread::sleep(Duration::from_millis(5)); // test: let a worker thread post
                        if self.inbox.lock().unwrap().is_empty() {
                            return;
                        }
                    }
                }
            }
        }

        fn at(&self, ms: f64) {
            self.host.now.set(ms);
        }

        fn key(&mut self, code: &str, up: bool) -> bool {
            let mut ev = KeyEvent::new("", code);
            ev.pressed = !up;
            ev.alt = code.starts_with("Alt");
            let front = self.host.front.borrow().clone();
            self.d.on_key(&ev, up, front)
        }
    }

    fn right_alt() -> Hotkey {
        Hotkey {
            code: "AltRight".into(),
            ctrl: false,
            alt: false,
            shift: false,
            meta: false,
        }
    }

    #[test]
    fn off_creates_nothing() {
        let mut r = Rig::new(false);
        r.d.apply_prefs();
        assert!(!r.d.has_parts());
        assert_eq!(
            r.s.made.get(),
            0,
            "the machinery is never made while dictation is off"
        );
        assert!(
            r.host.timers.borrow().is_empty(),
            "no timer runs while it is off"
        );
        assert!(!r.key("AltRight", false), "a key is not read");
        assert!(r.calls().is_empty());
        assert!(!armed() || !r.d.window_reads_key());
        // Switching it on makes it; switching it off drops it again.
        r.set("flare.dictation.enabled", "1");
        r.d.apply_prefs();
        assert!(r.d.has_parts());
        assert_eq!(r.s.made.get(), 1);
        r.set("flare.dictation.enabled", "0");
        r.d.apply_prefs();
        assert!(!r.d.has_parts());
        assert!(r.calls().contains(&"shutdown".to_string()));
        assert!(r.host.timers.borrow().is_empty());
    }

    #[test]
    fn the_key_is_read_first_and_swallowed_keys_go_nowhere_else() {
        let mut r = Rig::on_with_model(false);
        assert!(r.d.window_reads_key());
        // The bare key's down is decided later (solo rules), its up after a start is swallowed.
        r.at(1000.0);
        let down = r.key("AltRight", false);
        assert!(down, "the dictation key is swallowed on its way down");
        r.at(1210.0);
        r.d.on_timer(TimerKey::WindowKey);
        assert_eq!(
            r.count("open_mic"),
            1,
            "held alone for the hold time: the microphone opens"
        );
        // A key that is not the dictation key is not swallowed.
        assert!(!r.key("KeyA", false));
        // The release is swallowed and keeps the window menu shut.
        r.at(1500.0);
        let up = r.key("AltRight", true);
        assert!(up, "the release is swallowed");
        assert_eq!(
            r.host.alt_swallows.get(),
            1,
            "the bare Alt up asks the guard to eat the menu"
        );
    }

    #[test]
    fn only_over_a_live_shell() {
        let mut r = Rig::on_with_model(false);
        *r.host.front.borrow_mut() = None;
        r.at(1000.0);
        assert!(
            !r.key("AltRight", false),
            "over Settings or an empty window the key is nobody's"
        );
        assert!(!r.host.timers.borrow().contains_key(&TimerKey::WindowKey));
        r.at(1300.0);
        r.d.on_timer(TimerKey::WindowKey);
        assert_eq!(r.count("open_mic"), 0);
        // With a shell in front the same press starts.
        *r.host.front.borrow_mut() = Some("t1".into());
        r.at(2000.0);
        assert!(r.key("AltRight", false));
        r.at(2210.0);
        r.d.on_timer(TimerKey::WindowKey);
        assert_eq!(r.count("open_mic"), 1);
    }

    #[test]
    fn the_key_to_the_paste() {
        let mut r = Rig::on_with_model(false);
        r.at(1000.0);
        r.key("AltRight", false);
        r.at(1210.0);
        r.d.on_timer(TimerKey::WindowKey);
        assert_eq!(r.count("open_mic"), 1);
        // The microphone opens, a second of speech arrives, the key is released.
        let done = r.s.done.borrow_mut().take().expect("open_mic gave a done");
        done(Ok(Box::new(FakeCapture)));
        r.pump();
        let mut on_frame = r.s.on_frame.borrow_mut().take().expect("a frame callback");
        for _ in 0..120 {
            on_frame(&[0.1f32; 160]);
        }
        r.pump();
        r.at(2400.0);
        r.key("AltRight", true);
        assert_eq!(
            r.count("transcribe_final"),
            1,
            "the release asks for the one final pass"
        );
        r.pump();
        assert_eq!(
            *r.host.pastes.borrow(),
            [("t1".to_string(), "hello there ".to_string())],
            "the words and one space, never an Enter"
        );
        assert!(r.d.view.message.is_none());
        assert!(!r.d.view.active);
    }

    #[test]
    fn a_tab_that_closes_while_the_words_are_found_gets_nothing() {
        let mut r = Rig::on_with_model(false);
        r.at(1000.0);
        r.key("AltRight", false);
        r.at(1210.0);
        r.d.on_timer(TimerKey::WindowKey);
        let done = r.s.done.borrow_mut().take().unwrap();
        done(Ok(Box::new(FakeCapture)));
        r.pump();
        let mut on_frame = r.s.on_frame.borrow_mut().take().unwrap();
        for _ in 0..120 {
            on_frame(&[0.1f32; 160]);
        }
        r.pump();
        r.at(2400.0);
        r.key("AltRight", true);
        // The tab closes while the engine works.
        r.d.on_tab_gone("t1");
        r.pump();
        assert!(r.host.pastes.borrow().is_empty());
        assert_eq!(r.d.view.message.as_deref(), Some("That tab has closed"));
    }

    #[test]
    fn the_paste_goes_through_the_sanitiser_with_one_space() {
        let mut r = Rig::on_with_model(false);
        r.d.paste("t1", "a\u{1b}[201~b\u{7}c ");
        let pastes = r.host.pastes.borrow().clone();
        assert_eq!(pastes.len(), 1);
        assert!(
            !pastes[0].1.contains('\u{1b}'),
            "no escape reaches the shell: {:?}",
            pastes[0].1
        );
        assert!(!pastes[0].1.contains('\u{7}'));
        assert!(pastes[0].1.ends_with(' ') && !pastes[0].1.ends_with("  "));
        // A tab that is gone is told to the controller, not pasted into; a busy window is tried again.
        r.host.paste_outcome.set(Some(PasteOutcome::Gone));
        r.d.paste("t9", "x ");
        assert_eq!(r.host.pastes.borrow().len(), 1);
        r.host.paste_outcome.set(Some(PasteOutcome::Busy));
        r.d.paste("t1", "again ");
        assert!(matches!(
            r.inbox.lock().unwrap().pop_front(),
            Some(DictMsg::Paste { .. })
        ));
    }

    #[test]
    fn quit_stops_every_child_first() {
        let mut r = Rig::on_with_model(false);
        r.at(1000.0);
        r.key("AltRight", false);
        r.at(1210.0);
        r.d.on_timer(TimerKey::WindowKey);
        let done = r.s.done.borrow_mut().take().unwrap();
        done(Ok(Box::new(FakeCapture)));
        r.pump();
        r.d.stop();
        let calls = r.calls();
        let at = |what: &str| calls.iter().position(|c| c == what);
        let shutdown = at("shutdown").expect("the parts are shut down");
        for before in ["stop_engine", "hook_stop"] {
            assert!(
                at(before).is_some_and(|i| i < shutdown),
                "{before} comes before the parts are dropped: {calls:?}"
            );
        }
        assert!(!r.d.has_parts());
        assert!(r.host.timers.borrow().is_empty(), "no timer outlives it");
        assert!(!armed(), "the key path is off");
        // A second quit changes nothing.
        let n = r.calls().len();
        r.d.stop();
        assert_eq!(r.calls().len(), n);
    }

    #[test]
    fn the_e2e_never_starts_the_hook() {
        let mut r = Rig::on_with_model(true);
        r.set("flare.dictation.anywhere", "1");
        r.d.apply_prefs();
        assert_eq!(r.count("hook_start"), 0);
        assert!(r.s.hook_tx.borrow().is_none());
        // Lines still arrive through the seam: the service reads them.
        r.d.on_msg(DictMsg::Hook(HookEvent::Ready));
        assert_eq!(r.d.role(), Role::Service);
        assert_eq!(r.count("hook_start"), 0);
    }

    #[test]
    fn the_hook_never_starts_with_anywhere_off() {
        let mut r = Rig::new(false);
        r.set("flare.dictation.enabled", "1");
        r.set("flare.dictation.model", "tiny");
        r.set("flare.dictation.anywhere", "0");
        r.d.apply_prefs();
        assert_eq!(
            r.count("hook_start"),
            0,
            "dictation on, from any app off: no hook"
        );
        // On, the hook starts with the binding's keys; off again, it stops.
        r.set("flare.dictation.anywhere", "1");
        r.d.apply_prefs();
        assert_eq!(r.count("hook_start"), 1);
        assert!(r.calls().contains(&"hook_keys".to_string()));
        r.set("flare.dictation.anywhere", "0");
        r.d.apply_prefs();
        assert!(r.calls().contains(&"hook_stop".to_string()));
        // A binding with no virtual key starts none either.
        r.set("flare.dictation.anywhere", "1");
        r.set("flare.dictation.hotkey", r#"{"code":"KeyQ","ctrl":true}"#);
        let before = r.count("hook_start");
        r.d.apply_prefs();
        assert!(r.count("hook_start") >= before);
    }

    #[test]
    fn switching_off_mid_recording_pastes_nothing_and_resumes_media() {
        let mut r = Rig::on_with_model(false);
        r.set("flare.dictation.pauseMedia", "1");
        r.d.apply_prefs();
        r.at(1000.0);
        r.key("AltRight", false);
        r.at(1210.0);
        r.d.on_timer(TimerKey::WindowKey);
        assert_eq!(r.count("pause_media"), 1);
        r.pump();
        let done = r.s.done.borrow_mut().take().unwrap();
        done(Ok(Box::new(FakeCapture)));
        r.pump();
        r.set("flare.dictation.enabled", "0");
        r.d.apply_prefs();
        assert_eq!(
            r.count("resume_media"),
            1,
            "what dictation paused plays again"
        );
        assert!(r.host.pastes.borrow().is_empty());
        assert!(!r.d.has_parts());
    }

    /// Parity 576, Prism Terminal's second lock: a line break in dictated words is a space, never an Enter.
    #[test]
    fn a_line_break_in_the_words_reaches_the_shell_as_one_space() {
        let mut r = Rig::on_with_model(false);
        r.d.paste(
            "t1",
            "one\r\ntwo\nthree\u{2028}four\u{2029}\u{2029}\r\nfive ",
        );
        let pastes = r.host.pastes.borrow().clone();
        assert_eq!(pastes.len(), 1);
        assert_eq!(pastes[0].1, "one two three four five ");
        // The words routed back over the link take the same road (the controller hands them to the same paste).
        r.d.on_msg(DictMsg::PasteFront("back\nhere".into()));
        let last = r.host.pastes.borrow().last().cloned().unwrap();
        assert!(
            !last.1.contains(['\n', '\r', '\u{2028}', '\u{2029}']),
            "{last:?}"
        );
    }

    fn pause_rig() -> Rig {
        let mut r = Rig::new(false);
        r.s.hold_pause.set(true);
        r.set("flare.dictation.enabled", "1");
        r.set("flare.dictation.model", "tiny");
        r.set("flare.dictation.hotkey", r#"{"code":"AltRight"}"#);
        r.set("flare.dictation.pauseMedia", "1");
        r.d.apply_prefs();
        r.at(1000.0);
        r.key("AltRight", false);
        r.at(1210.0);
        r.d.on_timer(TimerKey::WindowKey);
        assert_eq!(r.count("pause_media"), 1, "the pause is asked for");
        r
    }

    fn answer_pause(r: &Rig) {
        let tx =
            r.s.pause_tx
                .borrow_mut()
                .take()
                .expect("a pause is waiting");
        let _ = tx.send(Token(vec!["m".into()]));
    }

    fn until_late(r: &Rig, want: &[&str]) {
        for _ in 0..400 {
            if r.s.late.lock().unwrap().len() >= want.len() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5)); // test: let the media worker answer
        }
        assert_eq!(*r.s.late.lock().unwrap(), want);
    }

    /// Parity 867: a pause that has not answered when dictation goes off still plays the music again.
    #[test]
    fn switching_off_before_the_pause_answers_still_plays_the_media_again() {
        let mut r = pause_rig();
        r.set("flare.dictation.enabled", "0");
        r.d.apply_prefs();
        assert!(!r.d.has_parts());
        answer_pause(&r);
        until_late(&r, &["m"]);
        assert_eq!(
            r.count("resume_media"),
            0,
            "the parts were gone: the late path did it"
        );
    }

    #[test]
    fn a_pause_answer_that_was_already_posted_when_it_goes_off_is_resumed_too() {
        let mut r = pause_rig();
        // The worker has posted the answer; the UI thread has not read it yet.
        answer_pause(&r);
        std::thread::sleep(Duration::from_millis(50)); // test: the media worker posts
        r.set("flare.dictation.enabled", "0");
        r.d.apply_prefs();
        r.pump();
        let n = r.s.late.lock().unwrap().len();
        assert!(n >= 1, "the token is resumed, not dropped");
    }

    #[test]
    fn quitting_before_the_pause_answers_waits_for_it_and_plays_the_media_again() {
        let mut r = pause_rig();
        let tx = r.s.pause_tx.borrow_mut().take().unwrap();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(60)); // test: the pause answers during the quit
            let _ = tx.send(Token(vec!["m".into()]));
        });
        r.d.stop();
        assert_eq!(*r.s.late.lock().unwrap(), vec!["m".to_string()]);
        t.join().unwrap();
    }

    // ---- the link threads and the UI thread ----

    fn bridge(gate: &Arc<AtomicBool>, wait_ms: u64) -> (Bridge, Arc<Mutex<VecDeque<DictMsg>>>) {
        let q: Arc<Mutex<VecDeque<DictMsg>>> = Arc::default();
        let q2 = Arc::clone(&q);
        let post: Post = Arc::new(move |m| q2.lock().unwrap().push_back(m));
        (
            Bridge {
                post,
                probe: Arc::new(FakeProbe),
                gate: Arc::clone(gate),
                wait: Duration::from_millis(wait_ms),
            },
            q,
        )
    }

    #[test]
    fn a_call_that_timed_out_never_runs_afterwards() {
        let gate = Arc::new(AtomicBool::new(false));
        let (b, q) = bridge(&gate, 40);
        let ran = Arc::new(AtomicBool::new(false));
        let r2 = Arc::clone(&ran);
        let got = b.call(move |_d| {
            r2.store(true, Ordering::SeqCst);
            1
        });
        assert_eq!(got, None, "nobody ran it in time");
        // The UI thread comes round later: the queued call must be a no-op now.
        let mut rig = Rig::new(false);
        while let Some(m) = q.lock().unwrap().pop_front() {
            rig.d.on_msg(m);
        }
        assert!(
            !ran.load(Ordering::SeqCst),
            "a write of the words or of the Enter never lands outside its window"
        );
    }

    #[test]
    fn a_closed_gate_frees_a_waiting_link_thread_at_once() {
        let gate = Arc::new(AtomicBool::new(false));
        let (b, q) = bridge(&gate, 3000);
        let g2 = Arc::clone(&gate);
        let closer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30)); // test: the UI thread begins to close the link
            g2.store(true, Ordering::SeqCst);
        });
        let began = Instant::now();
        assert_eq!(b.call(|_d| 1), None);
        assert!(
            began.elapsed() < Duration::from_millis(1500),
            "freed in {:?}, not the 3 s",
            began.elapsed()
        );
        closer.join().unwrap();
        // And a call made after the gate closed is not even posted.
        assert_eq!(b.call(|_d| 2), None);
        assert_eq!(q.lock().unwrap().len(), 1, "only the first was posted");
    }

    #[test]
    fn a_call_the_ui_thread_took_is_answered_even_if_the_wait_ran_out() {
        let gate = Arc::new(AtomicBool::new(false));
        let (b, q) = bridge(&gate, 200);
        let mut rig = Rig::new(false);
        let h = std::thread::spawn(move || {
            b.call(|_d| {
                std::thread::sleep(Duration::from_millis(400)); // test: slow inside, past the 200 ms wait
                7
            })
        });
        // The UI thread (this one) takes the call before the wait ends, then is slow inside it.
        let m = loop {
            if let Some(m) = q.lock().unwrap().pop_front() {
                break m;
            }
            std::thread::sleep(Duration::from_millis(1)); // test: the call thread posts
        };
        if let DictMsg::Call(f) = m {
            f(&mut rig.d);
        }
        assert_eq!(
            h.join().unwrap(),
            Some(7),
            "a claimed call is never answered as lost"
        );
    }

    #[test]
    fn a_link_that_stops_or_flips_closes_the_gate_the_waiting_threads_hold() {
        let mut r = Rig::on_with_model(false);
        for fx in [
            SvcEffect::StopLinks,
            SvcEffect::StartLinkService,
            SvcEffect::StartLinkClient,
        ] {
            let held = Arc::clone(&r.d.gate);
            assert!(!held.load(Ordering::SeqCst));
            r.d.run_svc(vec![fx]);
            assert!(
                held.load(Ordering::SeqCst),
                "the old threads are told before anything joins them"
            );
            assert!(
                !r.d.gate.load(Ordering::SeqCst),
                "a fresh gate for the next"
            );
        }
        let held = Arc::clone(&r.d.gate);
        r.set("flare.dictation.enabled", "0");
        r.d.apply_prefs();
        assert!(held.load(Ordering::SeqCst), "switching off closes it first");
    }

    /// Parity 834: every dictation child, the link and the key hook stop BEFORE the shells are killed. Every route
    /// out of the window (a close, a quit for an update, the end after the loop) ends in `AppCtl::finish`, which
    /// kills and waits for the shells, so the stop belongs at its top. A source scan: the order is the rule.
    #[test]
    fn quit_stops_dictation_before_finish_kills_the_shells() {
        let src = include_str!("appctl.rs");
        let body = src
            .split("    fn finish(&mut self) {")
            .nth(1)
            .and_then(|s| s.split("\n    }\n").next())
            .expect("AppCtl::finish exists");
        let stop = body
            .find("crate::dictation_ctl::stop_all()")
            .expect("finish stops dictation");
        let shells = body.find("end_of_life(").expect("finish ends the shells");
        assert!(stop < shells, "dictation stops first:\n{body}");
        let ended = body.find("self.ended = true").expect("finish is once-only");
        assert!(
            ended < stop,
            "after the once-only guard, so a second call stops nothing twice"
        );
        for route in ["fn quit_for_update(&mut self) {", "pub fn end(&mut self) {"] {
            let r = src.split(route).nth(1).expect(route);
            assert!(
                r.split("\n    }\n")
                    .next()
                    .unwrap()
                    .contains("self.finish()"),
                "{route} goes through finish"
            );
        }
    }

    /// The window controller offers every key to dictation before anything else, and a swallowed key
    /// is consumed (a source scan: the order is the rule, D2).
    #[test]
    fn the_controller_offers_dictation_the_first_look() {
        let squash = |s: &str| {
            s.lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .flat_map(str::chars)
                .filter(|c| !c.is_whitespace())
                .collect::<String>()
        };
        let src = include_str!("appctl.rs");
        let down = squash(
            src.split("pub fn on_key(&mut self, ev: &KeyEvent) -> KeyOutcome {")
                .nth(1)
                .expect("on_key"),
        );
        assert!(
            down.starts_with(
                "ifself.dictation.feed_key(ev,false,&||self.dictation_front()){returnKeyOutcome::Consumed;}"
            ),
            "dictation is asked before the chords, and a swallowed key is consumed"
        );
        let up = squash(
            src.split("pub fn on_key_up(&mut self, ev: &KeyEvent) {")
                .nth(1)
                .expect("on_key_up"),
        );
        assert!(
            up.starts_with("ifself.dictation.feed_key(ev,true,&||self.dictation_front()){return;}"),
            "a released key is offered first too"
        );
    }
    #[test]
    fn the_window_keys_are_w3c() {
        use crate::keymap::code_name;
        use slint::winit_030::winit::keyboard::{KeyCode, PhysicalKey};
        let table = [
            (KeyCode::ControlLeft, "ControlLeft"),
            (KeyCode::ControlRight, "ControlRight"),
            (KeyCode::AltLeft, "AltLeft"),
            (KeyCode::AltRight, "AltRight"),
            (KeyCode::ShiftLeft, "ShiftLeft"),
            (KeyCode::ShiftRight, "ShiftRight"),
            (KeyCode::SuperLeft, "MetaLeft"),
            (KeyCode::SuperRight, "MetaRight"),
            (KeyCode::Pause, "Pause"),
            (KeyCode::ScrollLock, "ScrollLock"),
            (KeyCode::Space, "Space"),
            (KeyCode::Backquote, "Backquote"),
            (KeyCode::Escape, "Escape"),
            (KeyCode::F1, "F1"),
            (KeyCode::F12, "F12"),
            (KeyCode::F24, "F24"),
            (KeyCode::KeyA, "KeyA"),
            (KeyCode::KeyZ, "KeyZ"),
            (KeyCode::Digit0, "Digit0"),
            (KeyCode::Digit9, "Digit9"),
        ];
        for (code, want) in table {
            let name = code_name(&PhysicalKey::Code(code));
            assert_eq!(w3c_code(&name), want, "{code:?}");
        }
        // Every key the hook can name is one the window can name the same way.
        for vk in 1u16..=255 {
            if let Some(code) = flare_dictation::keyfeed::code_of_vk(vk) {
                assert_eq!(w3c_code(&code), code, "{code} is already W3C");
            }
        }
    }

    #[test]
    fn a_mic_that_opens_after_a_cancel_is_let_go() {
        let mut r = Rig::on_with_model(false);
        r.at(1000.0);
        r.key("AltRight", false);
        r.at(1210.0);
        r.d.on_timer(TimerKey::WindowKey);
        // Escape while the microphone is still opening, then the open finishes.
        let done = r.s.done.borrow_mut().take().unwrap();
        r.d.on_blur();
        done(Ok(Box::new(FakeCapture)));
        r.pump();
        assert!(
            r.d.mic.cap.is_none(),
            "no capture is kept for a recording that is over"
        );
        assert_eq!(r.count("transcribe_final"), 0);
        let _ = right_alt();
    }
}
