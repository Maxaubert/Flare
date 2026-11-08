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
//!   with it off there is no engine, no hook, no media worker and no link (a switch-off drops them all, and the
//!   Job Object kills the children). The model store is the exception: [install] makes it at startup so the
//!   page's model and GPU cards work while dictation is off (F2).
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
    Controller, Effect, Event, OUTSIDE, PARTIAL_EVERY_MS, Phase, Role, RouteResult, View,
};
use flare_dictation::cues::Cue;
use flare_dictation::engine::{Engine, EngineConfig, Reason, TranscribeRequest, TranscribeResult};
use flare_dictation::hotkey::{
    HOST_TICK_MS, Hotkey, KeyChord, KeyConfig, KeyEvt, KeyState, Phase as KeyPhase,
    parse_hotkey_from_event, reduce_key, usable_hotkey,
};
use flare_dictation::job::Job;
use flare_dictation::keyfeed::HookEvent;
use flare_dictation::keyhook::KeyHook;
use flare_dictation::link::{self, LinkClient, LinkService};
use flare_dictation::media::{MediaPause, Token};
use flare_dictation::mic::{self, Capture, CaptureFailure, DoneFn, FrameFn, LostFn, MicSource};
use flare_dictation::prefs::{self, DictPrefs, read as read_prefs};
use flare_dictation::route::{RemoteCopy, SendDecision, SendTab, merge_tabs};
use flare_dictation::service::{
    AnywhereConfig, ClientHost, LivePhase, LiveView, OverlayState, Service, ServiceOpts, SvcEffect,
    SvcEvent, TimerId, Tone, dictation_answers,
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

/// The pill's face as the window's dict-phase counts it: 0 nothing (no pill in the tree), 1 listening,
/// 2 transcribing, 3 a message. Transcribing wins over listening.
pub fn pill_phase(v: &DictView) -> i32 {
    if v.transcribing {
        2
    } else if v.listening {
        1
    } else if v.message.is_some() {
        3
    } else {
        0
    }
}

/// Feeds the pill mounted over the terminal area from the view (M8 Task 23's Appendix D call site).
fn push_pill(app: &AppWindow, v: &DictView) {
    app.set_dict_phase(pill_phase(v));
    app.set_dict_starting(v.starting);
    app.set_dict_live_text(v.live_text.as_str().into());
    app.set_dict_message(v.message.clone().unwrap_or_default().as_str().into());
    app.set_dict_level(v.level);
}

/// Counts every change of the view, so a drawing loop redraws only when it moved.
pub fn view_epoch() -> u64 {
    VIEW_EPOCH.with(Cell::get)
}

/// Whether dictation is switched on and this window reads the key (the cheap answer the key path asks).
pub fn armed() -> bool {
    ARMED.with(Cell::get)
}

/// The stored dictation preferences, as the Dictation page reads them (no key is reserved for the app here).
pub fn stored_prefs(kv: &dyn Fn(&str) -> Option<String>) -> DictPrefs {
    read_prefs(kv, &[])
}

/// Whether the Dictation page is wired (Settings' find index and the page env read it). M8 Task 24 turned it on
/// together with the page model.
pub fn wired() -> bool {
    true
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
    /// The open microphone's device went away mid-recording (audit L37).
    MicLost { epoch: u64, why: CaptureFailure },
    /// The engine answered the pass `id`.
    Heard { id: u64, result: TranscribeResult },
    /// A line of the keyboard hook.
    Hook(HookEvent),
    /// The media pause number `seq` finished.
    Media { seq: u64, token: Token },
    /// Every linked copy's tabs, answering the service's question.
    Tabs(RemoteTabs),
    /// A checked send finished, with the words it carried (so a refusal can keep them whatever else is in flight).
    Sent(SendDecision, String),
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
    /// Words kept on the clipboard: whether the copy took. A failure puts `fallback` back on the widget while
    /// `shown` is still on it; with no `fallback` it says the words could not be copied.
    Copied {
        ok: bool,
        shown: Option<OverlayState>,
        fallback: Option<OverlayState>,
    },
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
    /// A hook that died is armed again (N15).
    SvcRearm,
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
            TimerKey::SvcRearm => "dictation.hook-rearm",
        }
    }

    fn all() -> [TimerKey; 7] {
        [
            TimerKey::WindowKey,
            TimerKey::Partial,
            TimerKey::Warm,
            TimerKey::SvcKey,
            TimerKey::SvcPick,
            TimerKey::WidgetHide,
            TimerKey::SvcRearm,
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
    /// Whether Claude's prompt box in this tab holds unsent text, read off its screen. Unknown is `false`.
    fn prompt_has_draft(&self, _tab: &str) -> bool {
        false
    }
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
    /// Puts words on the clipboard, on a worker; `done(true)` only when it took the exact text. The default
    /// refuses.
    fn copy_text(&self, _text: String, done: Box<dyn FnOnce(bool) + Send>) {
        done(false);
    }
}

/// What dictation owns that touches the OS. Created only while dictation is on. UI thread only (a real one
/// holds threads, never shares them).
pub trait Parts {
    /// Opens the microphone off the UI thread; `done` runs on a worker.
    fn open_mic(&mut self, device: Option<String>, on_frame: FrameFn, done: DoneFn, lost: LostFn);
    fn transcribe(&mut self, req: TranscribeRequest) -> Receiver<TranscribeResult>;
    fn warm(&mut self, model: &str, language: &str) -> Receiver<TranscribeResult>;
    /// Links in the models the neighbour apps already have (O14, Q9). Asked once per switch-on; the real parts run it on a worker.
    fn reuse_neighbour_models(&mut self) {}
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
    fn hook_rec(&mut self, on: bool);
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
    /// The model store, for the Dictation page. It is the process's one store, so it outlives these parts.
    fn store(&self) -> Option<Store> {
        None
    }
    /// The GPU engine could not start and the CPU answers for the life of the engine (the GPU row's note).
    fn gpu_fell_back(&self) -> bool {
        false
    }
    /// Whether an engine is up and the kind of the last one (the e2e probes read it; nothing else does).
    fn engine_state(&self) -> (bool, Option<flare_dictation::engine::EngineKind>) {
        (false, None)
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
/// What a failed copy of kept words says when nothing else is on screen.
const COPY_FAILED: &str = "Words could not be copied";
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

    fn prompt_has_draft(&self, id: &str) -> bool {
        let id = id.to_string();
        self.call(move |d| d.host.prompt_has_draft(&id))
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
    /// The device went away before the open's answer was taken in: applied the moment it is.
    lost_early: Option<CaptureFailure>,
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
    /// The tab in front when the outside recording began: its words are pasted there (`SvcEffect::NoteFront`).
    rec_session: Option<String>,
    /// The last paste found its tab gone.
    paste_gone: bool,
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
            rec_session: None,
            paste_gone: false,
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
        let mut parts = (self.make_parts)();
        parts.reuse_neighbour_models();
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
        // A recording whose microphone is still opening (audit L43): the controller waits for the open to finish
        // the cancel it was given, but the answer is dropped once the machinery is gone. Tell it the open is over
        // so it ends the recording and resumes the media; the late capture is let go by `on_mic_done`.
        if self.mic.wanted && self.mic.cap.is_none() {
            let fx = self.ctl_on(Event::MicOpened);
            self.run(fx);
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
            TimerKey::SvcRearm => self.svc_on(SvcEvent::Timer(TimerId::HookRearm)),
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
            DictMsg::MicLost { epoch, why } => self.on_mic_lost(epoch, why),
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
            DictMsg::Sent(d, text) => {
                if self.svc.is_some() {
                    self.svc_on(SvcEvent::Sent(d, text));
                } else if matches!(d, SendDecision::No(_)) {
                    // Dictation was switched off while the send was out: the words are still kept.
                    self.keep_words(text);
                }
            }
            DictMsg::LinkState(on) => self.svc_on(SvcEvent::LinkState(on)),
            DictMsg::PasteFront(text) => {
                // Another copy routed these words here. With no shell in front they would be lost: keep them.
                if !self.paste_front(&text) {
                    self.keep_words(text);
                }
            }
            DictMsg::Paste { session, text } => self.paste(&session, &text),
            DictMsg::Pick(id) => self.svc_on(SvcEvent::Pick(id)),
            DictMsg::Dismiss => self.svc_on(SvcEvent::Dismiss),
            DictMsg::Copied {
                ok,
                shown,
                fallback,
            } => self.on_copied(ok, shown, fallback),
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
                Effect::Keep(text) => self.keep_words(text),
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
        let post_lost = Arc::clone(&self.post);
        let lost: LostFn = Box::new(move |why| post_lost(DictMsg::MicLost { epoch, why }));
        self.mic.lost_early = None;
        if let Some(p) = self.parts.as_mut() {
            p.open_mic(device, on_frame, done, lost);
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
                if let Some(why) = self.mic.lost_early.take() {
                    self.lose_mic(why);
                    return;
                }
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

    /// The device went away while recording: the capture is let go and the recording ends with the failure.
    fn on_mic_lost(&mut self, epoch: u64, why: CaptureFailure) {
        if epoch != self.mic.epoch || !self.mic.wanted {
            return;
        }
        if self.mic.cap.is_none() {
            self.mic.lost_early = Some(why);
            return;
        }
        self.lose_mic(why);
    }

    fn lose_mic(&mut self, why: CaptureFailure) {
        self.close_mic();
        let fx = self.ctl_on(Event::MicFailed(why));
        self.run(fx);
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
                self.paste_gone = true;
                // The tab closed under the paste: the words are kept, not dropped.
                self.keep_words(clean);
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

    /// The e2e seam `DictRoute`: returns false when there is no service to route through (dictation is off).
    fn e2e_route(&mut self, text: String, front: Option<u32>) -> bool {
        let now = self.host.now_ms();
        let Some(s) = self.svc.as_mut() else {
            return false;
        };
        let fx = s.route(text, front, now);
        self.run_svc(fx);
        true
    }

    /// The e2e seam `DictRouted`: a routed result handed in as the window would hand it.
    fn svc_routed(&mut self, r: RouteResult) -> bool {
        if self.svc.is_none() {
            return false;
        }
        self.svc_on(SvcEvent::Routed(r));
        true
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
                SvcEffect::HookRec(on) => {
                    if let Some(p) = self.parts.as_mut() {
                        p.hook_rec(on);
                    }
                }
                SvcEffect::NoteFront => self.rec_session = self.host.front_session(),
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
                    // The tab that was in front when the recording began, not the one in front now. With none (or
                    // one that closed) the words are kept; with none the service goes on to the list. A route that never
                    // noted a front (no recording began here) takes the tab in front now.
                    let session = self
                        .rec_session
                        .take()
                        .or_else(|| self.host.front_session());
                    let had = session.is_some();
                    let fx = self.ctl_on(Event::Front(session));
                    self.run(fx);
                    self.paste_gone = false;
                    let fx = self.ctl_on(Event::PasteFront(text.clone()));
                    self.run(fx);
                    if !had {
                        self.svc_on(SvcEvent::Pasted(false));
                    } else if self.paste_gone {
                        self.svc_on(SvcEvent::PasteFailed);
                    } else {
                        self.svc_on(SvcEvent::Pasted(true));
                    }
                }
                SvcEffect::PasteCopy { pid, text } => {
                    let ok = self
                        .parts
                        .as_mut()
                        .is_some_and(|p| p.link_paste(pid, &text));
                    self.svc_on(SvcEvent::Pasted(ok));
                }
                SvcEffect::CopyText {
                    text,
                    shown,
                    fallback,
                } => self.copy_words(text, shown, fallback),
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

    /// Pastes `text` into the front tab; false when no shell is in front (nothing was pasted).
    fn paste_front(&mut self, text: &str) -> bool {
        let front = self.host.front_session();
        let has_shell = front.is_some();
        let fx = self.ctl_on(Event::Front(front));
        self.run(fx);
        let fx = self.ctl_on(Event::PasteFront(text.to_string()));
        self.run(fx);
        has_shell
    }

    /// Keeps words on the clipboard with no note of its own (a failed copy still says so, see `on_copied`).
    fn keep_words(&mut self, text: String) {
        self.copy_words(text, None, None);
    }

    fn copy_words(
        &mut self,
        text: String,
        shown: Option<OverlayState>,
        fallback: Option<OverlayState>,
    ) {
        let post = Arc::clone(&self.post);
        self.host.copy_text(
            text,
            Box::new(move |ok| {
                post(DictMsg::Copied {
                    ok,
                    shown,
                    fallback,
                })
            }),
        );
    }

    /// A copy of unsent words answered. A failed one puts the plain note back, but only while the note that
    /// said "copied" is still the face on screen (compared exactly): a newer face is never replaced. A failed
    /// copy with no note of its own says so when nothing else is showing.
    fn on_copied(&mut self, ok: bool, shown: Option<OverlayState>, fallback: Option<OverlayState>) {
        if ok {
            return;
        }
        let face = match fallback {
            Some(face) => {
                if shown.is_none() || self.widget != shown {
                    return;
                }
                face
            }
            None => {
                if self.widget.is_some() || self.svc.is_none() {
                    return;
                }
                OverlayState::Note {
                    message: COPY_FAILED.into(),
                    tone: Tone::Warn,
                }
            }
        };
        self.run_svc(vec![
            SvcEffect::Widget(face),
            SvcEffect::WidgetHide {
                after_ms: flare_dictation::service::NOTE_HIDE_MS,
            },
        ]);
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
        let kept = text.clone();
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
                post(DictMsg::Sent(d, text));
            });
        if spawned.is_err() {
            self.svc_on(SvcEvent::Sent(
                SendDecision::No("the send could not start".into()),
                kept,
            ));
        }
    }

    fn send_remote(&mut self, pid: u32, id: &str, text: &str) {
        let Some(p) = self.parts.as_mut() else {
            // No machinery to send with: the words are kept, not left on "Sending".
            return self.svc_on(SvcEvent::Sent(
                SendDecision::No("the send could not start".into()),
                text.to_string(),
            ));
        };
        let rx = p.link_send(pid, id, text, Duration::from_millis(link::SEND_MS));
        let post = Arc::clone(&self.post);
        let words = text.to_string();
        let spawned = std::thread::Builder::new()
            .name("flare-dictation-send-remote".into())
            .spawn(move || {
                // sync-ok: a worker waits for the other copy's answer (the link's own limit applies)
                let got = rx.recv(); // sync-ok: a worker waits for the other copy's answer (the link's own limit applies)
                let d = got
                    .unwrap_or_else(|_| SendDecision::No("that copy of the app has closed".into()));
                post(DictMsg::Sent(d, words));
            });
        if spawned.is_err() {
            self.svc_on(SvcEvent::Sent(
                SendDecision::No("the send could not start".into()),
                text.to_string(),
            ));
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
        TimerId::HookRearm => TimerKey::SvcRearm,
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
        // The Dictation page's key field, while it waits for a key, takes every key (PT's capture listener).
        match capture_key(ev, up) {
            CaptureTake::NotWaiting => {}
            CaptureTake::Taken => return true,
            CaptureTake::TakenAltUp => {
                with_dict(|d| d.host.swallow_alt_up());
                return true;
            }
        }
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
    // A window that loses the keyboard ends a key capture (PT: `window.addEventListener('blur', stop)`).
    cancel_capture();
    if !armed() {
        return;
    }
    with_dict(Dict::on_blur);
}

/// Quit: stops the engine, the host, the hook and the link before the shells are killed.
pub fn stop_all() {
    page_left();
    with_dict(Dict::stop);
    ARMED.with(|a| a.set(false));
    crate::dictation_widget::close();
}

/// A row clicked in the widget (M8 Task 26): it reaches the service on the next turn of the event loop.
pub fn widget_pick(id: String) {
    make_post()(DictMsg::Pick(id));
}

/// The model store: the process's one store, made at startup by [install] and kept for the life of the process, so
/// the page's model cards work while dictation is off and a download still running can be cancelled (F1, F2).
pub fn store() -> Option<Store> {
    #[cfg(test)]
    if let Some(s) = TEST_STORE.with(|s| s.borrow().clone()) {
        return Some(s);
    }
    with_dict(|d| d.parts.as_ref().and_then(|p| p.store()))
        .flatten()
        .or_else(|| APP_STORE.get().cloned())
}

// ---------------------------------------------------------------------------------------------------------------------
// The Dictation page: what it shows that is not stored, and what its controls do (M8 Task 25)
// ---------------------------------------------------------------------------------------------------------------------
//
// NOTHING HERE STARTS ANYTHING BY ITSELF (PT DictationPage.tsx): opening the page opens no microphone (the meter is
// behind a Test button) and downloads nothing; every download is a press. The page's own state lives in `PAGE` on
// the UI thread; what workers learn (a download's progress, the microphone list) arrives as a posted call.

/// How a key was met by the capture.
pub enum CaptureTake {
    /// The field is not waiting: the key goes on.
    NotWaiting,
    /// The capture took the key.
    Taken,
    /// The capture took a modifier's release that is an Alt (the window menu must not open).
    TakenAltUp,
}

/// Said under the microphone row when the Test could not open it (PT MicField).
const MIC_DENIED: &str = "Microphone access was refused.";
const MIC_NONE: &str = "No working microphone was found.";
/// The Test ends by itself after this long (PT: 10000 ms).
const MIC_TEST_MS: u64 = 10_000;
/// How often the meter moves (80 ms, as PT's transition).
const METER_TICK_MS: u64 = 80;
const TEST_TICK: &str = "dictation.mic-test";
const TEST_STOP: &str = "dictation.mic-test-stop";

/// What the page keeps between rebuilds.
#[derive(Default)]
struct PageUi {
    /// The key field waits for a key.
    capturing: bool,
    /// A modifier (or chord of modifiers) pressed during the capture, committed on its release.
    bare: Option<Hotkey>,
    /// Codes down during the capture, to tell a repeat from a press.
    held: HashSet<String>,
    /// A press elsewhere just ended the capture: the field's own click that follows must not start it again.
    press_cancelled: bool,
    /// The model whose download waits on the one-line question.
    ask: Option<String>,
    mic_testing: bool,
    /// The meter, smoothed (PT: up at once, down by 85/15).
    mic_level: f32,
    mic_problem: Option<String>,
    /// The microphones found (listed on a worker when the page opens); `None` until listed.
    mics: Option<Vec<(String, String)>>,
    mics_pending: bool,
    /// Which Test is current: an answer of an older one lets go of its microphone.
    test_epoch: u64,
    test_cap: Option<Box<dyn Capture>>,
    /// The peak level heard since the last tick, as `f32` bits.
    test_peak: Option<Arc<std::sync::atomic::AtomicU32>>,
    /// The stored settings, for what a worker finishes.
    prefs: Option<flare_settings::kv::PrefStore>,
    /// Rebuilds the page (Settings registers it).
    refresh: Option<Rc<dyn Fn()>>,
    /// Moves the meter without a rebuild.
    meter: Option<Rc<dyn Fn(f32)>>,
}

thread_local! {
    static PAGE: RefCell<PageUi> = RefCell::new(PageUi::default());
    /// A store the tests put in place of the real one.
    #[cfg(test)]
    static TEST_STORE: RefCell<Option<Store>> = const { RefCell::new(None) };
    /// A microphone the tests put in place of the real one (a WAV, never the device).
    #[cfg(test)]
    static TEST_MIC: RefCell<Option<MicSource>> = const { RefCell::new(None) };
}

/// A download or removal under way, or its last failure; written by workers, read when the page is built.
#[derive(Clone, Debug, Default)]
struct Run {
    done: u64,
    active: bool,
    /// Which download started this run (audit L47): the end of an older flight, one that was cancelled and then
    /// asked for again, must not mark the new one inactive.
    flight: u64,
    failure: Option<flare_dictation::store::Failure>,
}

static RUNS: std::sync::LazyLock<Mutex<HashMap<String, Run>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

fn runs() -> std::sync::MutexGuard<'static, HashMap<String, Run>> {
    RUNS.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// What the page needs of its own state, read when a page is built.
#[derive(Clone, Debug, Default)]
pub struct PageSnap {
    pub capturing: bool,
    pub ask: Option<String>,
    pub mic_testing: bool,
    pub mic_level: f32,
    pub mic_problem: Option<String>,
}

/// The page's state now.
pub fn page_snap() -> PageSnap {
    PAGE.with(|p| {
        let p = p.borrow();
        PageSnap {
            capturing: p.capturing,
            ask: p.ask.clone(),
            mic_testing: p.mic_testing,
            mic_level: p.mic_level,
            mic_problem: p.mic_problem.clone(),
        }
    })
}

/// The microphones the page lists: the last listing, or nothing yet (a worker lists them, never this thread).
pub fn page_mics() -> Vec<(String, String)> {
    let (have, start) = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let start = p.mics.is_none() && !p.mics_pending;
        if start {
            p.mics_pending = true;
        }
        (p.mics.clone().unwrap_or_default(), start)
    });
    // The tests list nothing: no unit test enumerates the machine's devices.
    if start && !cfg!(test) {
        std::thread::Builder::new()
            .name("flare-mic-list".into())
            .spawn(|| {
                let found: Vec<(String, String)> = mic::list_mics()
                    .into_iter()
                    .map(|m| (m.id, m.name))
                    .collect();
                post_ui(move || {
                    PAGE.with(|p| {
                        let mut p = p.borrow_mut();
                        p.mics = Some(found);
                        p.mics_pending = false;
                    });
                    page_changed();
                });
            })
            .ok();
    }
    have
}

/// How long a "not installed" answer is believed (audit L46). The store believes a yes for 30 s but never a no, and a
/// download's progress rebuilds the page up to ten times a second, so every absent model was a file check on the UI
/// thread each time. A download or removal ending forgets the answers at once.
const ABSENT_MEMO: Duration = Duration::from_secs(3);

thread_local! {
    /// Catalogue ids found not installed, and when.
    static ABSENT: RefCell<HashMap<String, Instant>> = RefCell::new(HashMap::new());
}

/// Whether `id` was found not installed a moment ago.
fn absent_recently(id: &str) -> bool {
    ABSENT.with(|a| {
        a.borrow()
            .get(id)
            .is_some_and(|t| t.elapsed() < ABSENT_MEMO)
    })
}

fn forget_absent(id: &str) {
    ABSENT.with(|a| {
        a.borrow_mut().remove(id);
    });
}

/// Whether the page may show `id` as installed: the answer of `check` (a file check), except that a recent "no" is
/// not asked again.
pub(crate) fn installed_for_page(id: &str, check: impl FnOnce() -> bool) -> bool {
    // Only while a download reports progress: a page opened at rest looks at the disk, so a model file put in place
    // by hand shows at once.
    if !runs().values().any(|r| r.active) {
        forget_absent(id);
        return check();
    }
    if absent_recently(id) {
        return false;
    }
    let yes = check();
    if !yes {
        ABSENT.with(|a| {
            a.borrow_mut().insert(id.to_string(), Instant::now());
        });
    }
    yes
}

/// The download state of a catalogue entry: its percentage while it runs, the failure of its last end.
pub fn run_view(id: &str, bytes: u64) -> (Option<u32>, Option<&'static str>) {
    let r = runs().get(id).cloned().unwrap_or_default();
    let pct = r.active.then(|| {
        if bytes == 0 {
            0
        } else {
            ((r.done as f64 / bytes as f64) * 100.0).round().min(100.0) as u32
        }
    });
    let failure = match r.failure {
        None | Some(flare_dictation::store::Failure::Cancelled) => None,
        Some(f) => Some(f.text()),
    };
    (pct, failure)
}

thread_local! {
    /// Catalogue ids the scenes show as installed (the headless scenes have no store).
    static SCENE_INSTALLED: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
}

/// Whether a scene says this catalogue entry is installed.
pub(crate) fn scene_installed(id: &str) -> bool {
    SCENE_INSTALLED.with(|s| s.borrow().contains(&id))
}

/// The Dictation page's fixed states for the headless scenes and the scaling images (M8 Task 35): no store, no worker,
/// no microphone listing. `installed` entries wear their installed state, `downloading` is one entry at a percentage,
/// `ask` is the model whose one-line question is open. Called before the page is built.
pub(crate) fn scene_page(
    installed: &[&'static str],
    downloading: Option<(&'static str, u32)>,
    ask: Option<&str>,
) {
    SCENE_INSTALLED.with(|s| *s.borrow_mut() = installed.to_vec());
    let mut r = runs();
    r.clear();
    if let Some((id, pct)) = downloading {
        let bytes = flare_dictation::catalog::visible_models()
            .into_iter()
            .find(|m| m.id == id)
            .map_or(0, |m| m.bytes);
        r.insert(
            id.to_string(),
            Run {
                done: bytes / 100 * u64::from(pct),
                active: true,
                flight: 0,
                failure: None,
            },
        );
    }
    drop(r);
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.mics = Some(vec![(
            "scene-mic".to_string(),
            "Headset Microphone".to_string(),
        )]);
        p.mics_pending = false;
        p.ask = ask.map(str::to_string);
    });
}

/// Hands `f` to the UI thread. The tests have no event loop: they take what was posted and run it themselves.
fn post_ui(f: impl FnOnce() + Send + 'static) {
    #[cfg(test)]
    {
        TEST_POSTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(Box::new(f));
    }
    #[cfg(not(test))]
    if slint::invoke_from_event_loop(f).is_err() {
        log::debug!("dictation page: event loop closed");
    }
}

/// What the workers posted to the UI thread, for a test to run (see `post_ui`).
#[cfg(test)]
static TEST_POSTS: Mutex<std::collections::VecDeque<Box<dyn FnOnce() + Send>>> =
    Mutex::new(std::collections::VecDeque::new());

/// The page has something new to draw. From inside a Settings event the page is already being rebuilt, and Settings' own
/// guard (a busy controller) skips this call.
fn page_changed() {
    let refresh = PAGE.with(|p| p.borrow().refresh.clone());
    if let Some(f) = refresh {
        f();
    }
}

/// Settings tells the page how to rebuild and how to move the meter, and which store holds the settings.
pub fn register_page(
    prefs: flare_settings::kv::PrefStore,
    refresh: Rc<dyn Fn()>,
    meter: Rc<dyn Fn(f32)>,
) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.prefs = Some(prefs);
        p.refresh = Some(refresh);
        p.meter = Some(meter);
    });
}

/// The Dictation page was shown: the microphones are listed again, nothing else.
pub fn page_shown() {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if !p.mics_pending {
            p.mics = None;
        }
    });
}

/// The Dictation page went out of sight: a waiting key field, a running Test and an open question end.
pub fn page_left() {
    cancel_capture();
    stop_test();
    PAGE.with(|p| p.borrow_mut().ask = None);
}

/// Whether the engine fell back to the CPU for this session (the GPU row says so).
pub fn gpu_fell_back() -> bool {
    with_dict(|d| d.parts.as_ref().is_some_and(|p| p.gpu_fell_back())).unwrap_or(false)
}

/// A press anywhere in the window (PT: `pointerdown` in the capture phase, but not on the field itself).
pub fn on_press() {
    let was = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let was = p.capturing;
        if was {
            p.capturing = false;
            p.bare = None;
            p.held.clear();
        }
        // The field's own click, which follows this press, must not wait again.
        p.press_cancelled = was;
        was
    });
    // A press on the heading or on empty space rebuilds nothing by itself: draw the field idle.
    if was {
        page_changed();
    }
}

fn cancel_capture() {
    let was = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let was = p.capturing;
        p.capturing = false;
        p.bare = None;
        p.held.clear();
        was
    });
    if was {
        page_changed();
    }
}

fn is_modifier_code(code: &str) -> bool {
    matches!(
        code,
        "AltLeft"
            | "AltRight"
            | "ControlLeft"
            | "ControlRight"
            | "ShiftLeft"
            | "ShiftRight"
            | "MetaLeft"
            | "MetaRight"
    )
}

/// One key while the field may be waiting (PT HotkeyField). A bare modifier (Right Alt, the default) is taken on its
/// RELEASE, so Ctrl+Shift+D is not captured as "Ctrl"; a chord of modifiers is the last modifier's, also on its
/// release; a key that types, or a chord the terminal owns, is not taken and the field keeps waiting; Escape leaves.
fn capture_key(ev: &KeyEvent, up: bool) -> CaptureTake {
    let commit = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if !p.capturing {
            return Err(CaptureTake::NotWaiting);
        }
        let code = w3c_code(&ev.code).to_string();
        if up {
            p.held.remove(&code);
            let bare = p.bare.take();
            return match bare {
                Some(b) if b.code == code => {
                    p.capturing = false;
                    p.held.clear();
                    Ok((Some(b), code.starts_with("Alt")))
                }
                other => {
                    p.bare = other;
                    Ok((None, code.starts_with("Alt")))
                }
            };
        }
        if !p.held.insert(code.clone()) {
            return Ok((None, false));
        }
        if code == "Escape" {
            p.capturing = false;
            p.bare = None;
            p.held.clear();
            return Ok((None, false));
        }
        let chord = KeyChord {
            code: code.clone(),
            ctrl: ev.ctrl,
            alt: ev.alt,
            shift: ev.shift,
            meta: ev.meta,
            repeat: false,
            at: 0.0,
        };
        let Some(hk) = parse_hotkey_from_event(&chord) else {
            return Ok((None, false));
        };
        if is_modifier_code(&code) {
            p.bare = Some(hk);
        } else if usable_hotkey(&hk, &[]) {
            p.capturing = false;
            p.held.clear();
            return Ok((Some(hk), false));
        }
        Ok((None, false))
    });
    match commit {
        Err(not_waiting) => not_waiting,
        Ok((chosen, alt_up)) => {
            if let Some(hk) = chosen {
                write_hotkey(&hk);
            }
            page_changed();
            if up && alt_up {
                CaptureTake::TakenAltUp
            } else {
                CaptureTake::Taken
            }
        }
    }
}

fn write_hotkey(h: &Hotkey) {
    let json =
        json!({ "code": h.code, "ctrl": h.ctrl, "alt": h.alt, "shift": h.shift, "meta": h.meta })
            .to_string();
    page_put(2, &json);
}

/// Writes one of the nine stored dictation keys (an index of `prefs::KEYS`).
fn page_put(i: usize, value: &str) {
    if let Some(p) = PAGE.with(|p| p.borrow().prefs.clone()) {
        p.set(prefs::KEYS[i], value);
    }
}

fn page_get(i: usize) -> Option<String> {
    PAGE.with(|p| p.borrow().prefs.clone())
        .and_then(|p| p.get(prefs::KEYS[i]))
}

/// The active model's id as stored.
fn active_model() -> String {
    page_get(7).unwrap_or_default()
}

/// Whether the row `id` is one the Dictation page's controls answer.
pub fn owns_row(id: &str) -> bool {
    id.starts_with("dictation-")
        || id == flare_dictation::catalog::GPU_PACK.id
        || flare_dictation::catalog::model(id).is_some()
}

/// A switch of the page. `true` when something changed that the page must show.
pub fn page_switch(row: &str, on: bool) -> bool {
    let (i, v) = match row {
        "dictation-enabled" => (0, on),
        "dictation-pause-media" => (5, on),
        "dictation-sounds" => (6, on),
        "dictation-anywhere" => (8, on),
        _ => return false,
    };
    if row == "dictation-enabled" && !on {
        // Off means off: nothing waits, listens or asks.
        page_left();
    }
    page_put(i, if v { "1" } else { "0" });
    true
}

/// A choice of the page (the mode, the microphone, the language).
pub fn page_choose(row: &str, option: &str) -> bool {
    match row {
        "dictation-mode" if option == "hold" || option == "toggle" => page_put(1, option),
        "dictation-mic" => {
            // The Test follows the microphone it was started on: a new pick ends it.
            stop_test();
            page_put(3, option);
        }
        "dictation-language" if flare_dictation::catalog::valid_language(option) == option => {
            page_put(4, option);
        }
        _ => return false,
    }
    true
}

/// A button of the page: the key field, the microphone's Test, a model's or the pack's buttons. `true` when the
/// page must be built again.
pub fn page_button(row: &str, button: &str) -> bool {
    use flare_dictation::catalog;
    match (row, button) {
        ("dictation-hotkey", "capture") => {
            PAGE.with(|p| {
                let mut p = p.borrow_mut();
                if p.press_cancelled {
                    // The press before this click ended a capture: the click closes it, it does not reopen it.
                    p.press_cancelled = false;
                } else {
                    p.capturing = !p.capturing;
                    p.bare = None;
                    p.held.clear();
                }
            });
            true
        }
        ("dictation-hotkey", "reset") => {
            write_hotkey(&flare_dictation::hotkey::default_hotkey());
            true
        }
        ("dictation-mic", "test") => {
            if PAGE.with(|p| p.borrow().mic_testing) {
                stop_test();
            } else {
                start_test();
            }
            true
        }
        (id, "download") if catalog::model(id).is_some() => {
            fetch_model(id);
            true
        }
        (id, "cancel") if catalog::model(id).is_some() || id == catalog::GPU_PACK.id => {
            if let Some(s) = page_store() {
                s.cancel(id);
            }
            true
        }
        (id, "use") if catalog::model(id).is_some() => {
            page_put(7, id);
            true
        }
        (id, "uninstall") if catalog::model(id).is_some() => {
            remove_item(id);
            true
        }
        (id, "enable") if id == catalog::GPU_PACK.id => {
            download_item(id);
            true
        }
        (id, "disable") if id == catalog::GPU_PACK.id => {
            remove_item(id);
            true
        }
        _ => false,
    }
}

/// The answer of the one-line question: Download starts the download, Cancel drops it.
pub fn page_ask_answer(download: bool) -> bool {
    let id = PAGE.with(|p| p.borrow_mut().ask.take());
    match id {
        Some(id) if download => {
            download_item(&id);
            true
        }
        Some(_) => true,
        None => false,
    }
}

fn page_store() -> Option<Store> {
    store()
}

/// A model that does not support every language says so once, before its download (#121). A Retry after a failed one
/// has already been asked.
fn fetch_model(id: &str) {
    let failed = runs().get(id).is_some_and(
        |r| matches!(r.failure, Some(f) if f != flare_dictation::store::Failure::Cancelled),
    );
    if flare_dictation::catalog::limited_languages(id) && !failed {
        PAGE.with(|p| p.borrow_mut().ask = Some(id.to_string()));
    } else {
        download_item(id);
    }
}

static NEXT_FLIGHT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Starts the download of a catalogue id (ids come from the catalogue only, never from the page).
fn download_item(id: &str) {
    use flare_dictation::catalog;
    let known = catalog::model(id).is_some() || id == catalog::GPU_PACK.id;
    let Some(store) = page_store().filter(|_| known) else {
        return;
    };
    forget_absent(id);
    let flight = NEXT_FLIGHT.fetch_add(1, Ordering::Relaxed);
    runs().insert(
        id.to_string(),
        Run {
            done: 0,
            active: true,
            flight,
            failure: None,
        },
    );
    let for_progress = id.to_string();
    let rx = store.download(
        id,
        Box::new(move |p| {
            if let Some(r) = runs().get_mut(&for_progress).filter(|r| r.flight == flight) {
                r.done = p.done;
            }
            post_ui(page_changed);
        }),
    );
    let id = id.to_string();
    // The end of the download is waited for on its own thread; what follows runs on the UI thread.
    std::thread::Builder::new()
        .name("flare-dict-wait".into())
        .spawn(move || {
            let end = rx
                .recv()
                .unwrap_or(Err(flare_dictation::store::Failure::Network));
            post_ui(move || finish_download(&id, flight, end));
        })
        .ok();
}

/// A download ended. The first model anyone downloads is the one they meant to use: read NOW, not when the press
/// was made, so a second download finishing after the first had become the model never replaces it unasked.
fn finish_download(id: &str, flight: u64, end: Result<(), flare_dictation::store::Failure>) {
    if let Some(r) = runs().get_mut(id).filter(|r| r.flight == flight) {
        r.active = false;
        r.failure = end.err();
    }
    forget_absent(id);
    if end.is_ok() && flare_dictation::catalog::model(id).is_some() {
        let current = active_model();
        let installed =
            !current.is_empty() && page_store().is_some_and(|s| s.model_path(&current).is_some());
        if !installed {
            page_put(7, id);
        }
    }
    // A download that succeeded stops the engine, so the next pass starts from what is on disk now. A failure or a
    // cancel leaves it running: a dictation pass in flight on it keeps its server or resident host (M8 review F5).
    if end.is_ok() {
        stop_engine_now();
    }
    page_changed();
}

/// Removes a model or the pack: a running download is cancelled and the engine stops first, so nothing holds the
/// files; removing the active model clears the stored id.
fn remove_item(id: &str) {
    let Some(store) = page_store() else {
        return;
    };
    store.cancel(id);
    stop_engine_now();
    let rx = store.remove(id);
    let id = id.to_string();
    std::thread::Builder::new()
        .name("flare-dict-remove".into())
        .spawn(move || {
            let _ = rx.recv();
            post_ui(move || {
                runs().remove(&id);
                forget_absent(&id);
                if active_model() == id {
                    page_put(7, "");
                }
                page_changed();
            });
        })
        .ok();
}

fn stop_engine_now() {
    with_dict(|d| {
        if let Some(p) = d.parts.as_mut() {
            p.stop_engine();
        }
    });
}

/// Where the microphone's sound comes from: the device, or the e2e seam's WAV.
fn mic_source() -> MicSource {
    #[cfg(test)]
    if let Some(s) = TEST_MIC.with(|m| m.borrow().clone()) {
        return s;
    }
    let e2e = crate::e2e::global();
    mic_source_for(e2e.on(), e2e.env(names::SEAM_MIC))
}

/// The source given the e2e flag and its mic seam. Under --e2e an unset or empty seam FAILS CLOSED
/// (a path that cannot open, so the capture reports a failure), never the real microphone (audit L54).
fn mic_source_for(e2e: bool, seam: Option<String>) -> MicSource {
    match seam {
        Some(p) if !p.is_empty() => MicSource::File(PathBuf::from(p)),
        _ if e2e => MicSource::File(PathBuf::new()),
        _ => MicSource::Device,
    }
}

/// The Test button: opens the chosen microphone on a worker and moves the meter for at most ten seconds.
fn start_test() {
    use std::sync::atomic::{AtomicU32, Ordering as O};
    let device = page_get(3).filter(|d| !d.is_empty());
    let (epoch, peak) = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.mic_problem = None;
        p.mic_testing = true;
        p.mic_level = 0.0;
        p.test_epoch += 1;
        let peak = Arc::new(AtomicU32::new(0));
        p.test_peak = Some(Arc::clone(&peak));
        (p.test_epoch, peak)
    });
    let on_frame: FrameFn = Box::new(move |samples: &[f32]| {
        let l = flare_dictation::wav::level(samples);
        let mut cur = peak.load(O::Relaxed);
        while f32::from_bits(cur) < l {
            match peak.compare_exchange_weak(cur, l.to_bits(), O::Relaxed, O::Relaxed) {
                Ok(_) => break,
                Err(now) => cur = now,
            }
        }
    });
    let done: DoneFn = Box::new(move |result| post_ui(move || test_opened(epoch, result)));
    mic::start_capture_async(mic_source(), device, on_frame, done);
    crate::timer_registry::set(TEST_TICK, METER_TICK_MS, true, meter_tick);
    crate::timer_registry::set(TEST_STOP, MIC_TEST_MS, false, || {
        stop_test();
        page_changed();
    });
}

fn test_opened(epoch: u64, result: Result<Box<dyn Capture>, CaptureFailure>) {
    let current = PAGE.with(|p| {
        let p = p.borrow();
        p.mic_testing && p.test_epoch == epoch
    });
    match result {
        Ok(cap) if current => PAGE.with(|p| p.borrow_mut().test_cap = Some(cap)),
        // A late answer of a Test nobody wants any more: let go of the microphone at once.
        Ok(cap) => {
            // sync-ok: ending a capture joins its own thread, which stops within a frame (10 ms)
            drop(cap.stop());
        }
        Err(f) if current => {
            PAGE.with(|p| {
                p.borrow_mut().mic_problem = Some(
                    if f == CaptureFailure::Denied {
                        MIC_DENIED
                    } else {
                        MIC_NONE
                    }
                    .to_string(),
                );
            });
            stop_test();
            page_changed();
        }
        Err(_) => {}
    }
}

/// The meter: the peak since the last tick, up at once and down slowly (PT: `shown*0.85 + l*0.15`).
fn meter_tick() {
    use std::sync::atomic::Ordering as O;
    let (level, sink) = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let peak = p
            .test_peak
            .as_ref()
            .map_or(0.0, |a| f32::from_bits(a.swap(0, O::Relaxed)));
        p.mic_level = if peak > p.mic_level {
            peak
        } else {
            p.mic_level * 0.85 + peak * 0.15
        };
        (p.mic_level.min(1.0), p.meter.clone())
    });
    if let Some(f) = sink {
        f(level);
    }
}

/// Ends the Test: the microphone is let go and the meter falls to nothing.
fn stop_test() {
    let (cap, sink) = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.mic_testing = false;
        p.mic_level = 0.0;
        p.test_epoch += 1;
        p.test_peak = None;
        (p.test_cap.take(), p.meter.clone())
    });
    crate::timer_registry::clear(TEST_TICK);
    crate::timer_registry::clear(TEST_STOP);
    if let Some(cap) = cap {
        // sync-ok: ending a capture joins its own thread, which stops within a frame (10 ms)
        drop(cap.stop());
    }
    if let Some(f) = sink {
        f(0.0);
    }
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
    // The model store exists from startup, dictation on or off: the Speech models and GPU cards are live while it is
    // off (PT disables only Listening and While), so they read it and a download starts from it (F2). It opens no
    // file and starts nothing; the engine, hook and microphone still exist only while dictation is on.
    shared_store(&APP_STORE, || Store::open(cfg.root.clone(), store_fetch()));
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

/// The test hook's `DictLine`: one helper line, as if the keyboard hook had printed it (the real hook is never
/// installed under --e2e). A line that is not a helper line is refused.
pub fn e2e_line(line: &str) -> Result<(), String> {
    let ev = flare_dictation::keyfeed::parse_helper_line(line)
        .ok_or_else(|| format!("not a helper line: {line:?}"))?;
    deliver(DictMsg::Hook(ev));
    Ok(())
}

/// The test hook's `DictRoute`: `text` was heard with the window of process `front` in front.
pub fn e2e_route(text: &str, front: Option<u32>) -> Result<(), String> {
    match with_dict(|d| d.e2e_route(text.to_string(), front)) {
        Some(true) => Ok(()),
        Some(false) => Err("dictation is off: nothing routes".into()),
        None => Err("dictation is busy: the route was not taken".into()),
    }
}

/// The test hook's `DictRouted`: what the window hands main as heard.
pub fn e2e_routed(result: &WireRouteResult) -> Result<(), String> {
    let r = match result {
        WireRouteResult::Text { text } => RouteResult::Text(text.clone()),
        WireRouteResult::Message { message } => RouteResult::Message(message.clone()),
    };
    match with_dict(|d| d.svc_routed(r)) {
        Some(true) => Ok(()),
        Some(false) => Err("dictation is off: nothing routes".into()),
        None => Err("dictation is busy: the result was not taken".into()),
    }
}

/// The engine as the probes read it: whether one runs, its kind, and whether a GPU engine fell back.
pub fn engine_probe() -> (bool, Option<flare_dictation::engine::EngineKind>, bool) {
    with_dict(|d| {
        d.parts.as_ref().map(|p| {
            let (running, kind) = p.engine_state();
            (running, kind, p.gpu_fell_back())
        })
    })
    .flatten()
    .unwrap_or((false, None, false))
}

/// This copy's dictation role, as the probes read it.
pub fn role_probe() -> Role {
    with_dict(|d| d.role()).unwrap_or(Role::None)
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
    clip: RefCell<Option<Arc<dyn flare_win::clipboard_port::ClipboardPort>>>,
}

impl AppHost {
    fn new(ctl: std::rc::Weak<RefCell<AppCtl>>, app: slint::Weak<AppWindow>) -> Self {
        AppHost {
            ctl,
            app,
            t0: Instant::now(),
            guarded: Cell::new(false),
            clip: RefCell::new(None),
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

    fn prompt_has_draft(&self, tab: &str) -> bool {
        let Some(rc) = self.ctl.upgrade() else {
            return false;
        };
        let Ok(c) = rc.try_borrow() else {
            return false;
        };
        c.handle_of(tab).is_some_and(|h| {
            flare_agent::question::prompt_has_draft(
                &h.typed_tail(flare_agent::indicator::SCREEN_TAIL_ROWS),
            )
        })
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

    fn view_changed(&self) {
        if let Some(app) = self.app.upgrade() {
            push_pill(&app, &view());
        }
    }

    fn copy_text(&self, text: String, done: Box<dyn FnOnce(bool) + Send>) {
        // The port is taken from the app once and kept, so a busy borrow of the app never loses a copy.
        let fresh = self
            .ctl
            .upgrade()
            .and_then(|rc| rc.try_borrow().ok().map(|c| c.dictation_clip()));
        if let Some(p) = &fresh {
            *self.clip.borrow_mut() = Some(Arc::clone(p));
        }
        let port = fresh.or_else(|| self.clip.borrow().clone());
        match port {
            Some(port) => {
                crate::clip_flow::copy_async(port, text, flare_win::clipboard_port::COPY_MAX, done)
            }
            None => done(false),
        }
    }

    fn widget(&self, face: Option<&OverlayState>) {
        let app = self.app.clone();
        crate::dictation_widget::set(
            face,
            &move || app.upgrade().map(|a| crate::dictation_widget::inks_of(&a)),
            crate::e2e::global().on(),
        );
    }
}

/// The process tree under a tab's shell (and on macOS its foreground job).
struct MgrProbe(Arc<flare_term::session::SessionManager>);

impl AgentProbe for MgrProbe {
    fn agent_under(&self, tab: &str) -> Option<String> {
        let pid = self
            .0
            .live_pids()
            .into_iter()
            .find(|(id, _)| id == tab)
            .map(|(_, pid)| pid)?;
        let rows = crate::agent_hub::tab_process_rows(&self.0, &[pid])?;
        use flare_agent::kinds::DetectedAgent;
        flare_agent::detect::tab_agent_kind(&rows, pid).map(|k| {
            match k {
                DetectedAgent::Claude => "claude",
                DetectedAgent::Codex => "codex",
                DetectedAgent::Agy => "agy",
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
    /// The neighbour apps' models folder, read once per switch-on (O14, Q9). `None` when the root is overridden (a test or e2e
    /// profile): the user's own neighbour models are never linked into an isolated store.
    neighbour_root: Option<PathBuf>,
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
        let mic = mic_source();
        let dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
        let engine_dir = flare_win::seam::var_os(names::SEAM_WHISPER_DIR)
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
            neighbour_root: flare_win::seam::var_os(flare_win::names::SEAM_DICTATION_ROOT)
                .filter(|v| !v.is_empty())
                .is_none()
                .then(flare_dictation::store::neighbour_root),
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

/// The download the store uses: the e2e's stand-in when `FLARE_E2E_FETCH=stall` (M8 Task 30, read only under
/// --e2e, never a request), else the real HTTPS fetch.
fn store_fetch() -> Box<dyn flare_dictation::store::Fetch> {
    if crate::e2e::global()
        .env(flare_win::names::SEAM_FETCH)
        .as_deref()
        == Some("stall")
    {
        return Box::new(flare_dictation::fetch_ureq::StallFetch::new());
    }
    Box::new(flare_dictation::fetch_ureq::UreqFetch::new())
}

/// The one model store of the process, made on the first switch-on and never dropped: a download keeps running
/// across a switch-off, and its Cancel must find it after the next switch-on (F1).
static APP_STORE: std::sync::OnceLock<Store> = std::sync::OnceLock::new();

/// The store in `cell`, made by `make` the first time only.
fn shared_store(cell: &std::sync::OnceLock<Store>, make: impl FnOnce() -> Store) -> Store {
    cell.get_or_init(make).clone()
}

impl RealParts {
    fn new(cfg: &RealCfg) -> Self {
        let store = shared_store(&APP_STORE, || Store::open(cfg.root.clone(), store_fetch()));
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
    fn open_mic(&mut self, device: Option<String>, on_frame: FrameFn, done: DoneFn, lost: LostFn) {
        mic::start_capture_async_watched(self.cfg.mic.clone(), device, on_frame, done, Some(lost));
    }

    fn transcribe(&mut self, req: TranscribeRequest) -> Receiver<TranscribeResult> {
        self.engine.transcribe(req)
    }

    fn reuse_neighbour_models(&mut self) {
        let Some(root) = self.cfg.neighbour_root.clone() else {
            return;
        };
        // The store links on its own thread; this waiter hands the page a refresh only when something was linked.
        let rx = self.store.reuse_models_from(root);
        std::thread::Builder::new()
            .name("flare-dict-reuse".into())
            .spawn(move || {
                if let Ok(ids) = rx.recv()
                    && !ids.is_empty()
                {
                    post_ui(page_changed);
                }
            })
            .ok();
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

    fn hook_rec(&mut self, on: bool) {
        if let Some(h) = &self.hook {
            h.rec(on);
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

    fn gpu_fell_back(&self) -> bool {
        self.engine.info().gpu_fell_back
    }

    fn engine_state(&self) -> (bool, Option<flare_dictation::engine::EngineKind>) {
        let info = self.engine.info();
        (info.running, info.engine)
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
        copies: RefCell<Vec<String>>,
        copy_ok: Cell<bool>,
        faces: RefCell<Option<OverlayState>>,
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
                copies: RefCell::default(),
                copy_ok: Cell::new(true),
                faces: RefCell::default(),
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
        fn copy_text(&self, text: String, done: Box<dyn FnOnce(bool) + Send>) {
            self.copies.borrow_mut().push(text);
            done(self.copy_ok.get());
        }
        fn widget(&self, face: Option<&OverlayState>) {
            *self.faces.borrow_mut() = face.cloned();
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
        lost: RefCell<Option<LostFn>>,
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
        fn open_mic(&mut self, _d: Option<String>, on_frame: FrameFn, done: DoneFn, lost: LostFn) {
            self.note("open_mic");
            *self.s.lost.borrow_mut() = Some(lost);
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
        fn reuse_neighbour_models(&mut self) {
            self.note("reuse_neighbour_models");
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
        fn hook_rec(&mut self, on: bool) {
            self.note(&format!("hook_rec_{on}"));
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
    fn the_first_switch_on_reuses_the_neighbours_models_and_never_while_off() {
        // O14, Q9: the neighbour apps' verified models are linked in when dictation comes on, on the worker the parts own.
        let mut r = Rig::new(false);
        r.d.apply_prefs();
        assert_eq!(
            r.count("reuse_neighbour_models"),
            0,
            "nothing is reused while dictation is off"
        );
        r.set("flare.dictation.enabled", "1");
        r.d.apply_prefs();
        assert_eq!(
            r.count("reuse_neighbour_models"),
            1,
            "the switch-on links the models the neighbour apps already have"
        );
        // A second apply while on is not a switch-on: the parts are already up.
        r.d.apply_prefs();
        assert_eq!(r.count("reuse_neighbour_models"), 1);
        r.set("flare.dictation.enabled", "0");
        r.d.apply_prefs();
        assert_eq!(
            r.count("reuse_neighbour_models"),
            1,
            "switching off reuses nothing"
        );
        r.set("flare.dictation.enabled", "1");
        r.d.apply_prefs();
        assert_eq!(
            r.count("reuse_neighbour_models"),
            2,
            "each switch-on asks again; the store links only what is still missing"
        );
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
    fn a_microphone_lost_mid_recording_ends_it_with_the_failure_and_pastes_nothing() {
        let mut r = Rig::on_with_model(false);
        r.at(1000.0);
        r.key("AltRight", false);
        r.at(1210.0);
        r.d.on_timer(TimerKey::WindowKey);
        let done = r.s.done.borrow_mut().take().unwrap();
        done(Ok(Box::new(FakeCapture)));
        r.pump();
        let mut on_frame = r.s.on_frame.borrow_mut().take().unwrap();
        for _ in 0..60 {
            on_frame(&[0.1f32; 160]);
        }
        r.pump();
        // The device is unplugged while the key is still held.
        let lost = r.s.lost.borrow_mut().take().expect("open_mic gave a lost");
        lost(CaptureFailure::NoDevice);
        r.pump();
        assert_eq!(r.d.view.message.as_deref(), Some("No microphone found"));
        assert!(!r.d.view.active, "the recording ended");
        assert!(r.d.mic.cap.is_none(), "the capture was let go");
        assert!(r.host.pastes.borrow().is_empty());
        r.at(2400.0);
        r.key("AltRight", true);
        r.pump();
        assert_eq!(r.count("transcribe_final"), 0, "nothing is transcribed");
    }

    #[test]
    fn a_loss_before_the_open_was_taken_in_still_ends_the_recording() {
        let mut r = Rig::on_with_model(false);
        r.at(1000.0);
        r.key("AltRight", false);
        r.at(1210.0);
        r.d.on_timer(TimerKey::WindowKey);
        let done = r.s.done.borrow_mut().take().unwrap();
        let lost = r.s.lost.borrow_mut().take().unwrap();
        lost(CaptureFailure::Failed);
        r.pump();
        done(Ok(Box::new(FakeCapture)));
        r.pump();
        assert_eq!(
            r.d.view.message.as_deref(),
            Some("The microphone could not be opened")
        );
        assert!(r.d.mic.cap.is_none());
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
        assert_eq!(
            r.d.view.message.as_deref(),
            Some("That tab has closed, copied")
        );
        assert_eq!(r.host.copies.borrow().len(), 1, "the words are kept");
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
        assert_eq!(
            *r.host.copies.borrow(),
            ["x "],
            "a paste into a gone tab keeps the words"
        );
        r.host.paste_outcome.set(Some(PasteOutcome::Busy));
        r.d.paste("t1", "again ");
        // (The copy of the gone tab's words answered first.)
        assert!(
            r.inbox
                .lock()
                .unwrap()
                .iter()
                .any(|m| matches!(m, DictMsg::Paste { .. }))
        );
    }

    /// How many times the engine was stopped.
    fn engine_stops(s: &Shared) -> usize {
        s.calls
            .borrow()
            .iter()
            .filter(|c| *c == "stop_engine")
            .count()
    }

    #[test]
    fn only_a_download_that_succeeds_stops_the_engine() {
        // A failed or cancelled download leaves the engine (and any pass in flight on it) alone (F5); a good one
        // stops it, so the next pass starts from what is on disk now.
        use super::fixture::pump_until;
        let (store, _dir, _calls) = fixture::fake_store(["fail", "stall", "b"]);
        fixture::install_store(store);
        register_page(
            flare_settings::kv::PrefStore::memory(),
            Rc::new(|| {}),
            Rc::new(|_: f32| {}),
        );
        let r = Rig::on_with_model(false);
        assert!(r.d.has_parts(), "the engine is up while dictation is on");
        let s = Rc::clone(&r.s);
        DICT.with(|c| *c.borrow_mut() = Some(r.d));
        assert_eq!(engine_stops(&s), 0);
        // A download that fails.
        download_item("base");
        pump_until("the failure", || run_view("base", 5).1.is_some());
        assert_eq!(
            engine_stops(&s),
            0,
            "a failed download does not stop the engine"
        );
        // A download that is cancelled.
        download_item("small");
        pump_until("the download under way", || {
            run_view("small", 5).0.is_some()
        });
        page_store().unwrap().cancel("small");
        pump_until("the cancelled end", || run_view("small", 5).0.is_none());
        assert_eq!(
            engine_stops(&s),
            0,
            "a cancelled download does not stop the engine"
        );
        // A download that succeeds.
        download_item("parakeet-v3");
        pump_until("the good download", || {
            run_view("parakeet-v3", 5).0.is_none()
        });
        assert_eq!(
            engine_stops(&s),
            1,
            "a download that succeeds stops the engine once"
        );
        let mut d = DICT.with(|c| c.borrow_mut().take()).unwrap();
        d.stop();
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

    /// Dictation on with the outside binding, and words heard with no Claude session to take them.
    fn heard_with_no_session(r: &mut Rig) {
        r.set("flare.dictation.anywhere", "1");
        r.d.apply_prefs();
        r.d.on_msg(DictMsg::Hook(HookEvent::Ready));
        assert!(r.d.e2e_route("what I said".into(), None));
        r.pump();
    }

    fn note_now(r: &Rig) -> Option<String> {
        match &*r.host.faces.borrow() {
            Some(OverlayState::Note { message, .. }) => Some(message.clone()),
            _ => None,
        }
    }

    #[test]
    fn words_with_no_session_are_copied_and_the_note_says_so() {
        let mut r = Rig::on_with_model(true);
        heard_with_no_session(&mut r);
        assert_eq!(*r.host.copies.borrow(), ["what I said"]);
        assert_eq!(note_now(&r).as_deref(), Some("No Claude session, copied"));
    }

    #[test]
    fn a_failed_copy_puts_the_old_note_back() {
        let mut r = Rig::on_with_model(true);
        r.host.copy_ok.set(false);
        heard_with_no_session(&mut r);
        assert_eq!(*r.host.copies.borrow(), ["what I said"]);
        assert_eq!(
            note_now(&r).as_deref(),
            Some("No Claude session to send to")
        );
    }

    fn note_of(message: &str) -> OverlayState {
        OverlayState::Note {
            message: message.into(),
            tone: Tone::Warn,
        }
    }

    #[test]
    fn a_failed_copy_never_replaces_a_newer_face() {
        let mut r = Rig::on_with_model(true);
        r.d.on_msg(DictMsg::Hook(HookEvent::Ready));
        // Nothing on screen: the old note stays gone.
        r.d.on_copied(
            false,
            Some(note_of("Not sent, copied")),
            Some(note_of("Not sent")),
        );
        assert_eq!(note_now(&r), None);
        // A newer ", copied" note of another event is not replaced by the earlier fallback.
        r.d.widget = Some(note_of("No Claude session, copied"));
        r.d.on_copied(
            false,
            Some(note_of("Not sent, copied")),
            Some(note_of("Not sent")),
        );
        assert_eq!(note_now(&r), None, "the host was not told");
        assert_eq!(r.d.widget, Some(note_of("No Claude session, copied")));
        // The same note still showing: the plain one goes back.
        r.d.on_copied(
            false,
            Some(note_of("No Claude session, copied")),
            Some(note_of("No Claude session to send to")),
        );
        assert_eq!(
            note_now(&r).as_deref(),
            Some("No Claude session to send to")
        );
    }

    #[test]
    fn a_failed_silent_copy_says_so_only_when_nothing_else_is_showing() {
        let mut r = Rig::on_with_model(true);
        r.host.copy_ok.set(false);
        r.d.on_msg(DictMsg::Hook(HookEvent::Ready));
        r.d.keep_words("lost?".into());
        r.pump();
        assert_eq!(note_now(&r).as_deref(), Some(COPY_FAILED));
        // A face on screen is left alone.
        r.d.widget = Some(note_of("Typed into the tab in front"));
        *r.host.faces.borrow_mut() = None;
        r.d.keep_words("lost?".into());
        r.pump();
        assert_eq!(note_now(&r), None);
    }

    #[test]
    fn a_route_to_this_copy_with_no_shell_in_front_keeps_the_words_and_never_claims_a_paste() {
        let mut r = Rig::on_with_model(true);
        r.set("flare.dictation.anywhere", "1");
        r.d.apply_prefs();
        r.d.on_msg(DictMsg::Hook(HookEvent::Ready));
        *r.host.front.borrow_mut() = None;
        assert!(r.d.e2e_route("no shell here".into(), Some(4242)));
        r.pump();
        assert!(r.host.pastes.borrow().is_empty());
        assert_eq!(*r.host.copies.borrow(), ["no shell here"]);
        assert_eq!(note_now(&r).as_deref(), Some("No Claude session, copied"));
    }

    #[test]
    fn a_route_to_this_copy_with_a_shell_in_front_pastes_and_says_so() {
        let mut r = Rig::on_with_model(true);
        r.set("flare.dictation.anywhere", "1");
        r.d.apply_prefs();
        r.d.on_msg(DictMsg::Hook(HookEvent::Ready));
        assert!(r.d.e2e_route("typed words".into(), Some(4242)));
        r.pump();
        assert_eq!(r.host.pastes.borrow().len(), 1);
        assert!(r.host.copies.borrow().is_empty());
        assert_eq!(note_now(&r).as_deref(), Some("Typed into the tab in front"));
    }

    #[test]
    fn words_routed_here_by_another_copy_with_no_shell_in_front_are_kept() {
        let mut r = Rig::on_with_model(true);
        *r.host.front.borrow_mut() = None;
        r.d.on_msg(DictMsg::PasteFront("from afar".into()));
        r.pump();
        assert_eq!(*r.host.copies.borrow(), ["from afar"]);
    }

    #[test]
    fn a_refused_send_keeps_its_own_words_and_one_that_lands_after_switch_off_too() {
        let mut r = Rig::on_with_model(true);
        r.set("flare.dictation.anywhere", "1");
        r.d.apply_prefs();
        r.d.on_msg(DictMsg::Hook(HookEvent::Ready));
        r.d.on_msg(DictMsg::Sent(SendDecision::No("busy".into()), "one".into()));
        assert_eq!(*r.host.copies.borrow(), ["one"]);
        assert_eq!(note_now(&r).as_deref(), Some("Not sent: busy, copied"));
        // Switched off while a send was out: the machinery is gone when the refusal arrives.
        r.set("flare.dictation.enabled", "0");
        r.d.apply_prefs();
        assert!(r.d.svc.is_none());
        r.d.on_msg(DictMsg::Sent(SendDecision::No("busy".into()), "two".into()));
        assert_eq!(*r.host.copies.borrow(), ["one", "two"]);
    }

    #[test]
    fn a_remote_send_with_no_machinery_answers_no_and_keeps_the_words() {
        let mut r = Rig::on_with_model(true);
        r.set("flare.dictation.anywhere", "1");
        r.d.apply_prefs();
        r.d.on_msg(DictMsg::Hook(HookEvent::Ready));
        r.d.parts = None;
        r.d.send_remote(9, "q1", "remote words");
        assert_eq!(*r.host.copies.borrow(), ["remote words"]);
        assert_eq!(
            note_now(&r).as_deref(),
            Some("Not sent: the send could not start, copied")
        );
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
    fn a_hook_that_died_is_started_again_after_a_second_with_its_keys_and_mask() {
        let mut r = Rig::new(false);
        r.set("flare.dictation.enabled", "1");
        r.set("flare.dictation.model", "tiny");
        r.set("flare.dictation.anywhere", "1");
        r.d.apply_prefs();
        assert_eq!(r.count("hook_start"), 1);
        r.d.on_msg(DictMsg::Hook(HookEvent::Ready));
        let keys = r.count("hook_keys");
        let masks = r.count("hook_mask");
        r.d.on_msg(DictMsg::Hook(HookEvent::Failed));
        assert_eq!(r.count("hook_stop"), 1, "the dead hook is dropped");
        assert_eq!(
            r.host.timers.borrow().get(&TimerKey::SvcRearm),
            Some(&(1000, false))
        );
        assert_eq!(r.count("hook_start"), 1, "not before the second is out");
        r.d.on_timer(TimerKey::SvcRearm);
        assert_eq!(r.count("hook_start"), 2, "a fresh hook");
        assert_eq!(r.count("hook_keys"), keys + 1);
        assert_eq!(r.count("hook_mask"), masks + 1);
        // Turned off meanwhile: nothing is armed.
        r.d.on_msg(DictMsg::Hook(HookEvent::Failed));
        r.set("flare.dictation.anywhere", "0");
        r.d.apply_prefs();
        assert!(!r.host.timers.borrow().contains_key(&TimerKey::SvcRearm));
        r.d.on_timer(TimerKey::SvcRearm);
        assert_eq!(r.count("hook_start"), 2);
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
    fn the_outside_level_is_sent_at_most_every_66_ms() {
        // N20: the outside recording's face reaches the service, its level at most every 66 ms.
        let mut r = Rig::on_with_model(false);
        let v = r.d.ctl.view().clone();
        assert_eq!(LEVEL_EVERY_MS, 66.0);
        r.at(1000.0);
        r.d.send_live(&v, false);
        assert_eq!(r.d.live_sent_ms, 1000.0, "the first look goes at once");
        r.at(1065.0);
        r.d.send_live(&v, false);
        assert_eq!(
            r.d.live_sent_ms, 1000.0,
            "65 ms after the last send it is held back"
        );
        r.at(1066.0);
        r.d.send_live(&v, false);
        assert_eq!(
            r.d.live_sent_ms, 1066.0,
            "66 ms after the last send it goes"
        );
        r.at(1067.0);
        r.d.send_live(&v, true);
        assert_eq!(
            r.d.live_sent_ms, 1067.0,
            "a forced send (a phase change) is never held back"
        );
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

    /// Review #84: the words go to the tab that was in front when the recording began, not the one in front
    /// when they come back.
    #[test]
    fn the_words_land_in_the_tab_the_recording_began_in() {
        let mut r = Rig::on_with_model(false);
        r.d.run_svc(vec![SvcEffect::NoteFront]);
        *r.host.front.borrow_mut() = Some("t2".into());
        r.d.run_svc(vec![SvcEffect::PastePage("hello".into())]);
        let pastes = r.host.pastes.borrow().clone();
        assert_eq!(
            pastes.last().map(|p| p.0.as_str()),
            Some("t1"),
            "{pastes:?}"
        );
    }

    /// Review #84: a tab that closed before the words came back is not typed into, and the paste says so.
    #[test]
    fn a_tab_closed_during_transcription_is_not_typed_into() {
        let mut r = Rig::on_with_model(false);
        r.d.run_svc(vec![SvcEffect::NoteFront]);
        r.host.paste_outcome.set(Some(PasteOutcome::Gone));
        r.d.run_svc(vec![SvcEffect::PastePage("hello".into())]);
        assert!(r.d.paste_gone, "the gone tab must be noticed");
        assert!(r.host.pastes.borrow().is_empty());
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

    /// Audit L43: switching dictation off while the microphone is still opening must end the recording at once.
    /// Before, the controller kept a recording nobody could finish (the late open is dropped), so the widget stayed
    /// up and the paused media stayed paused until restart.
    #[test]
    fn switching_off_while_the_mic_is_opening_ends_the_recording_and_resumes_media() {
        let mut r = Rig::on_with_model(false);
        r.set("flare.dictation.pauseMedia", "1");
        r.d.apply_prefs();
        r.at(1000.0);
        r.key("AltRight", false);
        r.at(1210.0);
        r.d.on_timer(TimerKey::WindowKey);
        assert_eq!(r.count("pause_media"), 1);
        r.pump();
        assert!(
            r.d.view.active,
            "the recording has begun, the mic is opening"
        );
        assert!(r.s.done.borrow().is_some(), "the open has not answered");
        r.set("flare.dictation.enabled", "0");
        r.d.apply_prefs();
        assert!(!r.d.view.active, "the recording is over, not stuck");
        assert_eq!(
            r.count("resume_media"),
            1,
            "what dictation paused plays again"
        );
        // The late open is let go and changes nothing.
        let done = r.s.done.borrow_mut().take().unwrap();
        done(Ok(Box::new(FakeCapture)));
        r.pump();
        assert!(r.d.mic.cap.is_none());
        assert!(!r.d.view.active);
    }
}

/// The Dictation page's controls (M8 Task 25): the key field's capture, the models' actions, the microphone's Test.
#[cfg(test)]
mod page_tests {
    use super::fixture::{fake_store, install_store, pump_until};
    use super::*;
    use flare_settings::kv::PrefStore;
    use std::sync::atomic::AtomicUsize;

    struct Page {
        prefs: PrefStore,
        calls: Arc<AtomicUsize>,
        refreshes: Rc<Cell<u32>>,
        _dir: tempfile::TempDir,
    }

    /// A page over a memory store of preferences and a store that downloads "hello" for every model.
    fn page(urls: [&str; 3]) -> Page {
        crate::ui_harness::ensure_platform();
        let (store, dir, calls) = fake_store(urls);
        install_store(store);
        let prefs = PrefStore::memory();
        let refreshes = Rc::new(Cell::new(0));
        let r = Rc::clone(&refreshes);
        register_page(
            prefs.clone(),
            Rc::new(move || r.set(r.get() + 1)),
            Rc::new(|_| {}),
        );
        Page {
            prefs,
            calls,
            refreshes,
            _dir: dir,
        }
    }

    fn key(code: &str) -> KeyEvent {
        KeyEvent::new(code, code)
    }

    fn feed(ev: &KeyEvent, up: bool) -> bool {
        DictationKeys.feed_key(ev, up, &|| None)
    }

    fn stored_key(p: &Page) -> String {
        let kv = |k: &str| p.prefs.get(k);
        flare_dictation::hotkey::format_hotkey(&stored_prefs(&kv).hotkey)
    }

    fn start_capture_field() {
        assert!(page_button("dictation-hotkey", "capture"));
        assert!(page_snap().capturing);
    }

    #[test]
    fn capture_commits_a_bare_modifier_on_release_and_a_chord_on_down() {
        let p = page(["a", "b", "c"]);
        // Ctrl, then Win with Ctrl held: the chord of both, committed when Win comes up (N3).
        start_capture_field();
        assert!(feed(&key("ControlLeft").ctrl(), false));
        assert_eq!(
            stored_key(&p),
            "Ctrl+Win",
            "nothing is stored while keys are down (default is Ctrl+Win)"
        );
        assert!(p.prefs.get(prefs::KEYS[2]).is_none());
        assert!(feed(&key("MetaLeft").ctrl().meta(), false));
        assert!(
            p.prefs.get(prefs::KEYS[2]).is_none(),
            "a modifier is taken on its release"
        );
        assert!(feed(&key("MetaLeft").ctrl().meta(), true));
        assert_eq!(stored_key(&p), "Ctrl+Win");
        assert!(!page_snap().capturing, "the capture ended");
        // A lone Right Alt (with AltGr's Ctrl riding on it) is bare Right Alt, on its release.
        start_capture_field();
        assert!(feed(&key("AltRight").ctrl().alt(), false));
        assert_eq!(stored_key(&p), "Ctrl+Win");
        assert!(feed(&key("AltRight").ctrl().alt(), true));
        assert_eq!(stored_key(&p), "Right Alt");
        // A chord with a key that is not a modifier is committed on its down.
        start_capture_field();
        assert!(feed(&key("ControlLeft").ctrl(), false));
        assert!(feed(&key("AltLeft").ctrl().alt(), false));
        assert!(feed(&key("KeyD").ctrl().alt(), false));
        assert_eq!(stored_key(&p), "Ctrl+Alt+D");
        assert!(!page_snap().capturing);
        // Its releases then belong to nobody here: the capture is over, the key goes on.
        assert!(!feed(&key("KeyD").ctrl().alt(), true));
    }

    #[test]
    fn altgr_reads_as_right_alt() {
        let p = page(["a", "b", "c"]);
        start_capture_field();
        // On a Norwegian keyboard Right Alt is AltGr: Ctrl and Alt flags with it, and a Ctrl down before it.
        assert!(feed(&key("ControlLeft").ctrl(), false));
        assert!(feed(&key("AltRight").ctrl().alt(), false));
        assert!(feed(&key("AltRight").ctrl().alt(), true));
        assert_eq!(stored_key(&p), "Right Alt");
    }

    #[test]
    fn escape_cancels_capture() {
        let p = page(["a", "b", "c"]);
        start_capture_field();
        assert!(feed(&key("Escape"), false));
        assert!(!page_snap().capturing);
        assert!(
            p.prefs.get(prefs::KEYS[2]).is_none(),
            "Escape is never a binding"
        );
        assert!(p.refreshes.get() > 0, "the page is drawn again");
    }

    #[test]
    fn a_press_outside_and_a_blur_cancel_capture_and_the_fields_own_click_does_not_reopen_it() {
        let p = page(["a", "b", "c"]);
        start_capture_field();
        let drawn = p.refreshes.get();
        on_press();
        assert!(!page_snap().capturing, "a press elsewhere ends the capture");
        assert!(
            p.refreshes.get() > drawn,
            "the field is drawn again, not left saying Press a key"
        );
        // The press was on the field itself: its click follows and must not wait again.
        assert!(page_button("dictation-hotkey", "capture"));
        assert!(!page_snap().capturing);
        // The next press and click on the field start it again.
        on_press();
        assert!(page_button("dictation-hotkey", "capture"));
        assert!(page_snap().capturing);
        on_blur();
        assert!(!page_snap().capturing, "a window blur ends it too");
        // A press while nothing waits draws nothing.
        let drawn = p.refreshes.get();
        on_press();
        assert_eq!(p.refreshes.get(), drawn);
        assert!(p.prefs.get(prefs::KEYS[2]).is_none());
    }

    #[test]
    fn an_unusable_key_keeps_capture_open() {
        let p = page(["a", "b", "c"]);
        start_capture_field();
        // A letter would be typed away by the terminal: it is refused and the field keeps listening.
        assert!(feed(&key("KeyA"), false));
        assert!(feed(&key("KeyA"), true));
        assert!(page_snap().capturing);
        assert!(p.prefs.get(prefs::KEYS[2]).is_none());
        // Ctrl+C is the terminal's.
        assert!(feed(&key("KeyC").ctrl(), false));
        assert!(page_snap().capturing);
        // A function key is fine.
        assert!(feed(&key("F8"), false));
        assert!(!page_snap().capturing);
        assert_eq!(stored_key(&p), "F8");
    }

    #[test]
    fn reset_writes_the_default_and_a_waiting_field_takes_no_other_key_away() {
        let p = page(["a", "b", "c"]);
        // Not waiting: keys are not the field's.
        assert!(!feed(&key("KeyA"), false));
        assert!(page_button("dictation-hotkey", "reset"));
        assert_eq!(stored_key(&p), "Ctrl+Win");
    }

    #[test]
    fn the_first_downloaded_model_becomes_active_unless_one_is_and_removing_it_clears_it() {
        let p = page(["a", "b", "c"]);
        assert!(page_button("base", "download"));
        assert_eq!(
            run_view("base", 5).0,
            Some(0),
            "the row shows a download under way"
        );
        pump_until("the first model", || active_model() == "base");
        assert_eq!(run_view("base", 5), (None, None));
        let s = page_store().unwrap();
        assert!(
            s.model_path("base").is_some(),
            "the file is in place and verified"
        );
        // A second one finishing never replaces the one in use.
        assert!(page_button("small", "download"));
        pump_until("the second model", || {
            page_store().unwrap().model_path("small").is_some() && run_view("small", 5).0.is_none()
        });
        assert_eq!(active_model(), "base");
        // Use picks it.
        assert!(page_button("small", "use"));
        assert_eq!(active_model(), "small");
        // Removing the active model clears the stored id.
        assert!(page_button("small", "uninstall"));
        pump_until("the removal", || active_model().is_empty());
        assert!(page_store().unwrap().model_path("small").is_none());
        assert_eq!(p.prefs.get(prefs::KEYS[7]).as_deref(), Some(""));
        let _ = p.calls.load(Ordering::SeqCst);
    }

    #[test]
    fn remove_cancels_a_download_under_way_and_leaves_nothing() {
        let p = page(["stall", "b", "c"]);
        assert!(page_button("base", "download"));
        assert!(run_view("base", 5).0.is_some());
        // Uninstall while it runs: the download is cancelled and the files go.
        assert!(page_button("base", "uninstall"));
        pump_until("the end of the download", || {
            run_view("base", 5).0.is_none()
        });
        pump_until("the removal", || {
            runs().get("base").is_none() && page_store().unwrap().model_path("base").is_none()
        });
        assert!(active_model().is_empty(), "nothing became active");
        let _ = &p;
    }

    #[test]
    fn a_model_that_does_not_support_every_language_asks_once_and_a_retry_does_not_ask() {
        let p = page(["a", "b", "fail"]);
        // The question first: nothing downloads while it asks.
        assert!(page_button("parakeet-v3", "download"));
        assert_eq!(page_snap().ask.as_deref(), Some("parakeet-v3"));
        assert_eq!(p.calls.load(Ordering::SeqCst), 0);
        assert_eq!(run_view("parakeet-v3", 5).0, None);
        // Cancel puts it away and downloads nothing.
        assert!(page_ask_answer(false));
        assert!(page_snap().ask.is_none());
        assert_eq!(p.calls.load(Ordering::SeqCst), 0);
        // Download starts the download; this one fails.
        assert!(page_button("parakeet-v3", "download"));
        assert!(page_ask_answer(true));
        pump_until("the failure", || run_view("parakeet-v3", 5).1.is_some());
        assert_eq!(run_view("parakeet-v3", 5).1, Some("The download failed."));
        // Retry has already been asked: it goes straight to the download.
        assert!(page_button("parakeet-v3", "download"));
        assert!(page_snap().ask.is_none(), "a retry does not ask again");
        assert!(run_view("parakeet-v3", 5).0.is_some());
        // A Whisper model never asks.
        assert!(page_button("base", "download"));
        assert!(page_snap().ask.is_none());
        pump_until("whisper", || active_model() == "base");
    }

    #[test]
    fn a_cancelled_download_says_nothing_and_download_ids_come_only_from_the_catalogue() {
        let p = page(["stall", "b", "c"]);
        assert!(page_button("base", "download"));
        assert!(page_button("base", "cancel"));
        pump_until("the cancelled end", || run_view("base", 5).0.is_none());
        assert_eq!(run_view("base", 5), (None, None), "a cancel is no failure");
        // Anything that is not a catalogue id is nobody's.
        let before = p.calls.load(Ordering::SeqCst);
        assert!(!page_button("../x", "download"));
        assert!(!page_button("dictation-model", "download"));
        download_item("../x");
        download_item("");
        assert_eq!(p.calls.load(Ordering::SeqCst), before);
        assert!(!owns_row("../x"));
        assert!(owns_row("dictation-model") && owns_row("gpu-pack") && owns_row("base"));
    }

    /// Audit L47: the end of a cancelled flight arrives after the same model was asked for again.
    #[test]
    fn the_end_of_an_old_flight_does_not_mark_the_new_download_inactive() {
        let p = page(["stall", "b", "c"]);
        assert!(page_button("base", "download"));
        let old = runs().get("base").unwrap().flight;
        // Cancel, then Retry before the old end has been delivered.
        assert!(page_button("base", "cancel"));
        assert!(page_button("base", "download"));
        let new = runs().get("base").unwrap().flight;
        assert_ne!(old, new, "each download is its own flight");
        finish_download("base", old, Err(flare_dictation::store::Failure::Cancelled));
        assert!(
            run_view("base", 5).0.is_some(),
            "the new download is still running"
        );
        assert!(runs().get("base").unwrap().failure.is_none());
        // Its own end is heard.
        finish_download("base", new, Err(flare_dictation::store::Failure::Network));
        assert_eq!(run_view("base", 5), (None, Some("The download failed.")));
        assert!(page_button("base", "cancel"));
        let _ = &p;
    }

    /// Audit L46: a "not installed" answer is not asked of the disk again at once, and a download ending clears it.
    #[test]
    fn a_missing_model_is_not_checked_again_until_something_changes() {
        let _p = page(["a", "b", "c"]);
        // At rest every build looks at the disk (a model put in place by hand shows at once).
        let mut rest = 0;
        for _ in 0..3 {
            installed_for_page("base", || {
                rest += 1;
                false
            });
        }
        assert_eq!(rest, 3);
        // While a download runs, its progress rebuilds the page up to ten times a second.
        runs().insert(
            "small".into(),
            Run {
                active: true,
                ..Run::default()
            },
        );
        let mut checks = 0;
        for _ in 0..10 {
            assert!(!installed_for_page("base", || {
                checks += 1;
                false
            }));
        }
        assert_eq!(checks, 1, "ten page builds, one file check");
        // A yes is never remembered here (the store has its own memo for it).
        assert!(installed_for_page("small", || true));
        assert!(!absent_recently("small"));
        // A download ending forgets the no.
        finish_download("base", 0, Ok(()));
        assert!(!absent_recently("base"));
        assert!(installed_for_page("base", || true));
    }

    #[test]
    fn nothing_changes_without_a_store() {
        let p = page(["a", "b", "c"]);
        TEST_STORE.with(|s| *s.borrow_mut() = None);
        // `store()` is None too (no controller exists in this test and `install` never ran): a press does nothing.
        assert!(page_button("base", "download"));
        assert!(runs().is_empty());
        assert_eq!(p.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn one_store_serves_every_switch_on_so_a_download_can_still_be_cancelled() {
        // The store is made once for the process: a second switch-on gets the same one, so the Cancel of a download
        // that began before a switch-off reaches that download (F1).
        let cell = std::sync::OnceLock::new();
        let (first, _dir, _calls) = fixture::fake_store(["stall", "stall", "stall"]);
        let a = shared_store(&cell, || first);
        let b = shared_store(&cell, || panic!("a second store was made"));
        let rx = a.download("base", Box::new(|_| {}));
        std::thread::sleep(Duration::from_millis(50));
        b.cancel("base");
        let end = rx.recv_timeout(Duration::from_secs(5)).expect("cancelled");
        assert_eq!(end, Err(flare_dictation::store::Failure::Cancelled));
    }
    #[test]
    fn the_switches_and_the_choices_write_their_own_keys() {
        let p = page(["a", "b", "c"]);
        assert!(page_switch("dictation-enabled", true));
        assert_eq!(p.prefs.get(prefs::KEYS[0]).as_deref(), Some("1"));
        assert!(page_switch("dictation-pause-media", true));
        assert_eq!(p.prefs.get(prefs::KEYS[5]).as_deref(), Some("1"));
        assert!(page_switch("dictation-sounds", false));
        assert_eq!(p.prefs.get(prefs::KEYS[6]).as_deref(), Some("0"));
        assert!(page_switch("dictation-anywhere", false));
        assert_eq!(p.prefs.get(prefs::KEYS[8]).as_deref(), Some("0"));
        assert!(page_choose("dictation-mode", "toggle"));
        assert_eq!(p.prefs.get(prefs::KEYS[1]).as_deref(), Some("toggle"));
        assert!(!page_choose("dictation-mode", "sideways"));
        assert!(page_choose("dictation-mic", "mic-9"));
        assert_eq!(p.prefs.get(prefs::KEYS[3]).as_deref(), Some("mic-9"));
        assert!(page_choose("dictation-language", "no"));
        assert_eq!(p.prefs.get(prefs::KEYS[4]).as_deref(), Some("no"));
        assert!(!page_choose("dictation-language", "klingon"));
        assert_eq!(p.prefs.get(prefs::KEYS[4]).as_deref(), Some("no"));
        // Off: nothing waits, listens or asks.
        start_capture_field();
        PAGE.with(|q| q.borrow_mut().ask = Some("parakeet-v3".into()));
        assert!(page_switch("dictation-enabled", false));
        assert!(!page_snap().capturing && page_snap().ask.is_none());
        assert_eq!(p.prefs.get(prefs::KEYS[0]).as_deref(), Some("0"));
    }

    #[test]
    fn under_e2e_an_unset_mic_seam_fails_closed_never_the_real_microphone() {
        // Audit L54. Under --e2e no seam (or an empty one) is a path that cannot open.
        assert!(
            matches!(mic_source_for(true, None), MicSource::File(p) if p.as_os_str().is_empty())
        );
        assert!(matches!(
            mic_source_for(true, Some(String::new())),
            MicSource::File(_)
        ));
        // The seam's own WAV still plays; without --e2e the device is the answer.
        assert!(
            matches!(mic_source_for(true, Some("a.wav".into())), MicSource::File(p) if p == Path::new("a.wav"))
        );
        assert!(matches!(mic_source_for(false, None), MicSource::Device));
    }

    /// A WAV of a tenth of a second of silence, in the temp folder.
    fn silent_wav(dir: &Path) -> PathBuf {
        let path = dir.join("silence.wav");
        std::fs::write(
            &path,
            flare_dictation::wav::encode_wav(&vec![0.0; 1600], 16_000),
        )
        .unwrap();
        path
    }

    #[test]
    fn the_page_never_opens_the_mic() {
        // 1. Nothing the page builds or reads touches a microphone: no source file of the page calls the capture
        //    or lists a device on the UI thread (the listing is a worker's).
        for (name, src) in [
            ("dictation_page.rs", include_str!("dictation_page.rs")),
            ("settings_ctl.rs", include_str!("settings_ctl.rs")),
        ] {
            let code = src.split("#[cfg(test)]").next().unwrap();
            for banned in ["start_capture", "open_mic(", "list_mics("] {
                assert!(!code.contains(banned), "{name} calls {banned}");
            }
        }
        // 2. At run time: reading the page's state and the controls that are not Test open none.
        let p = page(["a", "b", "c"]);
        let dir = tempfile::tempdir().unwrap();
        TEST_MIC.with(|m| *m.borrow_mut() = Some(MicSource::File(silent_wav(dir.path()))));
        let before = mic::opens_on_this_thread();
        let _ = page_snap();
        let _ = page_mics();
        page_shown();
        page_left();
        assert!(page_button("dictation-hotkey", "capture"));
        assert!(page_button("dictation-hotkey", "capture"));
        assert!(page_choose("dictation-mic", "x"));
        assert!(page_switch("dictation-enabled", true));
        assert_eq!(
            mic::opens_on_this_thread(),
            before,
            "nothing opened a microphone"
        );
        // 3. The Test button opens it, once, and Stop lets go.
        assert!(page_button("dictation-mic", "test"));
        assert_eq!(mic::opens_on_this_thread(), before + 1);
        assert!(page_snap().mic_testing);
        assert!(
            crate::timer_registry::running(TEST_TICK) && crate::timer_registry::running(TEST_STOP)
        );
        assert!(page_button("dictation-mic", "test"));
        assert!(!page_snap().mic_testing);
        assert!(
            !crate::timer_registry::running(TEST_TICK),
            "the meter timer is gone"
        );
        assert_eq!(mic::opens_on_this_thread(), before + 1);
        let _ = &p;
    }

    #[test]
    fn a_refused_microphone_says_so_and_another_failure_says_none_was_found() {
        let p = page(["a", "b", "c"]);
        let dir = tempfile::tempdir().unwrap();
        TEST_MIC.with(|m| *m.borrow_mut() = Some(MicSource::File(silent_wav(dir.path()))));
        for (failure, said) in [
            (CaptureFailure::Denied, "Microphone access was refused."),
            (CaptureFailure::NoDevice, "No working microphone was found."),
            (CaptureFailure::Failed, "No working microphone was found."),
        ] {
            assert!(page_button("dictation-mic", "test"));
            let epoch = PAGE.with(|q| q.borrow().test_epoch);
            test_opened(epoch, Err(failure));
            let snap = page_snap();
            assert!(
                !snap.mic_testing,
                "a microphone that will not open ends the Test"
            );
            assert_eq!(snap.mic_problem.as_deref(), Some(said));
        }
        // A new Test clears the problem; an answer of an older Test is let go of.
        assert!(page_button("dictation-mic", "test"));
        assert!(page_snap().mic_problem.is_none());
        let old = PAGE.with(|q| q.borrow().test_epoch) - 1;
        struct Gone(Arc<AtomicUsize>);
        impl Capture for Gone {
            fn stop(self: Box<Self>) -> Vec<f32> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Vec::new()
            }
        }
        let stopped = Arc::new(AtomicUsize::new(0));
        test_opened(old, Ok(Box::new(Gone(Arc::clone(&stopped)))));
        assert_eq!(
            stopped.load(Ordering::SeqCst),
            1,
            "an old answer's microphone is let go at once"
        );
        assert!(
            page_snap().mic_testing,
            "and the current Test is not disturbed"
        );
        page_left();
        assert!(!page_snap().mic_testing, "leaving the page ends the Test");
        let _ = &p;
    }

    #[test]
    fn the_meter_rises_at_once_and_falls_by_85_15() {
        let p = page(["a", "b", "c"]);
        let seen = Rc::new(Cell::new(-1.0_f32));
        let s = Rc::clone(&seen);
        register_page(p.prefs.clone(), Rc::new(|| {}), Rc::new(move |l| s.set(l)));
        let peak = Arc::new(std::sync::atomic::AtomicU32::new(0.8_f32.to_bits()));
        PAGE.with(|q| {
            let mut q = q.borrow_mut();
            q.mic_testing = true;
            q.test_peak = Some(Arc::clone(&peak));
        });
        meter_tick();
        assert!(
            (seen.get() - 0.8).abs() < 1e-6,
            "up at once: {}",
            seen.get()
        );
        meter_tick();
        assert!(
            (seen.get() - 0.8 * 0.85).abs() < 1e-6,
            "down by 85/15: {}",
            seen.get()
        );
        peak.store(2.0_f32.to_bits(), Ordering::SeqCst);
        meter_tick();
        assert!((seen.get() - 1.0).abs() < 1e-6, "never past full");
    }
}

/// What the Dictation page's tests share: a store that downloads "hello" and a clean page state.
#[cfg(test)]
pub(crate) mod fixture {
    use super::*;
    use flare_dictation::store::{Failure, Fetch, Item, Kind, StoreConfig};
    use std::sync::atomic::AtomicUsize;

    /// The SHA-256 of "hello".
    const HELLO: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

    /// Serves "hello", except for the urls that stall (until cancelled) or fail.
    struct Fake {
        calls: Arc<AtomicUsize>,
    }

    impl Fetch for Fake {
        fn get(&self, url: &str, sink: &mut dyn FnMut(&[u8]) -> bool) -> Result<(), Failure> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if url.contains("fail") {
                return Err(Failure::Network);
            }
            if url.contains("stall") {
                loop {
                    std::thread::sleep(Duration::from_millis(5));
                    if !sink(&[]) {
                        return Err(Failure::Cancelled);
                    }
                }
            }
            if sink(b"hello") {
                Ok(())
            } else {
                Err(Failure::Cancelled)
            }
        }
    }

    fn item(id: &str, url: &str) -> Item {
        Item {
            id: id.into(),
            url: url.into(),
            bytes: 5,
            sha256: HELLO.into(),
            kind: Kind::Model,
        }
    }

    /// A store of three catalogue ids (`base`, `small`, `parakeet-v3`) that downloads "hello" for each, from the
    /// urls given (a url with "stall" in it never ends, one with "fail" fails). Also the temp folder it lives in
    /// and the count of fetches made.
    pub(crate) fn fake_store(urls: [&str; 3]) -> (Store, tempfile::TempDir, Arc<AtomicUsize>) {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let store = Store::with_config(StoreConfig {
            root: dir.path().join("FlareDictation"),
            fetch: Box::new(Fake {
                calls: Arc::clone(&calls),
            }),
            catalog: vec![
                item("base", urls[0]),
                item("small", urls[1]),
                item("parakeet-v3", urls[2]),
            ],
            now: None,
            unzip: None,
        })
        .unwrap();
        (store, dir, calls)
    }

    static TEST_QUEUE_LOCK: Mutex<()> = Mutex::new(());

    thread_local! {
        static TEST_QUEUE_GUARD: RefCell<Option<std::sync::MutexGuard<'static, ()>>> = const { RefCell::new(None) };
    }

    /// Holds the lock on the process-wide page state (the posted-work queue and the download runs) until this test's
    /// thread ends. A scene test that stages a download takes it too, or a page test clearing the runs under it makes
    /// the scene flaky.
    pub(crate) fn hold_page_state() {
        TEST_QUEUE_GUARD.with(|g| {
            let mut g = g.borrow_mut();
            if g.is_none() {
                *g = Some(
                    TEST_QUEUE_LOCK
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner),
                );
            }
        });
    }

    /// Puts `store` in place of the real one and the page's state back to nothing.
    pub(crate) fn install_store(store: Store) {
        // The posted-work queue is one process-wide list, so two tests that use it at once would run (and clear)
        // each other's posts. A test that installs a store holds this lock until its thread ends.
        hold_page_state();
        TEST_STORE.with(|s| *s.borrow_mut() = Some(store));
        TEST_MIC.with(|m| *m.borrow_mut() = None);
        TEST_POSTS.lock().unwrap().clear();
        runs().clear();
        PAGE.with(|p| *p.borrow_mut() = PageUi::default());
    }

    /// Runs what the workers posted to the UI thread until `done` holds (or five seconds pass).
    pub(crate) fn pump_until(what: &str, done: impl Fn() -> bool) {
        let end = Instant::now() + Duration::from_secs(5);
        while !done() {
            let next = TEST_POSTS.lock().unwrap().pop_front();
            match next {
                Some(f) => f(),
                None => std::thread::sleep(Duration::from_millis(5)),
            }
            assert!(Instant::now() < end, "timed out waiting for {what}");
        }
    }

    /// The active model as stored.
    pub(crate) fn model_now() -> String {
        active_model()
    }

    /// The microphones the page lists.
    pub(crate) fn set_mics(mics: &[(&str, &str)]) {
        PAGE.with(|p| {
            p.borrow_mut().mics = Some(
                mics.iter()
                    .map(|(a, b)| ((*a).to_string(), (*b).to_string()))
                    .collect(),
            );
        });
    }

    /// A tenth of a second of silence as the microphone (a file, never the device).
    pub(crate) fn silent_mic(dir: &Path) {
        let path = dir.join("silence.wav");
        std::fs::write(
            &path,
            flare_dictation::wav::encode_wav(&[0.0; 1600], 16_000),
        )
        .unwrap();
        TEST_MIC.with(|m| *m.borrow_mut() = Some(MicSource::File(path)));
    }
}
