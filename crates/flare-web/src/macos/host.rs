//! The web panel's browser on macOS: CEF (`flare_cef`), windowless, behind the same [`WebBackend`] seam as the
//! Windows WebView2 host (port phase 5). One CEF browser per slot (a tab's panel, a plugin's bar or background
//! page), each drawn into a picture the window shows where the page sits ([`WebBackend::frame`]) and fed the
//! window's pointer, keys and input method ([`WebBackend::pointer`], [`WebBackend::key`], [`WebBackend::ime`]).
//!
//! The shape is the Windows host's: nothing waits for the browser. The first view asks the runtime question
//! (`WebEvent::Runtime`: is the framework in this app's bundle), the engine starts when it is answered, a view's
//! profile is made and initialized before its browser, and a browser navigates only once its own setup has answered
//! (the Page domain, the script that runs before the page's, a mod's binding), as a mini-app waits for its script on
//! Windows. CEF's handlers only queue ([`flare_cef::macos::view::ViewEvent`] into the inbox); [`WebBackend::pump`]
//! takes the queue in and tells the window. CEF asks for its turns itself (the external message pump): a scheduler
//! thread sends `WebEvent::Pump` when one is due, so the window's loop stays winit's.
//!
//! The walls are the Windows ones, decided by [`crate::walls`]: the web panel goes to http, https and about:blank, a
//! page's new window loads in the same view, a download waits for the person's yes, permissions are denied, a
//! script dialog is answered at once and never shown; a mini-app runs in the apps' private profile with its folder
//! served on its own host and every other request refused (a dead proxy behind that, the WebRTC policy and the
//! peer-connection script in front); a mod's or plugin's page is sealed as its kind says ([`crate::seal`]). CEF
//! cannot be started again in a process once it was shut down, so the engine stays up from the first view to the end
//! of the window (Windows lets its environment go with the last view): it is the process's ([`Cef`]), started by the
//! first host and adopted by every later one (the window lets an idle host go and makes a new one for the next
//! panel), and shut down once, by [`WebBackend::close_all`] or [`end_engine`] at quit (rule 1156).

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use flare_cef::macos::context::{Context, Profile};
use flare_cef::macos::engine::{Engine, EngineConfig, Schedule};
use flare_cef::macos::view::{
    Answer, Button, Downloads, EventSink, Key, KeyKind, Made, NavRule, NewWindow, NewWindowRule,
    NewWindows, Options, PopupSpec, RequestRule, View, ViewEvent, ViewSize, Wake, Walls, WindowAsk,
};
use flare_cef::status::{self, flags};
use flare_surfaces::app::AppSpec;
use flare_surfaces::modview;
use flare_surfaces::web::{
    AppLinkDedupe, ExternalScheme, PhysRect, PopupPlan, PopupRefusal, external_scheme, host_label,
    load_failure_logged, navigation_allowed, own_blob, pdf_view, permission_word, popup_plan,
    site_of, web_error_name,
};

use crate::backend::{
    ControllerProbe, ImeInput, KeyFilter, KeyInput, KeyPress, Mods, Nav, NoteKind, Pointer,
    PointerAct, PointerButton, PopupProbe, Sink, Slot, ViewFrame, ViewSink, WebBackend, WebEvent,
    WebNote,
};
use crate::popups::PopupBook;
use crate::seal::{DownloadWall, plugin_seal};
use crate::walls::{self, BINDING, DEAD_PROXY, Decision, Kind, NO_BYPASS};

/// How soon the host turns again while a view waits for CEF ([`WebHost::retry_while_waiting`]).
const WAITING_TURN: Duration = Duration::from_millis(20);

/// The longest message a mod's page may post when its plugin has no budget (the Windows host's).
const MAX_PAGE_MESSAGE: usize = 4 * 1024 * 1024;

/// Is a browser runtime there? The version string, or why not (the same shape as on Windows).
pub type RuntimeCheck = Arc<dyn Fn() -> Result<String, String> + Send + Sync>;

/// The real check: the CEF framework is in this app's bundle (`Contents/Frameworks`), where the dev bundle and the
/// release app put it. A plain `cargo run` of `flare` has none, and its web panel says so.
pub fn installed_runtime() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| format!("no executable path: {e}"))?;
    match flare_cef::macos::engine::framework_of(&exe) {
        Some(_) => Ok(format!(
            "CEF {} (Chromium {})",
            flare_cef::pin::CEF_VERSION,
            flare_cef::pin::CHROMIUM_VERSION
        )),
        None => Err(format!(
            "this build of Flare has no web engine: {} is not in its app bundle",
            flare_cef::pin::FRAMEWORK
        )),
    }
}

/// The real app's first step on macOS, before winit or anything else touches `NSApp`: the shared application is
/// made CEF's kind (`CefAppProtocol`, `flare_cef::macos::app::install_for_app`), so a web panel can start the engine
/// later. The activation policy, the windows and quitting stay winit's. Not on the main thread: an error.
pub fn prepare_app() -> Result<(), String> {
    let mtm = objc2::MainThreadMarker::new().ok_or("the app is prepared on the main thread")?;
    flare_cef::macos::app::install_for_app(mtm).map(|_| ())
}

/// What the host is told at the start (the same fields as on Windows).
pub struct HostConfig {
    pub parent: isize,
    pub data_dir: PathBuf,
    pub sink: Sink,
    pub runtime: RuntimeCheck,
    pub virtual_hosts: Vec<(String, PathBuf)>,
    pub without_webrtc_script: bool,
    pub limits: Option<Arc<flare_mods::limit::Limits>>,
}

/// CEF's turns: a thread that waits for the earliest turn CEF asked for and then tells the window (the host that
/// holds the engine now, [`Scheduler::point_to`]). It ends when the scheduler is dropped (rule 1157).
struct Scheduler {
    due: Arc<(Mutex<Option<Instant>>, Condvar)>,
    sink: Arc<Mutex<Sink>>,
    stop: Arc<AtomicBool>,
}

impl Scheduler {
    fn start(sink: Sink) -> Scheduler {
        let due: Arc<(Mutex<Option<Instant>>, Condvar)> = Arc::default();
        let sink = Arc::new(Mutex::new(sink));
        let stop: Arc<AtomicBool> = Arc::default();
        let (d, to, end) = (Arc::clone(&due), Arc::clone(&sink), Arc::clone(&stop));
        let _ = std::thread::Builder::new()
            .name("flare-cef-pump".into())
            .spawn(move || {
                let (m, cv) = &*d;
                let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
                loop {
                    // Read under the lock `Drop` takes before it wakes this thread: never missed.
                    if end.load(Ordering::SeqCst) {
                        return;
                    }
                    match *g {
                        // sync-ok: the pump scheduler's own thread waits for the next turn, never the UI thread
                        None => g = cv.wait(g).unwrap_or_else(|e| e.into_inner()),
                        Some(at) => {
                            let now = Instant::now();
                            if at <= now {
                                *g = None;
                                drop(g);
                                let sink = Arc::clone(&*lock(&to));
                                sink(WebEvent::Pump);
                                g = m.lock().unwrap_or_else(|e| e.into_inner());
                            } else {
                                g = cv
                                    // sync-ok: the pump scheduler's own thread, until the turn is due
                                    .wait_timeout(g, at - now)
                                    .unwrap_or_else(|e| e.into_inner())
                                    .0;
                            }
                        }
                    }
                }
            });
        Scheduler { due, sink, stop }
    }

    /// The turns go to this host from now on (a new host adopted the engine).
    fn point_to(&self, sink: &Sink) {
        *lock(&self.sink) = Arc::clone(sink);
    }

    fn schedule(&self) -> Schedule {
        let d = Arc::clone(&self.due);
        Arc::new(move |ms: i64| {
            let at = Instant::now() + Duration::from_millis(u64::try_from(ms).unwrap_or(0));
            let (m, cv) = &*d;
            let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
            if g.is_none_or(|t| at < t) {
                *g = Some(at);
                cv.notify_one();
            }
        })
    }
}

impl Drop for Scheduler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let (m, cv) = &*self.due;
        let _g = lock(m);
        cv.notify_all();
    }
}

/// The process's CEF (rule 1156): CEF starts once in a process and cannot start again after it shut down, so the
/// engine, its turns and the browsers still closing outlive any one host. On the main thread only, as CEF is.
struct Cef {
    engine: Engine,
    scheduler: Scheduler,
    /// Browsers a host that went away had closed and CEF has not finished closing: their last turns come from the
    /// next host or from the shutdown.
    closing: RefCell<Vec<View>>,
}

impl std::ops::Deref for Cef {
    type Target = Engine;
    fn deref(&self) -> &Engine {
        &self.engine
    }
}

enum ProcessCef {
    None,
    Up(Rc<Cef>),
    /// Shut down: nothing opens again in this process.
    Ended,
}

thread_local! {
    static CEF: RefCell<ProcessCef> = const { RefCell::new(ProcessCef::None) };
}

static ENGINE_STARTS: AtomicUsize = AtomicUsize::new(0);
static ENGINE_SHUTDOWNS: AtomicUsize = AtomicUsize::new(0);

/// How many times this process started CEF (`Engine::start`) and shut it down: at most once each (the tests).
pub fn engine_starts() -> (usize, usize) {
    (
        ENGINE_STARTS.load(Ordering::SeqCst),
        ENGINE_SHUTDOWNS.load(Ordering::SeqCst),
    )
}

/// The window is ending: the process's CEF shuts down if it is up, after the browsers still closing finished (the
/// cookies and the profiles on disk are written then). For the quit with no host left (the last panel closed and
/// the window let its host go); [`WebBackend::close_all`] ends it too. Nothing opens again afterwards.
pub fn end_engine() {
    let Some(taken) = CEF
        .try_with(|c| std::mem::replace(&mut *c.borrow_mut(), ProcessCef::Ended))
        .ok()
    else {
        return;
    };
    let ProcessCef::Up(cef) = taken else {
        return;
    };
    let Ok(Cef {
        engine,
        scheduler,
        closing,
    }) = Rc::try_unwrap(cef)
    else {
        log::warn!(
            "the web engine is still in use at the end: left running until the process ends"
        );
        return;
    };
    let closing = closing.into_inner();
    // CEF closes a browser asynchronously: its last turns, then the engine goes.
    let end = Instant::now() + Duration::from_secs(3);
    while Instant::now() < end && closing.iter().any(|v| !v.state().closed) {
        engine.pump();
        // sync-ok: the window is ending; CEF closes its browsers asynchronously, bounded to 3 s
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(closing);
    engine.pump_for(50);
    drop(scheduler);
    engine.shutdown();
    ENGINE_SHUTDOWNS.fetch_add(1, Ordering::SeqCst);
}

enum EngineState {
    None,
    /// The runtime question is out (`WebEvent::Runtime`).
    Checking,
    Ready(Rc<Cef>),
    /// The runtime is missing or CEF would not start: every view fails with this.
    Failed(String),
    /// `close_all` shut it down: nothing opens again in this process.
    Ended,
}

/// What a caller's DevTools call is for, by the id CEF knows it under.
enum Call {
    Agent { slot: Slot, id: u64 },
    Script { id: u64 },
}

struct Live {
    view: View,
    /// The view navigated (its setup answered).
    armed: bool,
}

struct Slotted {
    generation: u64,
    app: Option<AppSpec>,
    url: String,
    bounds: PhysRect,
    visible: bool,
    live: Option<Live>,
    /// The profile this view runs in (the key of `WebHost::contexts`).
    profile: (Kind, String),
    paints: u64,
}

/// A download held for the person's yes.
struct HeldDownload {
    slot: Slot,
    generation: u64,
    view_id: u64,
    file: String,
}

type Inbox = Arc<Mutex<VecDeque<(Slot, u64, ViewEvent)>>>;

/// What comes in from the pop-ups (rules 249, 250): CEF made (or gave up on) a pop-up's browser, or a pop-up's
/// handler said something. Their handlers only queue, as a view's do.
enum PopupIn {
    Made { id: u64, view: Option<View> },
    Event { id: u64, event: ViewEvent },
}

type PopupInbox = Arc<Mutex<VecDeque<PopupIn>>>;

/// The pop-ups of every web panel: a request waits (its site) while CEF makes the browser, an open one is its id
/// (its view is the host's, in `popup_views`). Shared with the new-window rule, which counts and asks in CEF's
/// handler.
type Book = Arc<Mutex<PopupBook<String, u64>>>;

/// An open pop-up's browser and what the host follows of it.
struct PopupView {
    view: View,
    /// The address the strip last named the site of.
    url: String,
    /// The frame serial of its last picture.
    serial: u64,
}

/// The web panels' app links (rule 252): where they are told and, per panel, the last one told, so a link said
/// twice within a second is asked about once.
#[derive(Clone)]
struct AppLinks {
    sink: Sink,
    seen: Arc<Mutex<HashMap<Slot, AppLinkDedupe>>>,
    start: Instant,
}

impl AppLinks {
    /// The page at `from` in the panel of `slot` went to `uri`, which the wall stopped. True when it is a link to
    /// an app (`external_scheme` offers it): the window is told once (`WebEvent::AppLink`) and the log notes
    /// `why: app-link`; the caller says nothing more. False for anything else, which the caller refuses as before.
    fn offer(&self, slot: &Slot, uri: &str, from: &str) -> bool {
        let ExternalScheme::Offer(scheme) = external_scheme(uri) else {
            return false;
        };
        let now = u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX);
        let fresh = lock(&self.seen)
            .entry(slot.clone())
            .or_default()
            .fresh(uri, now);
        if fresh {
            let sink = ViewSink::new(&self.sink, slot);
            let note = WebNote::new(NoteKind::NavRefused, uri, from).detail("app-link");
            sink.send(|tab| WebEvent::Note { tab, note });
            let site = if from.is_empty() {
                String::new()
            } else {
                site_of(from)
            };
            sink.send(|tab| WebEvent::AppLink {
                tab,
                uri: uri.to_string(),
                scheme,
                site,
            });
        }
        true
    }
}

/// The web panel's navigation wall: the main frame goes to http, https, about:blank, or a `blob:` the page it
/// leaves made itself (rule 253); a frame in the page anywhere, as on Windows.
fn web_nav_rule() -> NavRule {
    Arc::new(|url: &str, main: bool, from: &str| {
        !main || navigation_allowed(url) || own_blob(url, from)
    })
}

/// WebView2's permission kinds (`permission_word`'s numbers) for CEF's permission and media bits.
pub fn permission_kinds(permissions: u32, media: u32) -> Vec<i32> {
    let mut v = Vec::new();
    let mut add = |on: bool, kind: i32| {
        if on && !v.contains(&kind) {
            v.push(kind);
        }
    };
    add(media & 1 != 0 || permissions & 4096 != 0, 1);
    add(
        media & 2 != 0 || permissions & 4 != 0 || permissions & 2 != 0,
        2,
    );
    add(permissions & 256 != 0, 3);
    add(permissions & 32768 != 0, 4);
    add(permissions & 268_435_456 != 0, 5);
    add(permissions & 16 != 0, 6);
    add(permissions & 16384 != 0, 7);
    add(permissions & 16_777_216 != 0, 8);
    add(permissions & 128 != 0, 10);
    add(permissions & 8192 != 0, 11);
    add(permissions & 8_388_608 != 0, 12);
    if v.is_empty() && (permissions != 0 || media != 0) {
        v.push(-1);
    }
    v
}

/// What a web panel's new-window rule holds. It runs in CEF's `on_before_popup`, on the UI thread, while the host
/// is not borrowed (inside its pump): it only reads and writes these shared parts.
struct PopupRule {
    slot: Slot,
    sink: Sink,
    wake: Wake,
    book: Book,
    inbox: PopupInbox,
    links: AppLinks,
    requests: Option<RequestRule>,
}

/// The web panel's (and its pop-ups') answer to a page that asks for a window (rule 249, `popup_plan`), as the
/// Windows host's `take_new_windows`: a link to an app is asked about, a pop-up is a second windowless browser CEF
/// makes with the page's `window.opener`, a plain new-tab link opens in the view that asked, anything else opens
/// nothing; every request is noted for the log by site.
fn popup_rule(r: Arc<PopupRule>) -> NewWindowRule {
    Arc::new(move |ask: &WindowAsk| r.decide(ask))
}

impl PopupRule {
    fn decide(self: &Arc<Self>, ask: &WindowAsk) -> NewWindow {
        let (uri, from) = (ask.url.as_str(), ask.from.as_str());
        let sink = ViewSink::new(&self.sink, &self.slot);
        if !navigation_allowed(uri) && self.links.offer(&self.slot, uri, from) {
            return NewWindow::Refuse;
        }
        let counts = lock(&self.book).counts(&self.slot);
        let mut plan = popup_plan(uri, Some(&ask.name), ask.features, ask.user, counts);
        if plan == PopupPlan::Refuse(PopupRefusal::Scheme) && own_blob(uri, from) {
            plan = if ask.user {
                PopupPlan::InPlace
            } else {
                PopupPlan::Refuse(PopupRefusal::Blocked)
            };
        }
        let word = match plan {
            PopupPlan::InPlace => "panel",
            PopupPlan::Popup => "popup",
            PopupPlan::Refuse(r) => r.word(),
        };
        let note = WebNote::new(NoteKind::Popup, uri, from)
            .detail(word)
            .user(ask.user);
        sink.send(|tab| WebEvent::Note { tab, note });
        match plan {
            PopupPlan::InPlace => NewWindow::SameView,
            PopupPlan::Refuse(why) => {
                // An address that may not open is said in the bar as before; a blocked pop-up from its note.
                if why == PopupRefusal::Scheme {
                    sink.send(|tab| WebEvent::Refused {
                        tab,
                        url: uri.to_string(),
                    });
                }
                NewWindow::Refuse
            }
            PopupPlan::Popup => {
                let blank = uri.trim().is_empty() || uri.trim().eq_ignore_ascii_case("about:blank");
                let site = site_of(if blank { from } else { uri });
                let id = lock(&self.book).ask(&self.slot, site);
                let (inbox, wake) = (Arc::clone(&self.inbox), Arc::clone(&self.wake));
                let events: EventSink = Arc::new(move |event| {
                    lock(&inbox).push_back(PopupIn::Event { id, event });
                    wake();
                });
                let (inbox, wake) = (Arc::clone(&self.inbox), Arc::clone(&self.sink));
                let made: Made = Box::new(move |view| {
                    lock(&inbox).push_back(PopupIn::Made { id, view });
                    wake(WebEvent::Pump);
                });
                NewWindow::Popup(PopupSpec {
                    walls: Walls {
                        nav: Some(web_nav_rule()),
                        requests: self.requests.clone(),
                        new_windows: NewWindows::Refuse,
                        // A pop-up's own requests stack on the same tab.
                        new_window_rule: Some(popup_rule(Arc::clone(self))),
                        // A pop-up's downloads are cancelled at once, as on Windows.
                        downloads: Downloads::Cancel,
                    },
                    events,
                    frame_rate: 60,
                    made,
                })
            }
        }
    }
}

/// A mini-app's or a plugin's page opens no window of its own: noted by site and said in the bar.
fn refusing_rule(slot: Slot, sink: Sink) -> NewWindowRule {
    Arc::new(move |ask: &WindowAsk| {
        let sink = ViewSink::new(&sink, &slot);
        let note = WebNote::new(NoteKind::Popup, &ask.url, &ask.from)
            .detail("refused")
            .user(ask.user);
        sink.send(|tab| WebEvent::Note { tab, note });
        sink.send(|tab| WebEvent::Refused {
            tab,
            url: ask.url.clone(),
        });
        NewWindow::Refuse
    })
}

/// The CEF host.
pub struct WebHost {
    sink: Sink,
    data_dir: PathBuf,
    runtime: RuntimeCheck,
    virtual_hosts: Vec<(String, PathBuf)>,
    without_webrtc_script: bool,
    limits: Option<Arc<flare_mods::limit::Limits>>,
    engine: EngineState,
    views: HashMap<Slot, Slotted>,
    /// Views closed and not yet gone (CEF closes a browser asynchronously).
    closing: Vec<View>,
    generations: HashMap<Slot, u64>,
    contexts: HashMap<(Kind, String), Context>,
    inbox: Inbox,
    calls: HashMap<u64, Call>,
    call_seq: u64,
    downloads: HashMap<u64, HeldDownload>,
    download_seq: u64,
    downloads_dir: PathBuf,
    filter: Option<KeyFilter>,
    scale: f64,
    /// Chromium's network log for the engine, or none (the tests).
    net_log: Option<PathBuf>,
    /// The web panels' pop-ups (rules 249, 250): the book the new-window rules share, the open ones' views (never
    /// in `views`, so no agent call, script or navigation reaches one), and what their handlers queued.
    book: Book,
    popup_views: HashMap<u64, PopupView>,
    popup_inbox: PopupInbox,
    links: AppLinks,
    /// The web panel's navigation under way, per panel: its number, address and the page it left (the failed
    /// load's note names them).
    started: HashMap<Slot, (u64, String, String)>,
    /// The frame serial: every new picture of a page or a pop-up takes the next one.
    serial: u64,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// CEF's event flags for the window's modifiers.
fn mod_flags(m: Mods) -> u32 {
    let mut f = 0;
    if m.shift {
        f |= flags::SHIFT;
    }
    if m.ctrl {
        f |= flags::CONTROL;
    }
    if m.alt {
        f |= flags::ALT;
    }
    if m.cmd {
        f |= flags::COMMAND;
    }
    f
}

fn button_of(b: PointerButton) -> (Button, u32) {
    match b {
        PointerButton::Left => (Button::Left, flags::LEFT_BUTTON),
        PointerButton::Middle => (Button::Middle, flags::MIDDLE_BUTTON),
        PointerButton::Right => (Button::Right, flags::RIGHT_BUTTON),
    }
}

/// The key the filter sees for a forwarded key. On a Mac Cmd is the app's key (rule 1006): it plays the part `ctrl`
/// plays on Windows, and the real Ctrl is the page's.
pub fn key_press_of(k: &KeyInput) -> KeyPress {
    KeyPress {
        vk: k.vk,
        ctrl: k.mods.cmd,
        shift: k.mods.shift,
        alt: k.mods.alt,
        down: k.down,
        repeat: k.repeat,
    }
}

/// The character AppKit gives a key that types nothing (`NSEvent.characters`): Backspace is DEL, Forward Delete,
/// the arrows, Home, End, the page keys and the F-keys are AppKit's function-key codes. 0 for a key with none.
pub fn ns_key_char(vk: u32) -> u16 {
    match vk {
        0x08 => 0x7F,
        0x09 => 0x09,
        0x0D => 0x0D,
        0x1B => 0x1B,
        0x2E => 0xF728,
        0x26 => 0xF700,
        0x28 => 0xF701,
        0x25 => 0xF702,
        0x27 => 0xF703,
        0x24 => 0xF729,
        0x23 => 0xF72B,
        0x21 => 0xF72C,
        0x22 => 0xF72D,
        0x70..=0x7B => 0xF704 + (vk - 0x70) as u16,
        _ => 0,
    }
}

/// `Runtime.evaluate`'s answer as `ExecuteScript`'s: the value's JSON (`null` for undefined), or the exception.
pub fn script_result(answer: Result<String, String>) -> Result<String, String> {
    let body = answer?;
    let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    if let Some(ex) = v.get("exceptionDetails") {
        let text = ex["exception"]["description"]
            .as_str()
            .or_else(|| ex["text"].as_str())
            .unwrap_or("the script threw");
        return Err(text.to_string());
    }
    let value = v["result"].get("value").cloned();
    Ok(value.unwrap_or(serde_json::Value::Null).to_string())
}

impl WebHost {
    pub fn new(cfg: HostConfig) -> Self {
        let cfg_sink = Arc::clone(&cfg.sink);
        let downloads_dir = std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join("Downloads"))
            .unwrap_or_else(std::env::temp_dir);
        WebHost {
            sink: cfg.sink,
            data_dir: cfg.data_dir,
            runtime: cfg.runtime,
            virtual_hosts: cfg.virtual_hosts,
            without_webrtc_script: cfg.without_webrtc_script,
            limits: cfg.limits,
            engine: EngineState::None,
            views: HashMap::new(),
            closing: Vec::new(),
            generations: HashMap::new(),
            contexts: HashMap::new(),
            inbox: Arc::default(),
            calls: HashMap::new(),
            call_seq: 0,
            downloads: HashMap::new(),
            download_seq: 0,
            downloads_dir,
            filter: None,
            scale: 2.0,
            net_log: None,
            book: Arc::default(),
            popup_views: HashMap::new(),
            popup_inbox: Arc::default(),
            links: AppLinks {
                sink: Arc::clone(&cfg_sink),
                seen: Arc::default(),
                start: Instant::now(),
            },
            started: HashMap::new(),
            serial: 0,
        }
    }

    /// Chromium's network log for the engine this host starts (the tests read every request the engine made).
    /// Before the first view; the log is complete once `close_all` shut the engine down.
    pub fn set_net_log(&mut self, path: PathBuf) {
        self.net_log = Some(path);
    }

    /// Where a download the person said yes to is saved (`~/Downloads`; the tests give a folder of their own).
    pub fn set_downloads_dir(&mut self, dir: PathBuf) {
        self.downloads_dir = dir;
    }

    /// A headless browser process's turn (the engine tests): AppKit's queued events and the run loop, which winit
    /// serves in the real app, then CEF's work and the inbox.
    pub fn turn_headless(&mut self) {
        if let EngineState::Ready(e) = &self.engine {
            e.pump();
        }
        self.pump();
    }

    /// The profile's preference, for the tests (the walls read back).
    pub fn profile_preference(&self, slot: &Slot, name: &str) -> String {
        self.views
            .get(slot)
            .and_then(|v| self.contexts.get(&v.profile))
            .map(|c| c.preference(name))
            .unwrap_or_default()
    }

    /// What the view's handlers kept off the screen and out of the network, for the tests.
    pub fn refused(&self, slot: &Slot) -> Option<flare_cef::macos::view::Refused> {
        self.views
            .get(slot)?
            .live
            .as_ref()
            .map(|l| l.view.state().refused.clone())
    }

    /// How many times the slot's popup layer (a `<select>` list) painted, for the tests.
    pub fn popup_paints(&self, slot: &Slot) -> Option<u64> {
        Some(
            self.views
                .get(slot)?
                .live
                .as_ref()?
                .view
                .state()
                .popup_paints,
        )
    }

    /// Runs a script in any slot's page, for the tests (the window runs scripts only in a tab's panel).
    pub fn inspect_exec(&self, slot: &Slot, js: &str) {
        if let Some(v) = self.live(slot) {
            v.exec(js);
        }
    }

    /// The browser's zoom level of the slot's page, for the tests.
    pub fn zoom_level(&self, slot: &Slot) -> Option<f64> {
        Some(self.views.get(slot)?.live.as_ref()?.view.zoom_level())
    }

    fn view_sink(&self, slot: &Slot) -> ViewSink {
        ViewSink::new(&self.sink, slot)
    }

    fn fail(&self, slot: &Slot, why: &str, missing_runtime: bool) {
        self.view_sink(slot).send(|tab| WebEvent::Failed {
            tab,
            why: why.to_string(),
            missing_runtime,
        });
    }

    fn next_generation(&mut self, slot: &Slot) -> u64 {
        let g = self.generations.entry(slot.clone()).or_insert(0);
        *g += 1;
        *g
    }

    /// Asks the runtime question once (answered by `runtime_checked`), or makes what is waiting.
    fn ensure_engine(&mut self) {
        match &self.engine {
            EngineState::None => {
                self.engine = EngineState::Checking;
                (self.sink)(WebEvent::Runtime((self.runtime)()));
            }
            EngineState::Ready(_) => self.make_waiting(),
            EngineState::Failed(why) => {
                let why = why.clone();
                let slots: Vec<Slot> = self.views.keys().cloned().collect();
                for s in slots {
                    self.views.remove(&s);
                    // Not WebView2's missing runtime: the panel shows the sentence, never the Edge download link.
                    self.fail(&s, &why, false);
                }
            }
            EngineState::Checking | EngineState::Ended => {}
        }
    }

    /// The profile of a view of `app`, made the first time (its walls in its preferences).
    fn context(&mut self, app: Option<&AppSpec>) -> Result<(Kind, String), String> {
        let kind = Kind::of(app);
        let (name, private) = walls::profile_of(app);
        // One profile per mini-app, one per mod (rule 1029).
        let key = (kind, walls::context_name(app));
        if self.contexts.contains_key(&key) {
            return Ok(key);
        }
        let proxy = match kind {
            Kind::Web => None,
            Kind::AppSealed | Kind::Plugin => Some(DEAD_PROXY.to_string()),
            Kind::AppNet | Kind::PluginNet => Some(format!(
                "http://127.0.0.1:{}",
                flare_surfaces::netproxy::port()?
            )),
        };
        let staging = walls::download_staging(&self.data_dir);
        // sync-ok: one folder made once, the first time a profile is made
        std::fs::create_dir_all(&staging)
            .map_err(|e| format!("no folder for held downloads ({}): {e}", staging.display()))?;
        let profile = Profile {
            cache: (!private).then(|| walls::profile_folder(&self.data_dir, kind, &name)),
            staging: Some(staging),
            bypass: proxy.as_ref().map(|_| NO_BYPASS.to_string()),
            proxy,
            webrtc_sealed: kind.proxied(),
            wall: self.request_rule(app),
            wake: Some(self.wake()),
        };
        let ctx = Context::create(profile)?;
        self.contexts.insert(key.clone(), ctx);
        Ok(key)
    }

    /// The request wall of a view of `app`, as CEF's handlers call it (on its IO thread).
    fn request_rule(&self, app: Option<&AppSpec>) -> Option<RequestRule> {
        if app.is_none() && self.virtual_hosts.is_empty() {
            return None;
        }
        let app = app.cloned();
        let hosts = self.virtual_hosts.clone();
        Some(Arc::new(move |url: &str| {
            match walls::decide(app.as_ref(), &hosts, url) {
                Decision::Pass => Answer::Pass,
                Decision::Refuse => Answer::Refuse(403),
                Decision::Serve {
                    folder,
                    entry,
                    path,
                    module,
                } => match flare_mods::serve::serve_file(
                    // The file server checks every path against the folder's real path (`/var` is `/private/var`).
                    // sync-ok: CEF's IO thread answers the request, never the UI thread
                    &std::fs::canonicalize(&folder).unwrap_or(folder),
                    &entry,
                    &path,
                ) {
                    Ok(s) => {
                        let network = app.as_ref().is_some_and(|a| a.allow_network);
                        let headers = if module {
                            walls::header_pairs(&flare_mods::serve::headers_for(s.mime, network))
                        } else {
                            vec![("Cache-Control".into(), "no-store".into())]
                        };
                        Answer::Serve {
                            status: 200,
                            mime: s.mime.to_string(),
                            headers,
                            body: Arc::new(s.bytes),
                        }
                    }
                    Err(r) => {
                        log::debug!("a page's request was refused: {}", r.reason);
                        Answer::Refuse(r.status)
                    }
                },
            }
        }))
    }

    /// A turn of this host soon, asked from any thread: what CEF's handlers queue or set (a view's events, its
    /// setup's answers, a profile made ready) is only taken in by [`WebBackend::pump`]. CEF asks for turns only for
    /// its own work, and in the real window that work mostly runs in AppKit's run loop without asking, so without
    /// this a view waited for a turn that never came: no browser, no navigation, a dark panel (rule 1152). Through
    /// the scheduler, so a burst (60 paints a second) is one pump at a time. Before the engine: nothing to wake.
    fn wake(&self) -> Wake {
        match self.scheduler() {
            Some(s) => {
                let schedule = s.schedule();
                Arc::new(move || schedule(0))
            }
            None => Arc::new(|| {}),
        }
    }

    /// While a view waits for CEF (its profile not yet initialized, its setup not yet answered), another turn in
    /// [`WAITING_TURN`]: the handlers that end the wait wake the host ([`WebHost::wake`]), and this is the backstop
    /// for a wait whose end comes with no handler call and no turn CEF asks for (rule 1152; seen with a second host
    /// in one CEF: the profile's initialization ran only in a turn of CEF's work that nothing asked for).
    fn retry_while_waiting(&self) {
        let Some(s) = self.scheduler() else { return };
        let waiting = self
            .views
            .values()
            .any(|v| v.live.as_ref().is_none_or(|l| !l.armed));
        if waiting {
            (s.schedule())(i64::try_from(WAITING_TURN.as_millis()).unwrap_or(20));
        }
    }

    /// The first host's start of the process's CEF ([`Cef`]): once per process.
    fn start_engine(&self, result: Result<String, String>) -> Result<Rc<Cef>, String> {
        let version = result?;
        let scheduler = Scheduler::start(Arc::clone(&self.sink));
        // What a run that ended early left in the staging folder is no one's download any more.
        let staging = walls::download_staging(&self.data_dir);
        // sync-ok: the engine starts once; the staging folder holds only unanswered downloads' parts
        let _ = std::fs::remove_dir_all(&staging);
        let cfg = EngineConfig {
            root_cache: self.data_dir.clone(),
            log_file: None,
            schedule: Some(scheduler.schedule()),
            adopt_app: true,
            net_log: self.net_log.clone(),
        };
        ENGINE_STARTS.fetch_add(1, Ordering::SeqCst);
        let engine = Engine::start(&cfg)?;
        log::info!("the web engine started: {version}");
        if let Some(note) = engine.profile_key_note() {
            log::warn!("{note}");
        }
        let cef = Rc::new(Cef {
            engine,
            scheduler,
            closing: RefCell::default(),
        });
        CEF.with(|c| *c.borrow_mut() = ProcessCef::Up(Rc::clone(&cef)));
        Ok(cef)
    }

    fn scheduler(&self) -> Option<&Scheduler> {
        match &self.engine {
            EngineState::Ready(cef) => Some(&cef.scheduler),
            _ => None,
        }
    }

    /// A GPU or utility helper failed (rule 1158). Chromium starts it again, so it is a note on every live view and
    /// a line in the log, as the Windows host's `Failure::Recovers`; no page ends.
    fn helper_failed(&self, gone: &flare_cef::macos::helpers::HelperExit) {
        log::info!(
            "a browser helper ended ({} {}, {} {}); Chromium starts it again",
            gone.kind,
            gone.service,
            gone.reason(),
            gone.code()
        );
        for (slot, v) in &self.views {
            let Some(l) = &v.live else { continue };
            let note = WebNote::new(NoteKind::Process, &l.view.url(), "")
                .detail(gone.kind)
                .failure(gone.reason(), gone.code());
            self.view_sink(slot)
                .send(|tab| WebEvent::Note { tab, note });
        }
    }

    /// Makes the browsers of every view waiting for one whose profile is ready.
    fn make_waiting(&mut self) {
        if !matches!(self.engine, EngineState::Ready(_)) {
            return;
        }
        let waiting: Vec<Slot> = self
            .views
            .iter()
            .filter(|(_, v)| v.live.is_none())
            .map(|(s, _)| s.clone())
            .collect();
        for slot in waiting {
            self.make_browser(&slot);
        }
        self.retry_while_waiting();
    }

    fn make_browser(&mut self, slot: &Slot) {
        let wake = self.wake();
        let Some(v) = self.views.get(slot) else {
            return;
        };
        let app = v.app.clone();
        let key = match self.context(app.as_ref()) {
            Ok(k) => k,
            Err(why) => {
                self.views.remove(slot);
                self.fail(slot, &why, false);
                return;
            }
        };
        let ctx = match self.contexts.get(&key).map(Context::ready) {
            Some(Some(Ok(()))) => self.contexts.get(&key).map(|c| c.ctx.clone()),
            Some(Some(Err(why))) => {
                self.views.remove(slot);
                self.fail(slot, &why, false);
                return;
            }
            // CEF is still initializing the profile: the next pump tries again.
            _ => return,
        };
        let requests = self.request_rule(app.as_ref());
        let new_window_rule = if app.is_none() {
            popup_rule(Arc::new(PopupRule {
                slot: slot.clone(),
                sink: Arc::clone(&self.sink),
                wake: self.wake(),
                book: Arc::clone(&self.book),
                inbox: Arc::clone(&self.popup_inbox),
                links: self.links.clone(),
                requests: requests.clone(),
            }))
        } else {
            refusing_rule(slot.clone(), Arc::clone(&self.sink))
        };
        let Some(v) = self.views.get_mut(slot) else {
            return;
        };
        v.profile = key;
        let (css_w, css_h) = status::css_size(v.bounds.w, v.bounds.h, self.scale);
        let seal = app
            .as_ref()
            .filter(|a| a.module.is_some())
            .map(|_| plugin_seal(slot.kind));
        let nav_app = app.clone();
        let nav: NavRule = if app.is_none() {
            web_nav_rule()
        } else {
            Arc::new(move |url: &str, main: bool, _from: &str| {
                walls::nav_allowed(nav_app.as_ref(), url, main)
            })
        };
        let walls = Walls {
            nav: Some(nav),
            requests,
            new_window_rule: Some(new_window_rule),
            // A page cannot open a window: the web panel's goes to the same view, a mini-app's or a mod's nowhere.
            new_windows: if app.is_none() {
                NewWindows::SameView
            } else {
                NewWindows::Refuse
            },
            downloads: match seal.map(|s| s.downloads) {
                Some(DownloadWall::Cancel) => Downloads::Cancel,
                _ => Downloads::Ask,
            },
        };
        let inbox = Arc::clone(&self.inbox);
        let (s2, generation) = (slot.clone(), v.generation);
        let events: EventSink = Arc::new(move |e| {
            lock(&inbox).push_back((s2.clone(), generation, e));
            wake();
        });
        let module = app.as_ref().is_some_and(|a| a.module.is_some());
        let opts = Options {
            url: pdf_view(&v.url),
            size: ViewSize {
                width: css_w,
                height: css_h,
                scale: self.scale as f32,
            },
            walls,
            events: Some(events),
            context: ctx,
            init_script: app
                .as_ref()
                .and_then(|a| walls::init_script(a, self.without_webrtc_script)),
            binding: module.then(|| BINDING.to_string()),
            frame_rate: 60,
        };
        let visible = v.visible;
        match View::create(opts) {
            Ok(view) => {
                if !visible {
                    view.set_hidden(true);
                }
                if let Some(v) = self.views.get_mut(slot) {
                    v.live = Some(Live { view, armed: false });
                }
            }
            Err(why) => {
                self.views.remove(slot);
                self.fail(slot, &why, false);
            }
        }
    }

    fn drop_view(&mut self, slot: &Slot) {
        // Its pop-ups go with it (and the requests CEF has not made a browser for yet).
        self.close_popups(Some(slot));
        self.started.remove(slot);
        lock(&self.links.seen).remove(slot);
        if let Some(v) = self.views.remove(slot) {
            let held: Vec<u64> = self
                .downloads
                .iter()
                .filter(|(_, d)| d.slot == *slot)
                .map(|(id, _)| *id)
                .collect();
            for id in held {
                self.downloads.remove(&id);
            }
            self.calls
                .retain(|_, c| !matches!(c, Call::Agent { slot: s, .. } if s == slot));
            if let Some(l) = v.live {
                l.view.close();
                self.closing.push(l.view);
            }
            // A mini-app's in-memory profile goes with its last view (its browser keeps it until it closed): the
            // next app on that host starts empty, as a private profile should, and closed apps add up to nothing.
            let key = v.profile;
            if matches!(key.0, Kind::AppSealed | Kind::AppNet)
                && !self.views.values().any(|o| o.profile == key)
            {
                self.contexts.remove(&key);
            }
        }
    }

    fn put_view(
        &mut self,
        slot: &Slot,
        url: &str,
        app: Option<AppSpec>,
        bounds: PhysRect,
        visible: bool,
    ) {
        if matches!(self.engine, EngineState::Ended) {
            self.fail(slot, "the window is closing", false);
            return;
        }
        if self.views.get(slot).is_some_and(|v| {
            Kind::of(v.app.as_ref()) != Kind::of(app.as_ref())
                || v.app.as_ref().map(|a| &a.host) != app.as_ref().map(|a| &a.host)
        }) {
            self.drop_view(slot);
        }
        if self.views.contains_key(slot) {
            self.set_view_bounds(slot, bounds);
            self.set_view_visible(slot, visible);
            self.go_to(slot, url);
            return;
        }
        let generation = self.next_generation(slot);
        self.views.insert(
            slot.clone(),
            Slotted {
                generation,
                profile: (Kind::of(app.as_ref()), String::new()),
                app,
                url: url.to_string(),
                bounds,
                visible,
                live: None,
                paints: 0,
            },
        );
        self.ensure_engine();
        self.flush();
    }

    fn go_to(&mut self, slot: &Slot, url: &str) {
        let Some(v) = self.views.get_mut(slot) else {
            return;
        };
        v.url = url.to_string();
        if let Some(l) = &v.live
            && l.armed
        {
            l.view.navigate(&pdf_view(url));
        }
    }

    fn live(&self, slot: &Slot) -> Option<&View> {
        self.views
            .get(slot)?
            .live
            .as_ref()
            .filter(|l| l.armed)
            .map(|l| &l.view)
    }

    /// Takes in the setup answers of views not yet armed: Ready, or the view's end.
    fn arm(&mut self) {
        let mut ready = Vec::new();
        let mut failed = Vec::new();
        for (slot, v) in &mut self.views {
            if let Some(l) = &mut v.live
                && !l.armed
            {
                match l.view.poll() {
                    Ok(true) => {
                        l.armed = true;
                        ready.push(slot.clone());
                    }
                    Ok(false) => {}
                    Err(why) => failed.push((slot.clone(), why)),
                }
            }
        }
        for s in ready {
            self.view_sink(&s).send(|tab| WebEvent::Ready { tab });
        }
        for (s, why) in failed {
            self.drop_view(&s);
            self.fail(&s, &why, false);
        }
    }

    /// Hands the inbox to the window.
    fn flush(&mut self) {
        loop {
            let next = lock(&self.inbox).pop_front();
            let Some((slot, generation, e)) = next else {
                break;
            };
            if matches!(e, ViewEvent::Closed) {
                self.closing.retain(|v| !v.state().closed);
            }
            let current = self.views.get(&slot).map(|v| v.generation) == Some(generation);
            if current {
                self.deliver(&slot, e);
            }
        }
    }

    fn deliver(&mut self, slot: &Slot, e: ViewEvent) {
        let sink = self.view_sink(slot);
        let app = self.views.get(slot).and_then(|v| v.app.clone());
        // The page events of the diagnostics log (`web-nav`, `web-load-failed`, `web-cert`) are the plain web
        // panel's only (rule 248).
        let page_log = app.is_none();
        let note = |note: WebNote| sink.send(|tab| WebEvent::Note { tab, note });
        match e {
            ViewEvent::Armed(_)
            | ViewEvent::SetupAnswered
            | ViewEvent::Closed
            | ViewEvent::DownloadEnded { .. } => {}
            ViewEvent::Painted => {
                self.serial += 1;
                let serial = self.serial;
                if let Some(v) = self.views.get_mut(slot) {
                    v.paints = serial;
                }
            }
            ViewEvent::Loading { url, nav, from } => {
                if page_log {
                    note(WebNote::new(NoteKind::Nav, &url, &from));
                    self.started.insert(slot.clone(), (nav, url.clone(), from));
                }
                sink.send(|tab| WebEvent::Loading { tab, url, nav })
            }
            ViewEvent::Loaded {
                url,
                ok,
                status,
                nav,
            } => {
                if page_log && load_failure_logged(ok, status) {
                    let (to, from) = match self.started.get(slot) {
                        Some((n, to, from)) if *n == nav => (to.clone(), from.clone()),
                        _ => (url.clone(), String::new()),
                    };
                    note(
                        WebNote::new(NoteKind::LoadFailed, &to, &from)
                            .detail(web_error_name(status)),
                    );
                }
                // A load that succeeded reads the title again (one equal to the last page's raises no change), as
                // the Windows host does.
                if ok && let Some(view) = self.live(slot) {
                    let title = view.state().title.clone();
                    sink.send(|tab| WebEvent::Title { tab, title });
                }
                sink.send(|tab| WebEvent::Loaded {
                    tab,
                    url,
                    ok,
                    status: status.to_string(),
                    nav,
                });
            }
            ViewEvent::Url { url } => sink.send(|tab| WebEvent::Url { tab, url }),
            ViewEvent::SameDocument { url } => sink.send(|tab| WebEvent::SameDocument { tab, url }),
            ViewEvent::Title { title } => sink.send(|tab| WebEvent::Title { tab, title }),
            ViewEvent::History {
                can_back,
                can_forward,
            } => sink.send(|tab| WebEvent::History {
                tab,
                can_back,
                can_forward,
            }),
            ViewEvent::Refused { url, from, main } => {
                if !main {
                    // A frame of a mini-app's or a mod's page going elsewhere (the web panel's frames go anywhere).
                    if app.is_some() {
                        note(WebNote::new(NoteKind::NavRefused, &url, &from).detail("frame"));
                    }
                    return;
                }
                // A link to an app is stopped here and the window asks the person (rule 252).
                if page_log && self.links.offer(slot, &url, &from) {
                    return;
                }
                let why = match &app {
                    None => "scheme",
                    Some(a) if a.module.is_some() => "not-its-host",
                    Some(_) => "not-its-page",
                };
                note(WebNote::new(NoteKind::NavRefused, &url, &from).detail(why));
                sink.send(|tab| WebEvent::Refused { tab, url })
            }
            ViewEvent::Permission {
                origin,
                permissions,
                media,
            } => {
                let page = self.live(slot).map(View::url).unwrap_or_default();
                for kind in permission_kinds(permissions, media) {
                    note(
                        WebNote::new(NoteKind::Permission, &origin, &page)
                            .detail(permission_word(kind)),
                    );
                }
            }
            ViewEvent::Cert { url, status } => {
                if page_log {
                    let page = self.live(slot).map(View::url).unwrap_or_default();
                    note(WebNote::new(NoteKind::Cert, &url, &page).detail(web_error_name(status)));
                }
            }
            ViewEvent::Download { id, file, url } => {
                self.download_seq += 1;
                let hid = self.download_seq;
                let generation = self.views.get(slot).map_or(0, |v| v.generation);
                self.downloads.insert(
                    hid,
                    HeldDownload {
                        slot: slot.clone(),
                        generation,
                        view_id: id,
                        file: file.clone(),
                    },
                );
                sink.send(|tab| WebEvent::Download {
                    tab,
                    id: hid,
                    file: if file.is_empty() {
                        "a file".to_string()
                    } else {
                        file
                    },
                    host: host_label(&url),
                });
            }
            ViewEvent::Focus { has } => sink.send(|tab| WebEvent::Focus { tab, has }),
            ViewEvent::FullScreen { on } => sink.send(|tab| WebEvent::FullScreen { tab, on }),
            ViewEvent::Gone { why, reason, code } => {
                let site = self.live(slot).map(View::url).unwrap_or_default();
                note(
                    WebNote::new(NoteKind::Process, &site, "")
                        .detail("render")
                        .failure(reason, code),
                );
                sink.send(|tab| WebEvent::Gone { tab, why })
            }
            ViewEvent::Dialog { kind } => log::debug!("a page's {kind} was answered, not shown"),
            ViewEvent::Cdp { id, result } => match self.calls.remove(&id) {
                Some(Call::Agent { slot: s, id }) => {
                    (self.sink)(WebEvent::Cdp {
                        tab: s.owner.clone(),
                        id,
                        result,
                    });
                }
                Some(Call::Script { id }) => (self.sink)(WebEvent::Script {
                    id,
                    result: script_result(result),
                }),
                None => {}
            },
            ViewEvent::Binding { payload, url } => self.mod_message(slot, payload, &url),
        }
    }

    /// A mod's page posted over its bridge: from its own address only, within its budget, as on Windows.
    fn mod_message(&self, slot: &Slot, text: String, source: &str) {
        let Some(app) = self.views.get(slot).and_then(|v| v.app.as_ref()) else {
            return;
        };
        let Some(m) = &app.module else { return };
        let limiter = self.limits.as_ref().and_then(|l| l.of(&m.id));
        if limiter
            .as_ref()
            .is_some_and(|l| !l.has_room(Instant::now()))
        {
            return;
        }
        let cap = if limiter.is_some() {
            flare_mods::limit::MAX_MESSAGE
        } else {
            MAX_PAGE_MESSAGE
        };
        if modview::nav_allowed(source, &app.host) && !text.is_empty() && text.len() <= cap {
            let host = app.host.clone();
            self.view_sink(slot).send(|tab| WebEvent::ModMessage {
                tab,
                host,
                json: text,
            });
        }
    }

    // ---- the web panel's pop-ups (rules 249, 250) ----

    /// Lets go of the pop-ups of one panel, or of all: a request CEF has not made a browser for yet makes nothing
    /// (its view is closed when it comes), and every open pop-up is closed.
    fn close_popups(&mut self, slot: Option<&Slot>) {
        let (_, ids) = lock(&self.book).drain(slot);
        for id in ids {
            self.shut_popup(id);
        }
    }

    fn shut_popup(&mut self, id: u64) {
        if let Some(p) = self.popup_views.remove(&id)
            && !p.view.state().closed
        {
            p.view.close();
            self.closing.push(p.view);
        }
    }

    /// The pop-up on top of the panel of `slot`, if one is open.
    fn top_popup(&self, slot: &Slot) -> Option<&PopupView> {
        let id = lock(&self.book).top_id(slot)?;
        self.popup_views.get(&id)
    }

    /// What shows in the panel of `slot`: the pop-up on top, else the page.
    fn front(&self, slot: &Slot) -> Option<&View> {
        match self.top_popup(slot) {
            Some(p) => Some(&p.view),
            None => self.live(slot),
        }
    }

    /// The panel's pop-up on top (or, with none, the page) shows as the panel is; everything else waits hidden.
    fn show_front(&mut self, slot: &Slot) {
        let visible = self.views.get(slot).is_some_and(|v| v.visible);
        let top = lock(&self.book).top_id(slot);
        let ids: Vec<u64> = lock(&self.book)
            .open_of(slot)
            .iter()
            .map(|o| o.view)
            .collect();
        for id in ids {
            if let Some(p) = self.popup_views.get(&id) {
                p.view.set_hidden(!(visible && Some(id) == top));
            }
        }
        if let Some(page) = self.live(slot) {
            page.set_hidden(!(visible && top.is_none()));
        }
        // The window draws the new front at once.
        self.serial += 1;
        let serial = self.serial;
        if let Some(v) = self.views.get_mut(slot) {
            v.paints = serial;
        }
    }

    /// Tells the window the sites of the panel's pop-ups (its strip).
    fn tell_popups(&self, slot: &Slot) {
        if !slot.is_panel() {
            return;
        }
        let sites = lock(&self.book).sites(slot);
        self.view_sink(slot)
            .send(|tab| WebEvent::Popups { tab, sites });
    }

    /// Takes in what the pop-ups queued: a browser CEF made, and their handlers' events.
    fn take_popups(&mut self) {
        loop {
            let next = lock(&self.popup_inbox).pop_front();
            match next {
                None => break,
                Some(PopupIn::Made { id, view }) => self.adopt_popup(id, view),
                Some(PopupIn::Event { id, event }) => self.popup_event(id, event),
            }
        }
        // The strip names the site each pop-up is on now (not a blank page the opener is still writing into).
        let moved: Vec<(u64, String)> = self
            .popup_views
            .iter_mut()
            .filter_map(|(id, p)| {
                let url = p.view.state().url.clone();
                (url != p.url).then(|| {
                    p.url = url.clone();
                    (*id, url)
                })
            })
            .collect();
        for (id, url) in moved {
            if url.is_empty() || url.eq_ignore_ascii_case("about:blank") {
                continue;
            }
            let slot = {
                let mut b = lock(&self.book);
                b.set_site(id, site_of(&url))
                    .then(|| b.slot_of(id).cloned())
                    .flatten()
            };
            if let Some(slot) = slot {
                self.tell_popups(&slot);
            }
        }
    }

    /// A pop-up's browser is there: it shows over the page in the panel, sized as the page, and the page (or the
    /// pop-up below) waits hidden behind it.
    fn adopt_popup(&mut self, id: u64, view: Option<View>) {
        let taken = lock(&self.book).take(id);
        let Some(view) = view else {
            return;
        };
        let Some((slot, site)) = taken.filter(|(s, _)| self.live(s).is_some()) else {
            // Let go meanwhile (the panel closed, or its page went): the pop-up is not wanted.
            view.close();
            self.closing.push(view);
            return;
        };
        let bounds = self.views.get(&slot).map(|v| v.bounds).unwrap_or_default();
        let (w, h) = status::css_size(bounds.w, bounds.h, self.scale);
        let _ = view.resize_scaled(w, h, self.scale as f32);
        let focused = self.front(&slot).is_some_and(|f| f.state().focused);
        lock(&self.book).push(&slot, id, site, id);
        if focused {
            view.set_focus(true);
        }
        self.popup_views.insert(
            id,
            PopupView {
                view,
                url: String::new(),
                serial: 0,
            },
        );
        self.show_front(&slot);
        self.tell_popups(&slot);
    }

    /// A pop-up closes (its page's `window.close()`, its process, the strip's Close): the next one down, or the
    /// page, shows again.
    fn remove_popup(&mut self, id: u64) {
        let closed = lock(&self.book).close(id);
        let Some((slot, _, top)) = closed else {
            return;
        };
        self.shut_popup(id);
        if top {
            self.show_front(&slot);
        }
        self.tell_popups(&slot);
    }

    /// What a pop-up's handlers said: never told as the panel's page events (rule 250), only what the log, the app
    /// links and the strip need.
    fn popup_event(&mut self, id: u64, e: ViewEvent) {
        if matches!(e, ViewEvent::Closed) {
            self.closing.retain(|v| !v.state().closed);
        }
        let slot = lock(&self.book).slot_of(id).cloned();
        let Some(slot) = slot else {
            return;
        };
        let sink = self.view_sink(&slot);
        let note = |note: WebNote| sink.send(|tab| WebEvent::Note { tab, note });
        match e {
            ViewEvent::Closed => self.remove_popup(id),
            ViewEvent::Gone { reason, code, .. } => {
                let site = self
                    .popup_views
                    .get(&id)
                    .map(|p| p.view.url())
                    .unwrap_or_default();
                note(
                    WebNote::new(NoteKind::Process, &site, "")
                        .detail("render")
                        .failure(reason, code),
                );
                self.remove_popup(id);
            }
            ViewEvent::Refused {
                url,
                from,
                main: true,
            } => {
                if !self.links.offer(&slot, &url, &from) {
                    note(WebNote::new(NoteKind::NavRefused, &url, &from).detail("scheme"));
                }
            }
            ViewEvent::Permission {
                origin,
                permissions,
                media,
            } => {
                let page = self
                    .popup_views
                    .get(&id)
                    .map(|p| p.view.url())
                    .unwrap_or_default();
                for kind in permission_kinds(permissions, media) {
                    note(
                        WebNote::new(NoteKind::Permission, &origin, &page)
                            .detail(permission_word(kind)),
                    );
                }
            }
            ViewEvent::Painted => {
                self.serial += 1;
                let serial = self.serial;
                if let Some(p) = self.popup_views.get_mut(&id) {
                    p.serial = serial;
                }
            }
            _ => {}
        }
    }

    fn next_call(&mut self, call: Call) -> u64 {
        self.call_seq += 1;
        // CEF's ids are an `int`; the view's own setup counts down from near its top.
        if self.call_seq > (i32::MAX as u64) - 2000 {
            self.call_seq = 1;
        }
        self.calls.insert(self.call_seq, call);
        self.call_seq
    }
}

impl WebBackend for WebHost {
    fn open(&mut self, tab: &str, url: &str, bounds: PhysRect, visible: bool) {
        self.put_view(&Slot::panel(tab), url, None, bounds, visible);
    }

    fn open_view(&mut self, slot: &Slot, app: AppSpec, bounds: PhysRect, visible: bool) {
        if !slot.is_panel() && app.module.is_none() {
            self.drop_view(slot);
            self.fail(slot, "only a plugin's page opens here", false);
            return;
        }
        let url = app.url.clone();
        self.put_view(slot, &url, Some(app), bounds, visible);
    }

    fn navigate(&mut self, tab: &str, url: &str) {
        self.go_to(&Slot::panel(tab), url);
    }

    fn go(&mut self, tab: &str, nav: Nav) {
        if let Some(v) = self.live(&Slot::panel(tab)) {
            match nav {
                Nav::Back => v.back(),
                Nav::Forward => v.forward(),
                Nav::Reload => v.reload(),
                Nav::Stop => v.stop(),
            }
        }
    }

    fn set_view_bounds(&mut self, slot: &Slot, bounds: PhysRect) {
        let scale = self.scale;
        let (w, h) = status::css_size(bounds.w, bounds.h, scale);
        if let Some(v) = self.views.get_mut(slot) {
            v.bounds = bounds;
            if let Some(l) = &v.live {
                let _ = l.view.resize_scaled(w, h, scale as f32);
            }
        }
        // A pop-up takes the page's box (under Flare's strip, which the window has already taken off it).
        let ids: Vec<u64> = lock(&self.book)
            .open_of(slot)
            .iter()
            .map(|o| o.view)
            .collect();
        for id in ids {
            if let Some(p) = self.popup_views.get(&id) {
                let _ = p.view.resize_scaled(w, h, scale as f32);
            }
        }
    }

    fn set_view_visible(&mut self, slot: &Slot, visible: bool) {
        if let Some(v) = self.views.get_mut(slot) {
            v.visible = visible;
            if let Some(l) = &v.live
                && lock(&self.book).top_id(slot).is_none()
            {
                l.view.set_hidden(!visible);
            }
        }
        if lock(&self.book).top_id(slot).is_some() {
            self.show_front(slot);
        }
    }

    fn capture_still(&mut self, tab: &str, token: u64) {
        let png = self
            .front(&Slot::panel(tab))
            .and_then(|v| v.frame())
            .and_then(|p| p.png());
        (self.sink)(WebEvent::Still {
            tab: tab.to_string(),
            token,
            png,
        });
    }

    /// Only a tab's panel takes the keyboard: a bar or a background page never does.
    fn focus(&mut self, tab: &str) {
        if let Some(v) = self.front(&Slot::panel(tab)) {
            let had = v.state().focused;
            v.set_focus(true);
            if !had {
                (self.sink)(WebEvent::Focus {
                    tab: tab.to_string(),
                    has: true,
                });
            }
        }
    }

    fn blur(&mut self, tab: &str) {
        if let Some(v) = self.front(&Slot::panel(tab)) {
            let had = v.state().focused;
            v.set_focus(false);
            if had {
                (self.sink)(WebEvent::Focus {
                    tab: tab.to_string(),
                    has: false,
                });
            }
        }
    }

    fn close_view(&mut self, slot: &Slot) {
        self.drop_view(slot);
    }

    fn close_popup(&mut self, tab: &str) {
        let top = lock(&self.book).top_id(&Slot::panel(tab));
        if let Some(id) = top {
            self.remove_popup(id);
        }
    }

    fn close_all(&mut self) {
        self.close_popups(None);
        let slots: Vec<Slot> = self.views.keys().cloned().collect();
        for s in slots {
            self.drop_view(&s);
        }
        self.downloads.clear();
        self.calls.clear();
        if let EngineState::Ready(cef) = std::mem::replace(&mut self.engine, EngineState::Ended) {
            cef.closing.borrow_mut().append(&mut self.closing);
            self.contexts.clear();
            drop(cef);
            end_engine();
        }
        lock(&self.inbox).clear();
    }

    fn answer_download(&mut self, id: u64, save: bool) {
        let Some(d) = self.downloads.remove(&id) else {
            return;
        };
        let Some(v) = self
            .views
            .get(&d.slot)
            .filter(|v| v.generation == d.generation)
            .and_then(|v| v.live.as_ref())
        else {
            return;
        };
        if save {
            let path = status::download_target(&self.downloads_dir, &d.file, |p| p.exists());
            v.view.answer_download(d.view_id, Some(&path));
        } else {
            v.view.answer_download(d.view_id, None);
        }
    }

    fn pump(&mut self) {
        if let EngineState::Ready(e) = &self.engine {
            e.work();
            e.closing.borrow_mut().retain(|v| !v.state().closed);
            for gone in e.helper_exits() {
                self.helper_failed(&gone);
            }
        }
        self.make_waiting();
        self.arm();
        self.flush();
        self.take_popups();
    }

    fn runtime_checked(&mut self, result: Result<String, String>) {
        if !matches!(self.engine, EngineState::Checking) {
            return;
        }
        // The engine an earlier host started: adopted, never started again (rule 1156).
        let process = CEF.with(|c| match &*c.borrow() {
            ProcessCef::None => None,
            ProcessCef::Up(cef) => Some(Ok(Rc::clone(cef))),
            ProcessCef::Ended => Some(Err("the web engine has shut down".to_string())),
        });
        let started = match process {
            Some(adopted) => adopted.inspect(|cef| cef.scheduler.point_to(&self.sink)),
            None => self.start_engine(result),
        };
        match started {
            Ok(cef) => {
                self.engine = EngineState::Ready(cef);
                self.make_waiting();
            }
            Err(why) => {
                self.engine = EngineState::Failed(why);
                self.ensure_engine();
            }
        }
        self.flush();
    }

    fn controllers(&self) -> usize {
        self.views.len()
    }

    fn set_key_filter(&mut self, filter: KeyFilter) {
        self.filter = Some(filter);
    }

    fn cdp(&mut self, tab: &str, id: u64, method: &str, params: &str) {
        let slot = Slot::panel(tab);
        if self.live(&slot).is_none() {
            (self.sink)(WebEvent::Cdp {
                tab: tab.to_string(),
                id,
                result: Err("no web view".to_string()),
            });
            return;
        }
        let params: serde_json::Value =
            serde_json::from_str(params).unwrap_or_else(|_| serde_json::json!({}));
        // The agent's editing chords carry the command a Mac's text system would have made of them (rule 1027).
        let params = flare_cef::status::mac_devtools_params(method, &params);
        let cef_id = self.next_call(Call::Agent {
            slot: slot.clone(),
            id,
        });
        let sent = self
            .live(&slot)
            .map(|v| v.cdp_send(cef_id, method, &params));
        if let Some(Err(e)) = sent {
            self.calls.remove(&cef_id);
            (self.sink)(WebEvent::Cdp {
                tab: tab.to_string(),
                id,
                result: Err(e),
            });
        }
    }

    fn post_to_view(&mut self, slot: &Slot, json: &str) {
        let is_mod = self
            .views
            .get(slot)
            .is_some_and(|v| v.app.as_ref().is_some_and(|a| a.module.is_some()));
        if is_mod && let Some(v) = self.live(slot) {
            v.exec(&walls::deliver_script(json));
        }
    }

    fn set_zoom(&mut self, tab: &str, factor: f64) {
        if let Some(v) = self.live(&Slot::panel(tab)) {
            v.set_zoom(factor);
        }
    }

    fn run_script(&mut self, tab: &str, id: u64, js: &str) {
        let slot = Slot::panel(tab);
        if self.live(&slot).is_none() {
            (self.sink)(WebEvent::Script {
                id,
                result: Err("no web view".to_string()),
            });
            return;
        }
        let cef_id = self.next_call(Call::Script { id });
        let params = serde_json::json!({"expression": js, "returnByValue": true});
        let sent = self
            .live(&slot)
            .map(|v| v.cdp_send(cef_id, "Runtime.evaluate", &params));
        if let Some(Err(e)) = sent {
            self.calls.remove(&cef_id);
            (self.sink)(WebEvent::Script { id, result: Err(e) });
        }
    }

    fn probe_view(&self, slot: &Slot) -> Option<ControllerProbe> {
        let v = self.views.get(slot)?;
        let view = &v.live.as_ref()?.view;
        let (profile, private) = walls::profile_of(v.app.as_ref());
        let s = view.state();
        Some(ControllerProbe {
            bounds: v.bounds,
            visible: v.visible,
            // No window of its own: the page is a picture the window draws.
            child_windows: Vec::new(),
            url: s.url.clone(),
            title: s.title.clone(),
            browser_pid: std::process::id(),
            profile,
            private,
            // CEF's Alloy browser has no password manager or form autofill: nothing is offered or filled in.
            autofill: false,
            popups: {
                let top = lock(&self.book).top_id(slot);
                let ids: Vec<u64> = lock(&self.book)
                    .open_of(slot)
                    .iter()
                    .map(|o| o.view)
                    .collect();
                ids.into_iter()
                    .filter_map(|id| {
                        let p = self.popup_views.get(&id)?;
                        Some(PopupProbe {
                            bounds: v.bounds,
                            visible: v.visible && Some(id) == top,
                            url: p.view.state().url.clone(),
                            autofill: false,
                        })
                    })
                    .collect()
            },
        })
    }

    fn frame(&self, slot: &Slot) -> Option<ViewFrame> {
        let v = self.views.get(slot)?;
        if let Some(top) = self.top_popup(slot) {
            let p = top.view.frame()?;
            return Some(ViewFrame {
                width: p.width,
                height: p.height,
                bgra: p.bgra,
                serial: top.serial.max(v.paints),
            });
        }
        let p = v.live.as_ref()?.view.frame()?;
        Some(ViewFrame {
            width: p.width,
            height: p.height,
            bgra: p.bgra,
            serial: v.paints,
        })
    }

    fn pointer(&mut self, slot: &Slot, p: Pointer) {
        let Some(v) = self.front(slot) else { return };
        let (x, y) = status::css_point(f64::from(p.x), f64::from(p.y), self.scale);
        let mods = mod_flags(p.mods);
        match p.act {
            PointerAct::Move => v.mouse_move(x, y, mods, false),
            PointerAct::Leave => v.mouse_move(x, y, mods, true),
            PointerAct::Down(b, clicks) => {
                let (b, f) = button_of(b);
                v.mouse_button(x, y, b, false, i32::from(clicks), mods | f);
            }
            PointerAct::Up(b, clicks) => {
                let (b, _) = button_of(b);
                v.mouse_button(x, y, b, true, i32::from(clicks), mods);
            }
            PointerAct::Wheel { dx, dy } => {
                let k = self.scale.max(1.0);
                // CEF's positive delta scrolls toward the top, a wheel's positive dy toward the bottom.
                let (cx, cy) = (
                    (-f64::from(dx) / k).round() as i32,
                    (-f64::from(dy) / k).round() as i32,
                );
                v.wheel(x, y, cx, cy, mods);
            }
        }
    }

    fn key(&mut self, slot: &Slot, k: KeyInput) {
        // The app's chords and the second Escape first, as the accelerator hook takes them on Windows.
        let press = key_press_of(&k);
        if k.down && slot.is_panel() && self.filter.as_ref().is_some_and(|f| f(&press)) {
            (self.sink)(WebEvent::Key {
                tab: slot.owner.clone(),
                key: press,
            });
            return;
        }
        let Some(v) = self.front(slot) else { return };
        let mut modifiers = mod_flags(k.mods);
        if k.repeat {
            modifiers |= flags::IS_REPEAT;
        }
        let code = i32::try_from(k.vk).unwrap_or(0);
        let native = i32::from(k.mac_code);
        let ns_char = k
            .text
            .encode_utf16()
            .next()
            .unwrap_or_else(|| ns_key_char(k.vk));
        v.key(Key {
            kind: if k.down { KeyKind::Down } else { KeyKind::Up },
            windows_code: code,
            native_code: native,
            // CEF on a Mac makes an `NSEvent` of the key and runs it through the text input: it needs the key's
            // own characters, or a Backspace's key up deletes a second time (measured on CEF 156).
            character: ns_char,
            unmodified: ns_char,
            modifiers,
        });
        // What the key types goes in as characters (not with Cmd: that is a shortcut, not text).
        if k.down && !k.mods.cmd {
            for unit in k.text.encode_utf16() {
                v.key(Key {
                    kind: KeyKind::Char,
                    windows_code: code,
                    native_code: native,
                    character: unit,
                    unmodified: unit,
                    modifiers,
                });
            }
        }
    }

    fn ime(&mut self, slot: &Slot, i: ImeInput) {
        let Some(v) = self.front(slot) else { return };
        match i {
            ImeInput::Preedit { text, cursor } => v.ime_compose(&text, cursor),
            ImeInput::Commit(text) => v.ime_commit(&text),
            ImeInput::Cancel => v.ime_cancel(),
        }
    }

    fn set_scale(&mut self, scale: f64) {
        if !(scale.is_finite() && scale > 0.0) || (scale - self.scale).abs() < f64::EPSILON {
            return;
        }
        self.scale = scale;
        let slots: Vec<(Slot, PhysRect)> = self
            .views
            .iter()
            .map(|(s, v)| (s.clone(), v.bounds))
            .collect();
        for (s, b) in slots {
            self.set_view_bounds(&s, b);
        }
    }
}

impl Drop for WebHost {
    fn drop(&mut self) {
        // A download still waiting for a yes is never saved by the host going away.
        for v in self.views.values() {
            if let Some(l) = &v.live {
                l.view.cancel_downloads();
            }
        }
        // The engine stays (rule 1156): what this host closed, or leaves open, finishes closing in its turns.
        if let EngineState::Ready(cef) = &self.engine {
            let mut closing = cef.closing.borrow_mut();
            closing.append(&mut self.closing);
            for (_, v) in self.views.drain() {
                if let Some(l) = v.live {
                    l.view.close();
                    closing.push(l.view);
                }
            }
            for (_, p) in self.popup_views.drain() {
                p.view.close();
                closing.push(p.view);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_script_answer_reads_as_execute_scripts_json() {
        assert_eq!(
            script_result(Ok(r#"{"result":{"type":"number","value":42}}"#.into())),
            Ok("42".into())
        );
        assert_eq!(
            script_result(Ok(r#"{"result":{"type":"undefined"}}"#.into())),
            Ok("null".into())
        );
        assert_eq!(
            script_result(Ok(r#"{"result":{"type":"string","value":"a\"b"}}"#.into())),
            Ok(r#""a\"b""#.into())
        );
        let e = script_result(Ok(
            r#"{"result":{},"exceptionDetails":{"text":"Uncaught","exception":{"description":"Error: no"}}}"#
                .into(),
        ));
        assert_eq!(e, Err("Error: no".into()));
        assert_eq!(script_result(Err("gone".into())), Err("gone".into()));
    }

    #[test]
    fn on_a_mac_cmd_is_the_key_the_chord_table_reads_as_ctrl() {
        let k = KeyInput {
            down: true,
            vk: 0x54,
            mods: Mods {
                cmd: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let p = key_press_of(&k);
        assert!(p.ctrl && p.down && !p.alt);
        let k = KeyInput {
            mods: Mods {
                ctrl: true,
                ..Default::default()
            },
            ..k
        };
        assert!(!key_press_of(&k).ctrl, "the real Ctrl is the page's");
    }

    #[test]
    fn a_key_that_types_nothing_carries_appkits_character() {
        assert_eq!(ns_key_char(0x08), 0x7F, "Backspace");
        assert_eq!(ns_key_char(0x2E), 0xF728, "Forward Delete");
        assert_eq!(ns_key_char(0x25), 0xF702, "Left");
        assert_eq!(ns_key_char(0x70), 0xF704, "F1");
        assert_eq!(ns_key_char(0x7B), 0xF70F, "F12");
        assert_eq!(ns_key_char(0x41), 0);
    }

    #[test]
    fn modifiers_and_buttons_are_cefs_flags() {
        let all = Mods {
            shift: true,
            ctrl: true,
            alt: true,
            cmd: true,
        };
        assert_eq!(
            mod_flags(all),
            flags::SHIFT | flags::CONTROL | flags::ALT | flags::COMMAND
        );
        assert_eq!(mod_flags(Mods::default()), 0);
        assert_eq!(
            button_of(PointerButton::Right),
            (Button::Right, flags::RIGHT_BUTTON)
        );
    }

    #[test]
    fn without_the_framework_beside_it_the_runtime_check_says_why() {
        // The test binary is no app bundle.
        let r = installed_runtime();
        assert!(
            r.as_ref()
                .is_err_and(|e| e.contains("Chromium Embedded Framework.framework")),
            "{r:?}"
        );
    }
}
