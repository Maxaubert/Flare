//! The WebView2 glue: one controller per slot (a tab's open web panel, and for plugins a bar beside a tab or a
//! background page, see `flare_surfaces::slot`), parented to Flare's window. Views, their pending work and
//! their callbacks are keyed by slot, and each slot counts its own generations, so an answer for a view that
//! was closed or replaced is never applied to the slot's next one.
//!
//! Everything runs on the UI thread, which is a COM single-threaded apartment (winit initialises
//! OLE for drag and drop) with the message loop WebView2 needs. Nothing here waits for the browser:
//! the environment and each controller are created asynchronously, their completions arrive through
//! the message loop and are parked in an inbox; the window is told with [`WebEvent::Pump`] and
//! [`WebBackend::pump`] takes them in. A WebView2 callback never touches the host's state, so a
//! callback that fires while a call into the browser is still on the stack cannot re-enter it.
//!
//! No browser exists until the first web panel opens, and the environment goes with the last
//! controller (rule 34 and the anti-goal "panels cost nothing while closed"). A mini-app never shares
//! a browser with the web panel: it runs in an environment of its own, started with the WebRTC policy
//! and, when it has no network, a proxy that is not there (`EnvKind`, rule 199). There is one such
//! environment per kind, made when the first view of the kind opens and closed with the last. A
//! mini-app's view does not navigate until the script that removes the peer-connection API is
//! confirmed installed. The mods and plugins have a browser of their own as well (`EnvKind::Plugin`), apart
//! from the web panel's and the mini-apps', in the folder their profiles were always kept in, and every one of
//! their views is sealed by one function (`seal_plugin_view`, the walls of `crate::seal`).
//!
//! The profile is the folder the caller gives, never the user's own Edge profile (S4). Permissions
//! (camera, microphone, location, notifications and the rest) are denied. A page cannot open a
//! window of its own: `window.open` and `target=_blank` go to the same panel. A navigation to
//! anything but http, https and about:blank is cancelled. A download is held back until the window
//! has asked the person (`WebEvent::Download`, `answer_download`); no answer, a no, a closed panel or
//! a failed registration all mean no file is saved (S6). A process failure that the browser mends
//! itself (GPU, utility, a frame) is only logged; a lost renderer ends that panel's page and a lost
//! browser process ends every page and the environment, so the next open starts fresh.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use flare_mods::serve as modserve;
use flare_surfaces::app::{self, AppSpec};
use flare_surfaces::modview::{self, Route};
use flare_surfaces::netproxy;
use flare_surfaces::web::{
    Failure, PhysRect, failure_of_kind, host_label, navigation_allowed, pdf_view,
};
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{
    AcceleratorKeyPressedEventHandler, AddScriptToExecuteOnDocumentCreatedCompletedHandler,
    CallDevToolsProtocolMethodCompletedHandler, CapturePreviewCompletedHandler,
    ContainsFullScreenElementChangedEventHandler, CoreWebView2EnvironmentOptions,
    CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    DocumentTitleChangedEventHandler, DownloadStartingEventHandler, ExecuteScriptCompletedHandler,
    FocusChangedEventHandler, HistoryChangedEventHandler, LaunchingExternalUriSchemeEventHandler,
    NavigationCompletedEventHandler, NavigationStartingEventHandler,
    NewWindowRequestedEventHandler, PermissionRequestedEventHandler, ProcessFailedEventHandler,
    ScriptDialogOpeningEventHandler, SourceChangedEventHandler, WebMessageReceivedEventHandler,
    WebResourceRequestedEventHandler, WindowCloseRequestedEventHandler, take_pwstr,
};
use windows::Win32::Foundation::{E_POINTER, HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, CoInitializeEx, IStream, STATFLAG_NONAME, STATSTG, STREAM_SEEK_SET,
};
use windows::Win32::UI::Shell::SHCreateMemStream;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, GetClassNameW, GetParent, GetWindowRect,
};
use windows::core::{BOOL, Interface, PCWSTR, PWSTR};

use crate::backend::{
    ControllerProbe, KeyFilter, KeyPress, Nav, Sink, Slot, ViewSink, WebBackend, WebEvent,
};
use crate::seal::{DownloadWall, Seal, plugin_seal};

/// The longest still kept (a PNG of a very large panel).
const STILL_MAX: usize = 48 * 1024 * 1024;

/// Is the Edge WebView2 Runtime installed? The version string, or why not. It reads the registry and the
/// disk, so it runs on a worker thread (rule 34), once per browser that is made.
pub type RuntimeCheck = Arc<dyn Fn() -> Result<String, String> + Send + Sync>;

/// The real check: `GetAvailableCoreWebView2BrowserVersionString`.
pub fn installed_runtime() -> Result<String, String> {
    let mut version = PWSTR::null();
    // SAFETY: `version` is a valid out pointer; on success the loader allocates the string with
    // CoTaskMemAlloc and `take_pwstr` frees it.
    let r = unsafe { GetAvailableCoreWebView2BrowserVersionString(PCWSTR::null(), &mut version) };
    match r {
        Ok(()) => {
            let v = take_pwstr(version);
            if v.is_empty() {
                Err("no WebView2 Runtime version".to_string())
            } else {
                Ok(v)
            }
        }
        Err(e) => Err(e.message()),
    }
}

/// What the host is told at the start: the window it sits in, where its profile is, and (the e2e
/// only) which host names map to which folders.
pub struct HostConfig {
    pub parent: isize,
    pub data_dir: PathBuf,
    pub sink: Sink,
    pub runtime: RuntimeCheck,
    /// Host name to folder mappings added to every view before it navigates (the e2e's local pages;
    /// empty in a real run).
    pub virtual_hosts: Vec<(String, PathBuf)>,
    /// The e2e's seam only: do not add the peer-connection script to a mini-app, so the browser-level
    /// walls (the WebRTC policy and the dead proxy) can be tested on their own.
    pub without_webrtc_script: bool,
    /// The plugins' budgets (design 13): a message from a plugin's page that is over its budget is dropped before
    /// it is copied out of the page. None: no plugin is limited (a run with no mods).
    pub limits: Option<Arc<flare_mods::limit::Limits>>,
}

/// Which browser a view runs in. A mini-app never shares a browser with the web panel, and an app with
/// the network and one without are two browsers (they differ in the arguments they are started with).
/// The mods and plugins have their own (design 13).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum EnvKind {
    /// The web panel's.
    Web,
    /// Mini-apps with no network: the WebRTC policy and a proxy that is not there.
    AppSealed,
    /// Mini-apps that were granted `https:`: the WebRTC policy and Flare's filtering proxy
    /// (`flare_surfaces::netproxy`, issue #97).
    AppNet,
    /// Mods' and plugins' views (no network): the sealed mini-apps' arguments, in the folder every mod's
    /// profile lives in (`modview::env_folder`), so their storage is where it always was.
    Plugin,
    /// The views of a plugin that holds `network` (plugins phase 6): the WebRTC policy, Flare's filtering proxy and a
    /// resolver that knows no local name, in a folder of its own (`modview::net_env_folder`).
    PluginNet,
}

impl EnvKind {
    fn of(app: Option<&AppSpec>) -> Self {
        match app {
            None => Self::Web,
            Some(a) if a.module.is_some() && a.allow_network => Self::PluginNet,
            Some(a) if a.module.is_some() => Self::Plugin,
            Some(a) if a.allow_network => Self::AppNet,
            Some(_) => Self::AppSealed,
        }
    }
}

enum Inbound {
    /// The browser process ended: every view is dead.
    BrowserExited(EnvKind),
    Env {
        kind: EnvKind,
        result: Result<ICoreWebView2Environment, String>,
    },
    /// The mini-app's script wall was (or was not) installed in its view: only now may it navigate.
    Script {
        slot: Slot,
        generation: u64,
        result: Result<(), String>,
    },
    Controller {
        slot: Slot,
        generation: u64,
        result: Result<ICoreWebView2Controller, String>,
    },
}

enum Env {
    /// The runtime question is with a worker.
    Checking,
    Creating,
    Ready(ICoreWebView2Environment),
}

struct Live {
    controller: ICoreWebView2Controller,
    webview: ICoreWebView2,
}

struct View {
    /// Which of the slot's views this is (`State::generations`): a callback carries it, and one for an
    /// earlier view of the slot is let go.
    generation: u64,
    /// A mini-app's controller that exists but may not navigate yet: its script wall is not confirmed.
    pending: Option<Live>,
    /// A mini-app's view (its own private profile, its folder on its own host, a wall round it).
    app: Option<AppSpec>,
    creating: bool,
    live: Option<Live>,
    url: String,
    bounds: PhysRect,
    visible: bool,
}

impl View {
    fn kind(&self) -> EnvKind {
        EnvKind::of(self.app.as_ref())
    }

    /// Closes every controller of the view (live or waiting for its script wall).
    fn shut(self) {
        for l in [self.live, self.pending].into_iter().flatten() {
            // SAFETY: closing a controller we own; the page and its window go with it.
            let _ = unsafe { l.controller.Close() };
        }
    }
}

/// A download the browser started and is holding until it is told yes or no.
struct Download {
    slot: Slot,
    args: ICoreWebView2DownloadStartingEventArgs,
    deferral: ICoreWebView2Deferral,
}

type Downloads = Rc<RefCell<HashMap<u64, Download>>>;

/// A request for a file of a mod's page, held by the browser until the worker that reads the file has
/// answered (the page's request never waits on the UI thread, rule 34).
struct PendingServe {
    slot: Slot,
    /// The page is a plugin's that holds `network`: its answer carries the networked policy.
    network: bool,
    args: ICoreWebView2WebResourceRequestedEventArgs,
    deferral: ICoreWebView2Deferral,
    webview: ICoreWebView2,
}

type Serving = Rc<RefCell<HashMap<u64, PendingServe>>>;
/// What the serve workers have read, by request id, waiting for the UI thread to hand it to the browser.
type Served = Arc<Mutex<VecDeque<(u64, Result<modserve::Served, modserve::Refused>)>>>;
/// File reads for mod pages running at once; the next request is answered 503.
const MAX_SERVE_WORKERS: usize = 8;
/// A message from a mod's page larger than this is dropped here (the bridge refuses over 1 MB itself).
const MAX_PAGE_MESSAGE: usize = 4 * 1024 * 1024;
struct State {
    /// The browsers that exist or are being made, by kind (absent: none).
    envs: HashMap<EnvKind, Env>,
    views: HashMap<Slot, View>,
    /// The last generation each slot gave a view. Kept when the view closes, so the slot's next view never
    /// has the number of one whose callbacks may still arrive (one small entry per slot ever used).
    generations: HashMap<Slot, u64>,
}

pub struct WebHost {
    parent: HWND,
    data_dir: PathBuf,
    sink: Sink,
    runtime: RuntimeCheck,
    virtual_hosts: Vec<(String, PathBuf)>,
    without_webrtc_script: bool,
    limits: Option<Arc<flare_mods::limit::Limits>>,
    state: State,
    inbox: Rc<RefCell<VecDeque<Inbound>>>,
    filter: Rc<RefCell<Option<KeyFilter>>>,
    downloads: Downloads,
    download_seq: Rc<Cell<u64>>,
    serving: Serving,
    serve_seq: Rc<Cell<u64>>,
    served: Served,
    serve_live: Arc<AtomicUsize>,
}

/// Makes a controller on the apps' own private profile (nothing shared with the web panel: no
/// cookies, cache or storage, and nothing kept on disk). A runtime too old to have profiles is an error:
/// a mini-app is never put on the web panel's profile instead.
fn create_app_controller(
    env: &ICoreWebView2Environment,
    parent: HWND,
    handler: &ICoreWebView2CreateCoreWebView2ControllerCompletedHandler,
    module: Option<&str>,
) -> windows::core::Result<()> {
    let e10 = env.cast::<ICoreWebView2Environment10>().map_err(|_| {
        windows::core::Error::new(
            windows::Win32::Foundation::E_NOINTERFACE,
            "this WebView2 Runtime is too old to keep a mini-app apart",
        )
    })?;
    // A mod has a profile of its own that keeps its storage between runs (its notes); a mini-app's is private
    // and keeps nothing. Neither is the web panel's, and neither is Edge's.
    let name = wide(&module.map_or_else(|| app::PROFILE_NAME.to_string(), modview::profile_name));
    // SAFETY: `e10`, the options and the handler are live COM objects on this thread; `name` is
    // NUL-terminated and outlives the call.
    unsafe {
        let options = e10.CreateCoreWebView2ControllerOptions()?;
        options.SetProfileName(PCWSTR(name.as_ptr()))?;
        options.SetIsInPrivateModeEnabled(module.is_none())?;
        e10.CreateCoreWebView2ControllerWithOptions(parent, &options, handler)
    }
}

fn to_rect(r: PhysRect) -> RECT {
    RECT {
        left: r.x,
        top: r.y,
        right: r.right(),
        bottom: r.bottom(),
    }
}

fn from_rect(r: RECT) -> PhysRect {
    PhysRect {
        x: r.left,
        y: r.top,
        w: r.right - r.left,
        h: r.bottom - r.top,
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A string out of a COM `PWSTR` getter.
fn get_string(f: impl FnOnce(*mut PWSTR) -> windows::core::Result<()>) -> String {
    let mut p = PWSTR::null();
    match f(&mut p) {
        Ok(()) => take_pwstr(p),
        Err(_) => String::new(),
    }
}

fn get_bool(f: impl FnOnce(*mut BOOL) -> windows::core::Result<()>) -> bool {
    let mut b = BOOL(0);
    f(&mut b).is_ok() && b.as_bool()
}

/// Is this modifier down in the thread's message queue now.
fn modifier_down(vk: i32) -> bool {
    // SAFETY: GetKeyState takes a plain key code and reads the calling thread's key state.
    unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyState(vk) < 0 }
}

fn read_stream(stream: &IStream) -> Option<Vec<u8>> {
    // SAFETY: `stream` is a live memory stream; `st` and `read` are valid out values and `buf` is as
    // long as the `cb` passed to Read.
    unsafe {
        stream.Seek(0, STREAM_SEEK_SET, None).ok()?;
        let mut st = STATSTG::default();
        stream.Stat(&mut st, STATFLAG_NONAME).ok()?;
        let len = usize::try_from(st.cbSize).ok()?;
        if len == 0 || len > STILL_MAX {
            return None;
        }
        let mut buf = vec![0u8; len];
        let mut read = 0u32;
        let cb = u32::try_from(len).ok()?;
        stream
            .Read(buf.as_mut_ptr().cast(), cb, Some(&mut read))
            .ok()
            .ok()?;
        buf.truncate(read as usize);
        Some(buf)
    }
}

impl WebHost {
    pub fn new(cfg: HostConfig) -> Self {
        // Winit has made this thread a single-threaded apartment already; this only makes sure
        // (S_FALSE, already done, is fine; a thread that is something else fails at the first view).
        // SAFETY: plain COM initialisation of the calling thread.
        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        Self {
            parent: HWND(cfg.parent as *mut std::ffi::c_void),
            data_dir: cfg.data_dir,
            sink: cfg.sink,
            runtime: cfg.runtime,
            virtual_hosts: cfg.virtual_hosts,
            without_webrtc_script: cfg.without_webrtc_script,
            limits: cfg.limits,
            state: State {
                envs: HashMap::new(),
                views: HashMap::new(),
                generations: HashMap::new(),
            },
            inbox: Rc::new(RefCell::new(VecDeque::new())),
            filter: Rc::new(RefCell::new(None)),
            downloads: Rc::new(RefCell::new(HashMap::new())),
            download_seq: Rc::new(Cell::new(0)),
            serving: Rc::new(RefCell::new(HashMap::new())),
            serve_seq: Rc::new(Cell::new(0)),
            served: Arc::new(Mutex::new(VecDeque::new())),
            serve_live: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Ends the downloads still waiting for a yes (all of them, or the slot's): none is saved.
    fn cancel_downloads(&self, slot: Option<&Slot>) {
        let ids: Vec<u64> = self
            .downloads
            .borrow()
            .iter()
            .filter(|(_, d)| slot.is_none_or(|s| d.slot == *s))
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.finish_download(id, false);
        }
    }

    fn finish_download(&self, id: u64, save: bool) {
        let Some(d) = self.downloads.borrow_mut().remove(&id) else {
            return;
        };
        // SAFETY: the event argument and its deferral are live COM objects kept since the callback;
        // the browser saves to its proposed path in Downloads unless this is cancelled.
        unsafe {
            if !save {
                let _ = d.args.SetCancel(true);
            }
            let _ = d.deferral.Complete();
        }
    }

    /// The slot's view cannot be made: nothing of it is kept (a later open tries again, so a runtime
    /// installed meanwhile is found), and the window is told why.
    fn fail(&mut self, slot: &Slot, why: &str, missing_runtime: bool) {
        self.state.views.remove(slot);
        ViewSink::new(&self.sink, slot).send(|tab| WebEvent::Failed {
            tab,
            why: why.to_string(),
            missing_runtime,
        });
    }

    /// The next generation of the slot's view.
    fn next_generation(&mut self, slot: &Slot) -> u64 {
        let g = self.state.generations.entry(slot.clone()).or_insert(0);
        *g += 1;
        *g
    }

    /// Starts the slot's browser (the first time) and, once it exists, the slot's controller.
    fn ensure(&mut self, slot: &Slot) {
        let Some(kind) = self.state.views.get(slot).map(View::kind) else {
            return;
        };
        match self.state.envs.get(&kind) {
            None => {
                let (check, sink) = (Arc::clone(&self.runtime), Arc::clone(&self.sink));
                let job = move || sink(WebEvent::Runtime(check()));
                match std::thread::Builder::new()
                    .name("flare-web-runtime".into())
                    .spawn(job)
                {
                    Ok(_) => {
                        self.state.envs.insert(kind, Env::Checking);
                    }
                    Err(e) => self.fail(
                        slot,
                        &format!("the runtime check could not start: {e}"),
                        false,
                    ),
                }
            }
            Some(Env::Checking | Env::Creating) => {}
            Some(Env::Ready(_)) => self.start_controller(slot),
        }
    }

    /// Makes a browser of `kind` with [`env_arguments`], in a user data folder of its own. A networked kind whose proxy
    /// could not start is refused here, never made without it.
    fn start_env(&self, kind: EnvKind) -> Result<(), String> {
        let arguments = env_arguments(kind)?;
        if kind != EnvKind::Web {
            seal_against_overrides()?;
        }
        let inbox = Rc::clone(&self.inbox);
        let sink = Arc::clone(&self.sink);
        let handler =
            CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(move |err, env| {
                let result = match err {
                    Ok(()) => env.ok_or_else(|| "the browser gave no environment".to_string()),
                    Err(e) => Err(e.message()),
                };
                inbox.borrow_mut().push_back(Inbound::Env { kind, result });
                sink(WebEvent::Pump);
                Ok(())
            }));
        let dir = match kind {
            EnvKind::Web => self.data_dir.clone(),
            EnvKind::AppSealed => app::env_folder(&self.data_dir, false),
            EnvKind::AppNet => app::env_folder(&self.data_dir, true),
            EnvKind::Plugin => modview::env_folder(&self.data_dir),
            EnvKind::PluginNet => modview::net_env_folder(&self.data_dir),
        };
        let folder = wide(&dir.to_string_lossy());
        let o = CoreWebView2EnvironmentOptions::default();
        // SAFETY: the options object is ours alone and not yet shared with the loader.
        unsafe {
            // Thin scrollbars over the page that fade when it is still, as Windows 11 draws them,
            // instead of the classic grey bar with arrows (2026-10-25).
            o.set_scroll_bar_style(COREWEBVIEW2_SCROLLBAR_STYLE_FLUENT_OVERLAY);
            if let Some(arguments) = arguments {
                o.set_additional_browser_arguments(arguments);
                o.set_target_compatible_browser_version(app::APP_MIN_BROWSER.to_string());
            }
        }
        let options: Option<ICoreWebView2EnvironmentOptions> = Some(o.into());
        // SAFETY: `folder` is a NUL-terminated UTF-16 string that outlives the call; the options and
        // the handler are live COM objects the loader keeps alive until it fires.
        unsafe {
            CreateCoreWebView2EnvironmentWithOptions(
                // sync-ok: WebView2 objects are thread-affine to the UI STA; first open only
                PCWSTR::null(),
                PCWSTR(folder.as_ptr()),
                options.as_ref(),
                &handler,
            )
        }
        .map_err(|e| e.message())
    }

    /// The slots whose view runs in the browser of `kind`.
    fn slots_of(&self, kind: EnvKind) -> Vec<Slot> {
        self.state
            .views
            .iter()
            .filter(|(_, v)| v.kind() == kind)
            .map(|(s, _)| s.clone())
            .collect()
    }

    /// Drops the browsers no view uses any more (their processes end with them).
    fn drop_idle_envs(&mut self) {
        let used: HashSet<EnvKind> = self.state.views.values().map(View::kind).collect();
        self.state.envs.retain(|k, _| used.contains(k));
    }

    fn start_controller(&mut self, slot: &Slot) {
        let Some(view) = self.state.views.get_mut(slot) else {
            return;
        };
        let Some(Env::Ready(env)) = self.state.envs.get(&view.kind()) else {
            return;
        };
        if view.creating || view.live.is_some() {
            return;
        }
        view.creating = true;
        let (inbox, sink) = (Rc::clone(&self.inbox), Arc::clone(&self.sink));
        let (held_slot, generation) = (slot.clone(), view.generation);
        let handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
            move |err, controller| {
                let result = match err {
                    Ok(()) => controller.ok_or_else(|| "the browser gave no view".to_string()),
                    Err(e) => Err(e.message()),
                };
                inbox.borrow_mut().push_back(Inbound::Controller {
                    slot: held_slot.clone(),
                    generation,
                    result,
                });
                sink(WebEvent::Pump);
                Ok(())
            },
        ));
        let is_app = view.app.is_some();
        let module = view
            .app
            .as_ref()
            .and_then(|a| a.module.as_ref())
            .map(|m| m.id.clone());
        // SAFETY: `parent` is Flare's own live window on this thread; the handler is a live COM object.
        // sync-ok: WebView2 objects are thread-affine to the UI STA; one call per web panel
        let r = if is_app {
            create_app_controller(env, self.parent, &handler, module.as_deref())
        } else {
            // SAFETY: as above.
            // sync-ok: WebView2 objects are thread-affine to the UI STA; one call per web panel
            unsafe { env.CreateCoreWebView2Controller(self.parent, &handler) }
        };
        if let Err(e) = r {
            let why = e.message();
            self.fail(slot, &why, false);
        }
    }

    /// A controller arrived: set it up and send it where it was asked to go.
    fn adopt(&mut self, slot: &Slot, generation: u64, controller: ICoreWebView2Controller) {
        let current = self
            .state
            .views
            .get(slot)
            .is_some_and(|v| v.generation == generation);
        if !current {
            // The panel closed (or reopened) while the browser was making this one.
            // SAFETY: closing a controller we own.
            let _ = unsafe { controller.Close() };
            return;
        }
        let app = self.state.views.get(slot).and_then(|v| v.app.clone());
        match self.configure(slot, generation, &controller, app.as_ref()) {
            Ok(webview) => {
                let Some(view) = self.state.views.get_mut(slot) else {
                    return;
                };
                let live = Live {
                    controller,
                    webview,
                };
                if app.is_some() {
                    // A mini-app does not navigate until its script wall is confirmed
                    // (`Inbound::Script`): the page is never loaded without it.
                    view.pending = Some(live);
                } else {
                    view.creating = false;
                    Self::go_live(view, live);
                    ViewSink::new(&self.sink, slot).send(|tab| WebEvent::Ready { tab });
                }
            }
            Err(why) => {
                // SAFETY: closing a controller we own.
                let _ = unsafe { controller.Close() };
                self.fail(slot, &why, false);
            }
        }
    }

    /// Puts the view where it was asked to be and sends it to its address.
    fn go_live(view: &mut View, live: Live) {
        let url = wide(&pdf_view(&view.url));
        // SAFETY: `live` holds live COM objects owned here; `url` is NUL-terminated and outlives the call.
        unsafe {
            let _ = live.controller.SetBounds(to_rect(view.bounds));
            let _ = live.controller.SetIsVisible(view.visible);
            let _ = live.webview.Navigate(PCWSTR(url.as_ptr()));
        }
        view.live = Some(live);
    }

    /// The script wall of a mini-app's view came back: on success the view goes live and navigates, on
    /// an error it fails (there is no fallback to a page without the wall).
    fn script_armed(&mut self, slot: &Slot, generation: u64, result: Result<(), String>) {
        let current = self
            .state
            .views
            .get(slot)
            .is_some_and(|v| v.generation == generation && v.pending.is_some());
        if !current {
            return;
        }
        match result {
            Ok(()) => {
                let Some(view) = self.state.views.get_mut(slot) else {
                    return;
                };
                let Some(live) = view.pending.take() else {
                    return;
                };
                view.creating = false;
                Self::go_live(view, live);
                ViewSink::new(&self.sink, slot).send(|tab| WebEvent::Ready { tab });
            }
            Err(why) => {
                if let Some(v) = self.state.views.remove(slot) {
                    v.shut();
                }
                self.fail(slot, &format!("the mini-app's script wall: {why}"), false);
            }
        }
    }

    /// Settings and every handler of one view. The handlers hold the sink and the view's slot and
    /// nothing of the host. A mod's or plugin's view gets its walls from `seal_plugin_view`; the web
    /// panel's and a mini-app's are set here and in `configure_app` as they always were.
    fn configure(
        &self,
        slot: &Slot,
        generation: u64,
        controller: &ICoreWebView2Controller,
        app: Option<&AppSpec>,
    ) -> Result<ICoreWebView2, String> {
        // SAFETY: `controller` is a live COM object.
        let webview = unsafe { controller.CoreWebView2() }
            .map_err(|x| format!("the page: {}", x.message()))?;
        let plugin = app.is_some_and(|a| a.module.is_some());
        // SAFETY: `webview` is live; settings are plain flags.
        unsafe {
            if let Ok(s) = webview.Settings() {
                // A page cannot talk to the app, except a mod's page, over the bridge (`window.flare`).
                let _ = s.SetIsWebMessageEnabled(plugin);
                let _ = s.SetAreHostObjectsAllowed(false);
                if app.is_some() && !plugin {
                    // A mini-app is a page of the agent's: no developer tools of the person's to poke
                    // at it with, no status bar of links (the agent's calls go through the controller).
                    let _ = s.SetAreDevToolsEnabled(false);
                    let _ = s.SetIsStatusBarEnabled(false);
                }
            }
        }
        if let Some(a) = app {
            self.configure_app(&webview, a, slot, generation)?;
            if plugin {
                self.seal_plugin_view(&webview, a, slot, plugin_seal(slot.kind))?;
            }
        }
        for (host, folder) in &self.virtual_hosts {
            if let Ok(w3) = webview.cast::<ICoreWebView2_3>() {
                let (h, f) = (wide(host), wide(&folder.to_string_lossy()));
                // SAFETY: both strings are NUL-terminated and outlive the call.
                let _ = unsafe {
                    w3.SetVirtualHostNameToFolderMapping(
                        PCWSTR(h.as_ptr()),
                        PCWSTR(f.as_ptr()),
                        COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_ALLOW,
                    )
                };
            }
        }
        let mut token = 0i64;

        // ---- navigation ----
        {
            let sink = ViewSink::new(&self.sink, slot);
            // A mini-app's view may only show its own page; a web panel's, any http or https address.
            let app_url = app.map(|a| a.url.clone());
            // A mod's view may only go to its own host (any path of it).
            let mod_host = app.filter(|a| a.module.is_some()).map(|a| a.host.clone());
            let h = NavigationStartingEventHandler::create(Box::new(move |_w, args| {
                let Some(args) = args else { return Ok(()) };
                // SAFETY: `args` is the live event argument of this callback.
                let uri = get_string(|p| unsafe { args.Uri(p) });
                let allowed = match (&mod_host, &app_url) {
                    (Some(h), _) => modview::nav_allowed(&uri, h),
                    (None, Some(u)) => app::nav_allowed(&uri, u),
                    (None, None) => navigation_allowed(&uri),
                };
                if allowed {
                    let mut nav = 0u64;
                    // SAFETY: as above.
                    let _ = unsafe { args.NavigationId(&mut nav) };
                    sink.send(|tab| WebEvent::Loading { tab, url: uri, nav });
                } else {
                    // SAFETY: as above.
                    let _ = unsafe { args.SetCancel(true) };
                    sink.send(|tab| WebEvent::Refused { tab, url: uri });
                }
                Ok(())
            }));
            // SAFETY: `webview` and the handler are live COM objects; `token` is a valid out value.
            let _ = unsafe { webview.add_NavigationStarting(&h, &mut token) };
        }
        {
            let sink = ViewSink::new(&self.sink, slot);
            let h = NavigationCompletedEventHandler::create(Box::new(move |w, args| {
                let (Some(w), Some(args)) = (w, args) else {
                    return Ok(());
                };
                let mut st = COREWEBVIEW2_WEB_ERROR_STATUS(0);
                let mut nav = 0u64;
                // SAFETY: both are the live arguments of this callback.
                let (url, ok) = unsafe {
                    let _ = args.WebErrorStatus(&mut st);
                    let _ = args.NavigationId(&mut nav);
                    (get_string(|p| w.Source(p)), get_bool(|b| args.IsSuccess(b)))
                };
                // A new document clears the title (`WebEvent::Url`); one whose title is the same as the last page's
                // raises no change, so the title is read again once a load has succeeded. Not after a failed one:
                // a sound file "fails" (its stream is handed to the player) and still reports the last page's title.
                if ok {
                    // SAFETY: `w` is the live sender of this callback.
                    let title = get_string(|p| unsafe { w.DocumentTitle(p) });
                    sink.send(|tab| WebEvent::Title { tab, title });
                }
                sink.send(|tab| WebEvent::Loaded {
                    tab,
                    url,
                    ok,
                    status: st.0.to_string(),
                    nav,
                });
                Ok(())
            }));
            // SAFETY: as above.
            let _ = unsafe { webview.add_NavigationCompleted(&h, &mut token) };
        }
        {
            let sink = ViewSink::new(&self.sink, slot);
            let h = SourceChangedEventHandler::create(Box::new(move |w, args| {
                let Some(w) = w else { return Ok(()) };
                // SAFETY: `w` and `args` are the live arguments of this callback.
                let url = get_string(|p| unsafe { w.Source(p) });
                // A change inside the same document raises no navigation: a waiting call hears of it here.
                // SAFETY: as above, `args` is live for the whole callback.
                let new_document = args
                    .as_ref()
                    .is_none_or(|a| get_bool(|b| unsafe { a.IsNewDocument(b) }));
                if new_document {
                    sink.send(|tab| WebEvent::Url { tab, url });
                } else {
                    sink.send(|tab| WebEvent::SameDocument { tab, url });
                }
                Ok(())
            }));
            // SAFETY: as above.
            let _ = unsafe { webview.add_SourceChanged(&h, &mut token) };
        }
        {
            let sink = ViewSink::new(&self.sink, slot);
            let h = HistoryChangedEventHandler::create(Box::new(move |w, _args| {
                let Some(w) = w else { return Ok(()) };
                // SAFETY: `w` is the live sender of this callback.
                let (can_back, can_forward) = unsafe {
                    (
                        get_bool(|b| w.CanGoBack(b)),
                        get_bool(|b| w.CanGoForward(b)),
                    )
                };
                sink.send(|tab| WebEvent::History {
                    tab,
                    can_back,
                    can_forward,
                });
                Ok(())
            }));
            // SAFETY: as above.
            let _ = unsafe { webview.add_HistoryChanged(&h, &mut token) };
        }
        {
            let sink = ViewSink::new(&self.sink, slot);
            let h = DocumentTitleChangedEventHandler::create(Box::new(move |w, _args| {
                let Some(w) = w else { return Ok(()) };
                // SAFETY: `w` is the live sender of this callback.
                let title = get_string(|p| unsafe { w.DocumentTitle(p) });
                sink.send(|tab| WebEvent::Title { tab, title });
                Ok(())
            }));
            // SAFETY: as above.
            let _ = unsafe { webview.add_DocumentTitleChanged(&h, &mut token) };
        }
        if !plugin {
            // A page cannot open a window: the address goes to this panel (if it may open at all).
            // A mini-app's links open nowhere: it is one page, and the browser panel is the person's to
            // point somewhere.
            refuse_new_windows(&webview, ViewSink::new(&self.sink, slot), app.is_none());
            deny_permissions(&webview);
        }
        {
            let (sink, inbox) = (Arc::clone(&self.sink), Rc::clone(&self.inbox));
            let events = ViewSink::new(&self.sink, slot);
            let env_kind = EnvKind::of(app);
            let h = ProcessFailedEventHandler::create(Box::new(move |_w, args| {
                let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND(-1);
                if let Some(args) = args {
                    // SAFETY: `args` is the live argument of this callback.
                    let _ = unsafe { args.ProcessFailedKind(&mut kind) };
                }
                match failure_of_kind(kind.0) {
                    // The browser restarts it and the page keeps working.
                    Failure::Recovers => {
                        log::info!("a browser process restarted (kind {})", kind.0)
                    }
                    Failure::Page => events.send(|tab| WebEvent::Gone {
                        tab,
                        why: "the page's process ended".to_string(),
                    }),
                    Failure::Browser => {
                        inbox
                            .borrow_mut()
                            .push_back(Inbound::BrowserExited(env_kind));
                        sink(WebEvent::Pump);
                    }
                }
                Ok(())
            }));
            // SAFETY: as above.
            let _ = unsafe { webview.add_ProcessFailed(&h, &mut token) };
        }
        // `window.close()` does nothing here. A page that puts an element in full screen (a video's button) is
        // told to the app, which makes the page cover the window until the page leaves it.
        {
            let h = WindowCloseRequestedEventHandler::create(Box::new(move |_w, _a| Ok(())));
            // SAFETY: as above.
            let _ = unsafe { webview.add_WindowCloseRequested(&h, &mut token) };
            let sink = ViewSink::new(&self.sink, slot);
            let h = ContainsFullScreenElementChangedEventHandler::create(Box::new(move |w, _a| {
                let Some(w) = w else { return Ok(()) };
                // SAFETY: `w` is the live sender of this callback.
                let on = unsafe { get_bool(|b| w.ContainsFullScreenElement(b)) };
                sink.send(|tab| WebEvent::FullScreen { tab, on });
                Ok(())
            }));
            // SAFETY: as above.
            let _ = unsafe { webview.add_ContainsFullScreenElementChanged(&h, &mut token) };
        }
        if !plugin {
            self.hold_downloads(&webview, slot)?;
        }

        // ---- keys and focus, on the controller ----
        {
            let (sink, filter) = (ViewSink::new(&self.sink, slot), Rc::clone(&self.filter));
            let h = AcceleratorKeyPressedEventHandler::create(Box::new(move |_c, args| {
                let Some(args) = args else {
                    return Err(E_POINTER.into());
                };
                let mut kind = COREWEBVIEW2_KEY_EVENT_KIND(0);
                let mut vk = 0u32;
                // SAFETY: `args` is the live argument of this callback.
                unsafe {
                    let _ = args.KeyEventKind(&mut kind);
                    let _ = args.VirtualKey(&mut vk);
                }
                let down = kind == COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN
                    || kind == COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN;
                if !down {
                    return Ok(());
                }
                let mut status = COREWEBVIEW2_PHYSICAL_KEY_STATUS::default();
                // SAFETY: as above; `status` is a valid out value.
                let _ = unsafe { args.PhysicalKeyStatus(&mut status) };
                let key = KeyPress {
                    vk,
                    ctrl: modifier_down(0x11),
                    shift: modifier_down(0x10),
                    alt: modifier_down(0x12),
                    down,
                    repeat: status.WasKeyDown.as_bool(),
                };
                let taken = filter.borrow().as_ref().is_some_and(|f| f(&key));
                if taken {
                    // SAFETY: as above.
                    let _ = unsafe { args.SetHandled(true) };
                    sink.send(|tab| WebEvent::Key { tab, key });
                }
                Ok(())
            }));
            // SAFETY: `controller` and the handler are live COM objects.
            let _ = unsafe { controller.add_AcceleratorKeyPressed(&h, &mut token) };
        }
        for has in [true, false] {
            let sink = ViewSink::new(&self.sink, slot);
            let h = FocusChangedEventHandler::create(Box::new(move |_c, _a| {
                sink.send(|tab| WebEvent::Focus { tab, has });
                Ok(())
            }));
            // SAFETY: as above.
            let _ = unsafe {
                if has {
                    controller.add_GotFocus(&h, &mut token)
                } else {
                    controller.add_LostFocus(&h, &mut token)
                }
            };
        }
        Ok(webview)
    }

    /// A download is held until the person has said yes (S6): the browser would otherwise save it
    /// into Downloads at once. A runtime too old to hold one back cannot show a web panel at all.
    fn hold_downloads(&self, webview: &ICoreWebView2, slot: &Slot) -> Result<(), String> {
        let w4 = webview.cast::<ICoreWebView2_4>().map_err(|_| {
            "this WebView2 Runtime cannot hold a download for a question".to_string()
        })?;
        let (sink, held_slot) = (ViewSink::new(&self.sink, slot), slot.clone());
        let (held, seq) = (Rc::clone(&self.downloads), Rc::clone(&self.download_seq));
        let h = DownloadStartingEventHandler::create(Box::new(move |_w, args| {
            let Some(args) = args else { return Ok(()) };
            // SAFETY: `args` is the live argument of this callback.
            let deferral = unsafe { args.GetDeferral() };
            let Ok(deferral) = deferral else {
                // No way to wait for the person: no file.
                // SAFETY: as above.
                let _ = unsafe { args.SetCancel(true) };
                return Ok(());
            };
            // SAFETY: as above.
            let (path, uri) = unsafe {
                let uri = args
                    .DownloadOperation()
                    .map(|op| get_string(|p| op.Uri(p)))
                    .unwrap_or_default();
                (get_string(|p| args.ResultFilePath(p)), uri)
            };
            let file = path
                .rsplit(['\\', '/'])
                .next()
                .filter(|f| !f.is_empty())
                .unwrap_or("a file")
                .to_string();
            seq.set(seq.get() + 1);
            let id = seq.get();
            held.borrow_mut().insert(
                id,
                Download {
                    slot: held_slot.clone(),
                    args: args.clone(),
                    deferral,
                },
            );
            sink.send(|tab| WebEvent::Download {
                tab,
                id,
                file,
                host: host_label(&uri),
            });
            Ok(())
        }));
        let mut token = 0i64;
        // SAFETY: `w4` and the handler are live COM objects; `token` is a valid out value.
        if unsafe { w4.add_DownloadStarting(&h, &mut token) }.is_err() {
            return Err("the download question could not be registered".to_string());
        }
        Ok(())
    }

    /// The one function that seals a mod's or plugin's view, whatever its slot (design 13): every wall of
    /// `seal` (see `crate::seal`) is put up here, and a view whose walls cannot all be put up fails (there is no
    /// view with fewer). The network wall, the file server and the bridge are `configure_app`'s.
    fn seal_plugin_view(
        &self,
        webview: &ICoreWebView2,
        app: &AppSpec,
        slot: &Slot,
        seal: Seal,
    ) -> Result<(), String> {
        let refused = |what: &str| format!("this WebView2 Runtime cannot seal a plugin's {what}");
        // SAFETY: `webview` is live; settings are plain flags.
        unsafe {
            let s = webview.Settings().map_err(|_| refused("settings"))?;
            let _ = s.SetAreDevToolsEnabled(seal.dev_tools);
            let _ = s.SetIsStatusBarEnabled(seal.status_bar);
            let _ = s.SetAreDefaultContextMenusEnabled(seal.context_menu);
            // With the browser's dialogs off, a dialog is raised as `ScriptDialogOpening`; a handler that does
            // not accept it dismisses it (`alert` returns, `confirm` is false, `prompt` is null).
            let _ = s.SetAreDefaultScriptDialogsEnabled(seal.script_dialogs);
            if !seal.browser_keys {
                s.cast::<ICoreWebView2Settings3>()
                    .and_then(|s3| s3.SetAreBrowserAcceleratorKeysEnabled(false))
                    .map_err(|_| refused("keys"))?;
            }
        }
        if !seal.script_dialogs {
            let h = ScriptDialogOpeningEventHandler::create(Box::new(move |_w, _args| {
                log::debug!("a plugin's script dialog was dismissed");
                Ok(())
            }));
            let mut t = 0i64;
            // SAFETY: the view and the handler are live COM objects; `t` is a valid out value.
            unsafe { webview.add_ScriptDialogOpening(&h, &mut t) }
                .map_err(|_| refused("dialogs"))?;
        }
        if !seal.new_windows {
            refuse_new_windows(webview, ViewSink::new(&self.sink, slot), false);
        }
        if !seal.permissions {
            deny_permissions(webview);
        }
        match seal.downloads {
            DownloadWall::Ask => self.hold_downloads(webview, slot)?,
            DownloadWall::Cancel => cancel_downloads_at_once(webview)?,
        }
        if !seal.external_uris {
            refuse_external_uris(webview, ViewSink::new(&self.sink, slot))?;
        }
        if seal.frames_own_only {
            own_frames_only(webview, app)?;
        }
        Ok(())
    }

    /// What makes a view a mini-app's: its folder served on its own host name, and every request that
    /// is not the app's own refused (the second wall, behind the page's policy). A mod's view (stage 8)
    /// has the same walls with two differences: its files are NOT a folder mapping (each request is
    /// checked by `flare_mods::serve` on a worker, so a junction in the folder serves nothing outside it),
    /// and its page gets the `window.flare` bridge and a message channel to the host. A mod's launch and
    /// frame walls are put up with the rest of its seal (`seal_plugin_view`).
    fn configure_app(
        &self,
        webview: &ICoreWebView2,
        app: &AppSpec,
        slot: &Slot,
        generation: u64,
    ) -> Result<(), String> {
        let module = app.module.is_some();
        if !module {
            let w3 = webview
                .cast::<ICoreWebView2_3>()
                .map_err(|_| "this WebView2 Runtime cannot serve a mini-app".to_string())?;
            let (h, f) = (wide(&app.host), wide(&app.folder.to_string_lossy()));
            // SAFETY: both strings are NUL-terminated and outlive the call.
            unsafe {
                w3.SetVirtualHostNameToFolderMapping(
                    PCWSTR(h.as_ptr()),
                    PCWSTR(f.as_ptr()),
                    COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_DENY_CORS,
                )
            }
            .map_err(|e| format!("the mini-app's folder: {}", e.message()))?;
        }
        let (host, allow) = (app.host.clone(), app.allow_network);
        let files = app
            .module
            .as_ref()
            .map(|m| (m.entry.clone(), app.folder.clone()));
        let serve = ServeCtx {
            serving: Rc::clone(&self.serving),
            seq: Rc::clone(&self.serve_seq),
            served: Arc::clone(&self.served),
            live: Arc::clone(&self.serve_live),
            sink: Arc::clone(&self.sink),
            slot: slot.clone(),
            network: allow,
        };
        let h = WebResourceRequestedEventHandler::create(Box::new(move |w, args| {
            let (Some(w), Some(args)) = (w, args) else {
                return Ok(());
            };
            // SAFETY: both are the live arguments of this callback.
            unsafe {
                let Ok(req) = args.Request() else {
                    return Ok(());
                };
                let uri = get_string(|p| req.Uri(p));
                if let Some((entry, folder)) = &files {
                    // A plugin that holds `network` reaches public https hosts, and nothing local (plugins phase 6).
                    match modview::route_request_net(&uri, &host, allow) {
                        Route::Pass => {}
                        Route::Refuse => refuse_request(&w, &args, 403),
                        Route::Serve(path) => serve.start(&w, &args, entry, folder, path),
                    }
                    return Ok(());
                }
                if app::request_allowed(&uri, &host, allow) {
                    return Ok(());
                }
                // Answered here with a refusal: the request never leaves the PC.
                refuse_request(&w, &args, 403);
            }
            Ok(())
        }));
        let star = wide("*");
        let mut token = 0i64;
        // SAFETY: `webview` and the handler are live COM objects; `star` is NUL-terminated and
        // outlives the call; `token` is a valid out value.
        unsafe {
            // Where the runtime has it, the filter covers requests of every source (shared and
            // service workers too), not only the document's.
            let wide_filter = webview.cast::<ICoreWebView2_22>().and_then(|w22| {
                w22.AddWebResourceRequestedFilterWithRequestSourceKinds(
                    PCWSTR(star.as_ptr()),
                    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                    COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
                )
            });
            if wide_filter.is_err() {
                webview
                    .AddWebResourceRequestedFilter(
                        PCWSTR(star.as_ptr()),
                        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                    )
                    .map_err(|e| format!("the mini-app's wall: {}", e.message()))?;
            }
            webview
                .add_WebResourceRequested(&h, &mut token)
                .map_err(|e| format!("the mini-app's wall: {}", e.message()))?;
            // No peer connections. The policy has no directive for them and their address lookups never
            // reach the wall, so the page loses the API before its own code runs. This is the first
            // of three walls (the others are the browser's own: the WebRTC policy and, with no
            // network, a proxy that is not there). A mod's page also gets its bridge here, in the same
            // script. The completion is awaited: the view navigates only when `Inbound::Script` says
            // it is installed, and fails if it is not.
            let mut text = String::new();
            if !self.without_webrtc_script {
                text.push_str(app::NO_WEBRTC_SCRIPT);
            }
            if module {
                text.push('\n');
                text.push_str(flare_mods::bridge::BRIDGE_JS);
            }
            if text.is_empty() {
                // The e2e's seam: the browser-level walls are tested alone.
                self.inbox.borrow_mut().push_back(Inbound::Script {
                    slot: slot.clone(),
                    generation,
                    result: Ok(()),
                });
                (self.sink)(WebEvent::Pump);
            } else {
                let script = wide(&text);
                let (inbox, sink) = (Rc::clone(&self.inbox), Arc::clone(&self.sink));
                let held_slot = slot.clone();
                let done = AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(
                    move |err, _id| {
                        inbox.borrow_mut().push_back(Inbound::Script {
                            slot: held_slot.clone(),
                            generation,
                            result: err.map_err(|e| e.message()),
                        });
                        sink(WebEvent::Pump);
                        Ok(())
                    },
                ));
                webview
                    .AddScriptToExecuteOnDocumentCreated(PCWSTR(script.as_ptr()), &done)
                    .map_err(|e| format!("the mini-app's script wall: {}", e.message()))?;
            }
        }
        // A mod's page talks to the host over web messages, from its own address only.
        if module {
            let (sink, host) = (ViewSink::new(&self.sink, slot), app.host.clone());
            let (limits, id) = (
                self.limits.clone(),
                app.module.as_ref().map(|m| m.id.clone()),
            );
            let h = WebMessageReceivedEventHandler::create(Box::new(move |_w, args| {
                let Some(args) = args else { return Ok(()) };
                // A plugin's pages share one budget (protocol 1 mods have none): a message over it is dropped
                // here, before it is copied out of the page. The window spends what it takes.
                let limiter = limits
                    .as_ref()
                    .zip(id.as_deref())
                    .and_then(|(l, id)| l.of(id));
                if limiter
                    .as_ref()
                    .is_some_and(|l| !l.has_room(std::time::Instant::now()))
                {
                    return Ok(());
                }
                let cap = if limiter.is_some() {
                    flare_mods::limit::MAX_MESSAGE
                } else {
                    MAX_PAGE_MESSAGE
                };
                // SAFETY: `args` is the live argument of this callback.
                let (source, text) = unsafe {
                    (
                        get_string(|p| args.Source(p)),
                        get_string(|p| args.TryGetWebMessageAsString(p)),
                    )
                };
                if modview::nav_allowed(&source, &host) && !text.is_empty() && text.len() <= cap {
                    sink.send(|tab| WebEvent::ModMessage {
                        tab,
                        host: host.clone(),
                        json: text,
                    });
                }
                Ok(())
            }));
            let mut t = 0i64;
            // SAFETY: the view and the handler are live COM objects; `t` is a valid out value.
            unsafe { webview.add_WebMessageReceived(&h, &mut t) }
                .map_err(|e| format!("the mod's message channel: {}", e.message()))?;
        } else {
            // A mini-app's launch and frame walls (a mod's are part of its seal).
            refuse_external_uris(webview, ViewSink::new(&self.sink, slot))?;
            own_frames_only(webview, app)?;
        }
        Ok(())
    }

    /// A serve worker has read a file (or been refused): the browser is handed the answer and released.
    fn finish_serve(&self, id: u64, result: Result<modserve::Served, modserve::Refused>) {
        let Some(p) = self.serving.borrow_mut().remove(&id) else {
            return;
        };
        answer_request(&p.webview, &p.args, result, p.network);
        // SAFETY: the deferral is a live COM object kept since the callback.
        let _ = unsafe { p.deferral.Complete() };
    }

    /// Releases the page requests still waiting for a file read (all, or the slot's), each answered 503.
    fn cancel_serves(&self, slot: Option<&Slot>) {
        let ids: Vec<u64> = self
            .serving
            .borrow()
            .iter()
            .filter(|(_, p)| slot.is_none_or(|s| p.slot == *s))
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.finish_serve(
                id,
                Err(modserve::Refused {
                    status: 503,
                    reason: "the page was closed".into(),
                }),
            );
        }
    }

    fn live(&self, slot: &Slot) -> Option<&Live> {
        self.state.views.get(slot).and_then(|v| v.live.as_ref())
    }

    /// The live view of the tab's panel.
    fn panel(&self, tab: &str) -> Option<&Live> {
        self.live(&Slot::panel(tab))
    }

    fn child_windows(&self) -> Vec<(String, PhysRect)> {
        struct Acc {
            parent: HWND,
            out: Vec<(String, PhysRect)>,
        }
        unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> BOOL {
            // SAFETY: `lparam` is the `&mut Acc` passed below, alive for the whole enumeration on
            // this thread.
            let acc = unsafe { &mut *(lparam.0 as *mut Acc) };
            // SAFETY: plain window queries on a handle the enumeration just gave.
            unsafe {
                if GetParent(hwnd).ok() == Some(acc.parent) {
                    let mut name = [0u16; 128];
                    let n = usize::try_from(GetClassNameW(hwnd, &mut name)).unwrap_or(0);
                    let class = String::from_utf16_lossy(&name[..n]);
                    let mut r = RECT::default();
                    if GetWindowRect(hwnd, &mut r).is_ok() {
                        let mut tl = POINT {
                            x: r.left,
                            y: r.top,
                        };
                        let mut br = POINT {
                            x: r.right,
                            y: r.bottom,
                        };
                        let _ = ScreenToClient(acc.parent, &mut tl);
                        let _ = ScreenToClient(acc.parent, &mut br);
                        acc.out.push((
                            class,
                            from_rect(RECT {
                                left: tl.x,
                                top: tl.y,
                                right: br.x,
                                bottom: br.y,
                            }),
                        ));
                    }
                }
            }
            BOOL(1)
        }
        let mut acc = Acc {
            parent: self.parent,
            out: Vec::new(),
        };
        // SAFETY: `acc` outlives the call and the callback only runs inside it, on this thread.
        let _ = unsafe {
            EnumChildWindows(
                Some(self.parent),
                Some(each),
                LPARAM(std::ptr::addr_of_mut!(acc) as isize),
            )
        };
        acc.out
    }
}

/// What the browser's request handler of a mod view needs to hand a file read to a worker.
struct ServeCtx {
    serving: Serving,
    seq: Rc<Cell<u64>>,
    served: Served,
    live: Arc<AtomicUsize>,
    sink: Sink,
    slot: Slot,
    network: bool,
}

impl ServeCtx {
    /// Holds the request and starts the worker that reads its file; the answer comes back through `served`
    /// and `WebEvent::Pump`.
    ///
    /// # Safety
    /// `w` and `args` are the live arguments of the browser's callback.
    unsafe fn start(
        &self,
        w: &ICoreWebView2,
        args: &ICoreWebView2WebResourceRequestedEventArgs,
        entry: &str,
        folder: &std::path::Path,
        path: String,
    ) {
        // SAFETY: `args` is live per the contract above.
        let Ok(deferral) = (unsafe { args.GetDeferral() }) else {
            refuse_request(w, args, 500);
            return;
        };
        self.seq.set(self.seq.get() + 1);
        let id = self.seq.get();
        self.serving.borrow_mut().insert(
            id,
            PendingServe {
                slot: self.slot.clone(),
                network: self.network,
                args: args.clone(),
                deferral,
                webview: w.clone(),
            },
        );
        let post = |served: &Served, sink: &Sink, r| {
            served
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push_back((id, r));
            sink(WebEvent::Pump);
        };
        if self.live.fetch_add(1, Ordering::SeqCst) >= MAX_SERVE_WORKERS {
            self.live.fetch_sub(1, Ordering::SeqCst);
            post(
                &self.served,
                &self.sink,
                Err(modserve::Refused {
                    status: 503,
                    reason: "too many file reads at once".into(),
                }),
            );
            return;
        }
        let (served, sink, live) = (
            Arc::clone(&self.served),
            Arc::clone(&self.sink),
            Arc::clone(&self.live),
        );
        let (entry, folder) = (entry.to_string(), folder.to_path_buf());
        let spawned = std::thread::Builder::new()
            .name("flare-mod-serve".into())
            .spawn({
                let (served, sink, live) = (served.clone(), sink.clone(), live.clone());
                move || {
                    let r = modserve::serve_file(&folder, &entry, &path);
                    served
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push_back((id, r));
                    sink(WebEvent::Pump);
                    live.fetch_sub(1, Ordering::SeqCst);
                }
            });
        if spawned.is_err() {
            live.fetch_sub(1, Ordering::SeqCst);
            post(
                &served,
                &sink,
                Err(modserve::Refused {
                    status: 500,
                    reason: "the file reader could not start".into(),
                }),
            );
        }
    }
}

/// Answers a page request with a status and nothing else (the request never reaches the network).
fn refuse_request(
    w: &ICoreWebView2,
    args: &ICoreWebView2WebResourceRequestedEventArgs,
    status: i32,
) {
    answer_request(
        w,
        args,
        Err(modserve::Refused {
            status: status as u16,
            reason: String::new(),
        }),
        false,
    );
}

/// Sets the browser's answer to a request: the file with its headers, or the refusal's status.
fn answer_request(
    w: &ICoreWebView2,
    args: &ICoreWebView2WebResourceRequestedEventArgs,
    result: Result<modserve::Served, modserve::Refused>,
    network: bool,
) {
    let (status, reason, headers, bytes) = match &result {
        Ok(s) => (
            200,
            "OK",
            modserve::headers_for(s.mime, network),
            Some(s.bytes.as_slice()),
        ),
        Err(r) => (
            i32::from(r.status),
            match r.status {
                403 => "Blocked by Flare",
                404 => "Not found",
                413 => "Too large",
                503 => "Busy",
                _ => "Refused",
            },
            modserve::refusal_headers(),
            None,
        ),
    };
    let (reason, headers) = (wide(reason), wide(&headers));
    // SAFETY: a memory stream over the bytes (copied by the call) and live COM objects; both strings are
    // NUL-terminated and outlive the call.
    unsafe {
        let stream = bytes.and_then(|b| SHCreateMemStream(Some(b)));
        if let Ok(env) = w.cast::<ICoreWebView2_2>().and_then(|w2| w2.Environment())
            && let Ok(resp) = env.CreateWebResourceResponse(
                stream.as_ref(),
                status,
                PCWSTR(reason.as_ptr()),
                PCWSTR(headers.as_ptr()),
            )
        {
            let _ = args.SetResponse(&resp);
        }
    }
}

// ---- the walls a view's set-up and the plugins' seal share ----

/// A page cannot open a window of its own. With `in_place` (the web panel) an address that may open
/// is sent to the same view; otherwise (a mini-app, a mod or plugin) it opens nowhere and the window is told.
fn refuse_new_windows(webview: &ICoreWebView2, sink: ViewSink, in_place: bool) {
    let h = NewWindowRequestedEventHandler::create(Box::new(move |w, args| {
        let (Some(w), Some(args)) = (w, args) else {
            return Ok(());
        };
        // SAFETY: both are the live arguments of this callback.
        unsafe {
            let uri = get_string(|p| args.Uri(p));
            let _ = args.SetHandled(true);
            if in_place && navigation_allowed(&uri) {
                let u = wide(&pdf_view(&uri));
                let _ = w.Navigate(PCWSTR(u.as_ptr()));
            } else {
                sink.send(|tab| WebEvent::Refused { tab, url: uri });
            }
        }
        Ok(())
    }));
    let mut token = 0i64;
    // SAFETY: `webview` and the handler are live COM objects; `token` is a valid out value.
    let _ = unsafe { webview.add_NewWindowRequested(&h, &mut token) };
}

/// Every permission is denied (camera, microphone, location, notifications, clipboard ...).
fn deny_permissions(webview: &ICoreWebView2) {
    let h = PermissionRequestedEventHandler::create(Box::new(move |_w, args| {
        if let Some(args) = args {
            // SAFETY: `args` is the live argument of this callback.
            let _ = unsafe { args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY) };
        }
        Ok(())
    }));
    let mut token = 0i64;
    // SAFETY: `webview` and the handler are live COM objects; `token` is a valid out value.
    let _ = unsafe { webview.add_PermissionRequested(&h, &mut token) };
}

/// A download is cancelled as it starts (a view with nobody to ask): nothing is saved, nothing is asked.
fn cancel_downloads_at_once(webview: &ICoreWebView2) -> Result<(), String> {
    let w4 = webview
        .cast::<ICoreWebView2_4>()
        .map_err(|_| "this WebView2 Runtime cannot refuse a download".to_string())?;
    let h = DownloadStartingEventHandler::create(Box::new(move |_w, args| {
        if let Some(args) = args {
            // SAFETY: `args` is the live argument of this callback.
            let _ = unsafe { args.SetCancel(true) };
        }
        Ok(())
    }));
    let mut token = 0i64;
    // SAFETY: `w4` and the handler are live COM objects; `token` is a valid out value.
    unsafe { w4.add_DownloadStarting(&h, &mut token) }
        .map_err(|_| "the download wall could not be registered".to_string())
}

/// No external program is launched from the page (a mailto: or custom scheme would carry the page's data out
/// through another program, past the proxy).
fn refuse_external_uris(webview: &ICoreWebView2, sink: ViewSink) -> Result<(), String> {
    let w18 = webview
        .cast::<ICoreWebView2_18>()
        .map_err(|_| "this WebView2 Runtime cannot seal a mini-app".to_string())?;
    let h = LaunchingExternalUriSchemeEventHandler::create(Box::new(move |_w, args| {
        if let Some(args) = args {
            // SAFETY: `args` is the live argument of this callback.
            unsafe {
                let uri = get_string(|p| args.Uri(p));
                let _ = args.SetCancel(true);
                sink.send(|tab| WebEvent::Refused { tab, url: uri });
            }
        }
        Ok(())
    }));
    let mut t = 0i64;
    // SAFETY: the view and the handler are live COM objects; `t` is a valid out value.
    unsafe { w18.add_LaunchingExternalUriScheme(&h, &mut t) }
        .map_err(|e| format!("the mini-app's launch wall: {}", e.message()))
}

/// No frame navigates anywhere but a blank or srcdoc frame or the page's own: the app's page, or any page of
/// a mod's own host. (No cookie sweep: each app has a site of its own, and wiping the shared profile's jar
/// would log another app's tab out.)
fn own_frames_only(webview: &ICoreWebView2, app: &AppSpec) -> Result<(), String> {
    let app_url = app.url.clone();
    let mod_host = app.module.is_some().then(|| app.host.clone());
    let h = NavigationStartingEventHandler::create(Box::new(move |_w, args| {
        if let Some(args) = args {
            // SAFETY: `args` is the live argument of this callback.
            unsafe {
                let uri = get_string(|p| args.Uri(p));
                let own = match &mod_host {
                    Some(h) => modview::nav_allowed(&uri, h),
                    None => app::nav_allowed(&uri, &app_url),
                };
                if !(uri == "about:blank" || uri == "about:srcdoc" || own) {
                    let _ = args.SetCancel(true);
                }
            }
        }
        Ok(())
    }));
    let mut t = 0i64;
    // SAFETY: the view and the handler are live COM objects; `t` is a valid out value.
    unsafe { webview.add_FrameNavigationStarting(&h, &mut t) }
        .map_err(|e| format!("the mini-app's frame wall: {}", e.message()))
}

impl Drop for WebHost {
    fn drop(&mut self) {
        // A download still waiting for a yes is never saved by the host going away.
        self.cancel_downloads(None);
        self.cancel_serves(None);
    }
}

impl WebHost {
    /// Closes the slot's view and frees its controller; the browser environment stays.
    fn drop_view(&mut self, slot: &Slot) {
        self.cancel_downloads(Some(slot));
        self.cancel_serves(Some(slot));
        if let Some(view) = self.state.views.remove(slot) {
            view.shut();
        }
    }

    /// Opens the slot's view for `url`, as a mini-app's (or a mod's) when `app` is given. A view of the other
    /// kind is closed first: a web page never gets an app's wall and an app is never left on the web panel's
    /// profile.
    fn put_view(
        &mut self,
        slot: &Slot,
        url: &str,
        app: Option<AppSpec>,
        bounds: PhysRect,
        visible: bool,
    ) {
        if self.state.views.get(slot).is_some_and(|v| {
            v.kind() != EnvKind::of(app.as_ref())
                || v.app.as_ref().map(|a| &a.host) != app.as_ref().map(|a| &a.host)
        }) {
            // Swapped in the same browser: the environment stays, so a mini-app updated again and
            // again does not restart it.
            self.drop_view(slot);
        }
        if let Some(v) = self.state.views.get_mut(slot) {
            v.url = url.to_string();
            v.bounds = bounds;
            v.visible = visible;
            if v.live.is_some() {
                self.set_view_bounds(slot, bounds);
                self.set_view_visible(slot, visible);
                self.go_to(slot, url);
            }
            return;
        }
        let generation = self.next_generation(slot);
        self.state.views.insert(
            slot.clone(),
            View {
                generation,
                pending: None,
                app,
                creating: false,
                live: None,
                url: url.to_string(),
                bounds,
                visible,
            },
        );
        // A browser the swapped-out view alone used goes now (a different kind of view was put in).
        self.drop_idle_envs();
        self.ensure(slot);
    }

    /// Sends the slot's view to `url`.
    fn go_to(&mut self, slot: &Slot, url: &str) {
        if let Some(v) = self.state.views.get_mut(slot) {
            v.url = url.to_string();
        }
        if let Some(l) = self.live(slot) {
            let u = wide(&pdf_view(url));
            // SAFETY: `webview` is live; `u` is NUL-terminated and outlives the call.
            let _ = unsafe { l.webview.Navigate(PCWSTR(u.as_ptr())) };
        }
    }
}

impl WebBackend for WebHost {
    fn open(&mut self, tab: &str, url: &str, bounds: PhysRect, visible: bool) {
        self.put_view(&Slot::panel(tab), url, None, bounds, visible);
    }

    fn open_view(&mut self, slot: &Slot, app: AppSpec, bounds: PhysRect, visible: bool) {
        if !slot.is_panel() && app.module.is_none() {
            // A bar or a background page is a plugin's only: no view is made for anything else.
            self.drop_view(slot);
            self.fail(slot, "only a plugin's page opens here", false);
            self.drop_idle_envs();
            return;
        }
        let url = app.url.clone();
        self.put_view(slot, &url, Some(app), bounds, visible);
    }

    fn navigate(&mut self, tab: &str, url: &str) {
        self.go_to(&Slot::panel(tab), url);
    }

    fn go(&mut self, tab: &str, nav: Nav) {
        if let Some(l) = self.panel(tab) {
            // SAFETY: `webview` is a live COM object.
            let _ = unsafe {
                match nav {
                    Nav::Back => l.webview.GoBack(),
                    Nav::Forward => l.webview.GoForward(),
                    Nav::Reload => l.webview.Reload(),
                    Nav::Stop => l.webview.Stop(),
                }
            };
        }
    }

    fn set_view_bounds(&mut self, slot: &Slot, bounds: PhysRect) {
        if let Some(v) = self.state.views.get_mut(slot) {
            v.bounds = bounds;
        }
        if let Some(l) = self.live(slot) {
            // SAFETY: `controller` is a live COM object.
            let _ = unsafe { l.controller.SetBounds(to_rect(bounds)) };
        }
    }

    fn parent_moved(&mut self) {
        for v in self.state.views.values() {
            if let Some(l) = &v.live {
                // SAFETY: `controller` is a live COM object.
                let _ = unsafe { l.controller.NotifyParentWindowPositionChanged() };
            }
        }
    }

    fn set_view_visible(&mut self, slot: &Slot, visible: bool) {
        if let Some(v) = self.state.views.get_mut(slot) {
            v.visible = visible;
        }
        if let Some(l) = self.live(slot) {
            // SAFETY: `controller` is a live COM object.
            let _ = unsafe { l.controller.SetIsVisible(visible) };
        }
    }

    fn capture_still(&mut self, tab: &str, token: u64) {
        let none = |sink: &Sink| {
            sink(WebEvent::Still {
                tab: tab.to_string(),
                token,
                png: None,
            });
        };
        let Some(l) = self.panel(tab) else {
            none(&self.sink);
            return;
        };
        // SAFETY: a null argument asks for a new stream.
        let Some(stream) = (unsafe { SHCreateMemStream(None) }) else {
            none(&self.sink);
            return;
        };
        let (sink, name, held) = (Arc::clone(&self.sink), tab.to_string(), stream.clone());
        let h = CapturePreviewCompletedHandler::create(Box::new(move |err| {
            let png = err.ok().and_then(|()| read_stream(&held));
            sink(WebEvent::Still {
                tab: name.clone(),
                token,
                png,
            });
            Ok(())
        }));
        // SAFETY: `webview`, the stream and the handler are live COM objects.
        let r = unsafe {
            l.webview
                .CapturePreview(COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG, &stream, &h)
        };
        if r.is_err() {
            none(&self.sink);
        }
    }

    /// Only a tab's panel takes the keyboard: a bar or a background page never does.
    fn focus(&mut self, tab: &str) {
        if let Some(l) = self.panel(tab) {
            // SAFETY: `controller` is a live COM object.
            let _ = unsafe {
                l.controller
                    .MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC)
            };
        }
    }

    fn close_view(&mut self, slot: &Slot) {
        self.drop_view(slot);
        // A browser no view uses is closed with its last view (a later open makes it again).
        self.drop_idle_envs();
    }

    fn close_all(&mut self) {
        self.cancel_downloads(None);
        self.cancel_serves(None);
        for (_, view) in self.state.views.drain() {
            view.shut();
        }
        self.inbox.borrow_mut().clear();
        self.state.envs.clear();
    }

    fn answer_download(&mut self, id: u64, save: bool) {
        self.finish_download(id, save);
    }

    fn runtime_checked(&mut self, result: Result<String, String>) {
        // The answer is not tied to a kind: it settles every browser that is waiting for it.
        let waiting: Vec<EnvKind> = self
            .state
            .envs
            .iter()
            .filter(|(_, e)| matches!(e, Env::Checking))
            .map(|(k, _)| *k)
            .collect();
        for kind in waiting {
            self.state.envs.remove(&kind);
            let slots = self.slots_of(kind);
            if slots.is_empty() {
                // Everyone left while the question was out.
                continue;
            }
            match &result {
                Err(why) => {
                    for s in slots {
                        self.fail(&s, why, true);
                    }
                }
                Ok(version) => {
                    log::info!("WebView2 runtime {version}");
                    match self.start_env(kind) {
                        Ok(()) => {
                            self.state.envs.insert(kind, Env::Creating);
                        }
                        Err(why) => {
                            for s in slots {
                                self.fail(&s, &why, false);
                            }
                        }
                    }
                }
            }
        }
    }

    fn pump(&mut self) {
        // Files the serve workers have read are handed to the browser first.
        let ready: Vec<_> = self
            .served
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
            .collect();
        for (id, r) in ready {
            self.finish_serve(id, r);
        }
        loop {
            let next = self.inbox.borrow_mut().pop_front();
            let Some(item) = next else { break };
            match item {
                Inbound::BrowserExited(kind) => {
                    // Every page of that browser is gone and the environment with them: close what is
                    // left of it, drop it (the next open makes a fresh one) and tell each view's owner.
                    let slots = self.slots_of(kind);
                    for s in &slots {
                        self.drop_view(s);
                    }
                    self.state.envs.remove(&kind);
                    for s in slots {
                        ViewSink::new(&self.sink, &s).send(|tab| WebEvent::Gone {
                            tab,
                            why: "the browser's process ended".to_string(),
                        });
                    }
                }
                Inbound::Env {
                    kind,
                    result: Ok(env),
                } => {
                    let slots = self.slots_of(kind);
                    if slots.is_empty() {
                        // Everyone left while it was being made.
                        self.state.envs.remove(&kind);
                        continue;
                    }
                    self.state.envs.insert(kind, Env::Ready(env));
                    for s in slots {
                        self.start_controller(&s);
                    }
                }
                Inbound::Env {
                    kind,
                    result: Err(why),
                } => {
                    self.state.envs.remove(&kind);
                    for s in self.slots_of(kind) {
                        self.fail(&s, &why, false);
                    }
                }
                Inbound::Script {
                    slot,
                    generation,
                    result,
                } => self.script_armed(&slot, generation, result),
                Inbound::Controller {
                    slot,
                    generation,
                    result,
                } => match result {
                    Ok(c) => self.adopt(&slot, generation, c),
                    Err(why) => {
                        let mine = self
                            .state
                            .views
                            .get(&slot)
                            .is_some_and(|v| v.generation == generation);
                        if mine {
                            self.fail(&slot, &why, false);
                        }
                    }
                },
            }
        }
    }

    fn controllers(&self) -> usize {
        self.state.views.len()
    }

    fn set_key_filter(&mut self, filter: KeyFilter) {
        *self.filter.borrow_mut() = Some(filter);
    }

    fn cdp(&mut self, tab: &str, id: u64, method: &str, params: &str) {
        let name = tab.to_string();
        let Some(l) = self.panel(tab) else {
            (self.sink)(WebEvent::Cdp {
                tab: name,
                id,
                result: Err("no web view".to_string()),
            });
            return;
        };
        let (sink, held_tab) = (Arc::clone(&self.sink), name.clone());
        let h = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |err, json| {
            let result = match err {
                Ok(()) => Ok(json),
                Err(e) if json.is_empty() => Err(e.message()),
                Err(e) => Err(format!("{} {json}", e.message())),
            };
            sink(WebEvent::Cdp {
                tab: held_tab.clone(),
                id,
                result,
            });
            Ok(())
        }));
        let (m, p) = (wide(method), wide(params));
        // SAFETY: `webview` and the handler are live COM objects; both strings are NUL-terminated and
        // outlive the call.
        let r = unsafe {
            l.webview
                .CallDevToolsProtocolMethod(PCWSTR(m.as_ptr()), PCWSTR(p.as_ptr()), &h)
        };
        if let Err(e) = r {
            (self.sink)(WebEvent::Cdp {
                tab: name,
                id,
                result: Err(e.message()),
            });
        }
    }

    fn post_to_view(&mut self, slot: &Slot, json: &str) {
        let is_mod = self
            .state
            .views
            .get(slot)
            .is_some_and(|v| v.app.as_ref().is_some_and(|a| a.module.is_some()));
        if !is_mod {
            return;
        }
        if let Some(l) = self.live(slot) {
            let j = wide(json);
            // SAFETY: `webview` is a live COM object; `j` is NUL-terminated and outlives the call.
            let _ = unsafe { l.webview.PostWebMessageAsJson(PCWSTR(j.as_ptr())) };
        }
    }

    fn set_zoom(&mut self, tab: &str, factor: f64) {
        if let Some(l) = self.panel(tab) {
            // SAFETY: `controller` is a live COM object.
            let _ = unsafe { l.controller.SetZoomFactor(factor) };
        }
    }

    fn run_script(&mut self, tab: &str, id: u64, js: &str) {
        let Some(l) = self.panel(tab) else {
            (self.sink)(WebEvent::Script {
                id,
                result: Err("no web view".to_string()),
            });
            return;
        };
        let sink = Arc::clone(&self.sink);
        let h = ExecuteScriptCompletedHandler::create(Box::new(move |err, json| {
            sink(WebEvent::Script {
                id,
                result: err.map(|()| json).map_err(|e| e.message()),
            });
            Ok(())
        }));
        let script = wide(js);
        // SAFETY: `webview` and the handler are live; `script` is NUL-terminated and outlives the call.
        let r = unsafe { l.webview.ExecuteScript(PCWSTR(script.as_ptr()), &h) };
        if let Err(e) = r {
            (self.sink)(WebEvent::Script {
                id,
                result: Err(e.message()),
            });
        }
    }

    fn probe_view(&self, slot: &Slot) -> Option<ControllerProbe> {
        let l = self.live(slot)?;
        let mut r = RECT::default();
        let mut pid = 0u32;
        // SAFETY: `controller` and `webview` are live COM objects; the out values are valid.
        let (visible, url, title, profile, private) = unsafe {
            let _ = l.controller.Bounds(&mut r);
            let _ = l.webview.BrowserProcessId(&mut pid);
            let profile = l
                .webview
                .cast::<ICoreWebView2_13>()
                .and_then(|w| w.Profile())
                .map(|p| {
                    (
                        get_string(|s| p.ProfileName(s)),
                        get_bool(|b| p.IsInPrivateModeEnabled(b)),
                    )
                })
                .unwrap_or_default();
            (
                get_bool(|b| l.controller.IsVisible(b)),
                get_string(|p| l.webview.Source(p)),
                get_string(|p| l.webview.DocumentTitle(p)),
                profile.0,
                profile.1,
            )
        };
        Some(ControllerProbe {
            bounds: from_rect(r),
            visible,
            child_windows: self.child_windows(),
            url,
            title,
            browser_pid: pid,
            profile,
            private,
        })
    }
}

/// The extra browser arguments of a browser of `kind`. The web panel's has none. A mini-app's has the WebRTC policy and
/// either a proxy that is not there (no network) or Flare's filtering proxy (`https:` granted); the plugins' sealed
/// one has the sealed mini-apps' arguments, the networked one the filtering proxy and its resolver rules. The proxy's
/// port is fixed for the process, so a browser that was dropped while idle and made again gets the same arguments for
/// its folder. An error when the proxy could not start: a networked browser is then not made at all.
fn env_arguments(kind: EnvKind) -> Result<Option<String>, String> {
    Ok(match kind {
        EnvKind::Web => None,
        EnvKind::AppSealed | EnvKind::Plugin => Some(app::browser_arguments(None)),
        EnvKind::AppNet => Some(app::browser_arguments(Some(netproxy::port()?))),
        EnvKind::PluginNet => Some(modview::net_browser_arguments(netproxy::port()?)),
    })
}

/// Before a mini-app's browser starts: takes out of this process the variables that would replace the
/// arguments, profile and runtime Flare passes (removed on the UI thread just before the loader reads
/// them), and refuses the app when a Windows policy holds such a value (it cannot be removed, and a
/// browser without its walls must not start).
fn seal_against_overrides() -> Result<(), String> {
    for name in app::OVERRIDE_ENV_VARS {
        // SAFETY: called on the UI thread; the loader reads these only when it creates the
        // environment right after, on this same thread.
        unsafe { std::env::remove_var(name) };
    }
    let found: Vec<String> = app::OVERRIDE_POLICY_KEYS
        .iter()
        .filter(|k| policy_key_has_values(k))
        .map(|k| k.to_string())
        .collect();
    app::override_refusal(&found).map_or(Ok(()), Err)
}

/// Does `Software\Policies\Microsoft\Edge\WebView2\<key>` hold a value in HKCU or HKLM?
fn policy_key_has_values(key: &str) -> bool {
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, RegCloseKey, RegOpenKeyExW,
        RegQueryInfoKeyW,
    };
    let path = wide(&format!(r"Software\Policies\Microsoft\Edge\WebView2\{key}"));
    [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE]
        .into_iter()
        .any(|root| {
            let mut h: HKEY = std::ptr::null_mut();
            // SAFETY: `path` is NUL-terminated; `h` is a valid out value, closed below once opened.
            unsafe {
                if RegOpenKeyExW(root, path.as_ptr(), 0, KEY_READ, &mut h) != 0 {
                    return false;
                }
                let mut values = 0u32;
                let rc = RegQueryInfoKeyW(
                    h,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut values,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                );
                RegCloseKey(h);
                rc != 0 || values > 0
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A browser dropped while idle is made again in the same folder: its arguments must be the same both times.
    #[test]
    fn every_browser_kind_gets_the_same_arguments_each_time_it_is_made() {
        for kind in [
            EnvKind::Web,
            EnvKind::AppSealed,
            EnvKind::AppNet,
            EnvKind::Plugin,
            EnvKind::PluginNet,
        ] {
            assert_eq!(env_arguments(kind), env_arguments(kind), "{kind:?}");
        }
    }

    #[test]
    fn the_networked_browsers_go_through_the_filtering_proxy_and_the_web_panel_does_not() {
        let port = netproxy::port().unwrap();
        let proxy =
            format!("--proxy-server=http://127.0.0.1:{port} --proxy-bypass-list=<-loopback>");
        for kind in [EnvKind::AppNet, EnvKind::PluginNet] {
            let a = env_arguments(kind).unwrap().unwrap();
            assert!(a.contains(&proxy), "{kind:?}: {a}");
        }
        for kind in [EnvKind::AppSealed, EnvKind::Plugin] {
            let a = env_arguments(kind).unwrap().unwrap();
            assert!(
                a.contains("--proxy-server=http://127.0.0.1:9 "),
                "{kind:?}: {a}"
            );
        }
        assert_eq!(env_arguments(EnvKind::Web), Ok(None));
    }
}
