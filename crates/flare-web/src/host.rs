//! The WebView2 glue: one controller per tab with an open web panel, parented to Flare's window.
//!
//! Everything runs on the UI thread, which is a COM single-threaded apartment (winit initialises
//! OLE for drag and drop) with the message loop WebView2 needs. Nothing here waits for the browser:
//! the environment and each controller are created asynchronously, their completions arrive through
//! the message loop and are parked in an inbox; the window is told with [`WebEvent::Pump`] and
//! [`WebBackend::pump`] takes them in. A WebView2 callback never touches the host's state, so a
//! callback that fires while a call into the browser is still on the stack cannot re-enter it.
//!
//! No browser exists until the first web panel opens, and the environment goes with the last
//! controller (rule 34 and the anti-goal "panels cost nothing while closed").
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
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use flare_surfaces::web::{Failure, PhysRect, failure_of_kind, host_label, navigation_allowed};
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{
    AcceleratorKeyPressedEventHandler, CallDevToolsProtocolMethodCompletedHandler,
    CapturePreviewCompletedHandler, ContainsFullScreenElementChangedEventHandler,
    CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    DocumentTitleChangedEventHandler, DownloadStartingEventHandler, ExecuteScriptCompletedHandler,
    FocusChangedEventHandler, HistoryChangedEventHandler, NavigationCompletedEventHandler,
    NavigationStartingEventHandler, NewWindowRequestedEventHandler,
    PermissionRequestedEventHandler, ProcessFailedEventHandler, SourceChangedEventHandler,
    WindowCloseRequestedEventHandler, take_pwstr,
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

use crate::backend::{ControllerProbe, KeyFilter, KeyPress, Nav, Sink, WebBackend, WebEvent};

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
}

enum Inbound {
    /// The browser process ended: every view is dead.
    BrowserExited,
    Env(Result<ICoreWebView2Environment, String>),
    Controller {
        tab: String,
        serial: u64,
        result: Result<ICoreWebView2Controller, String>,
    },
}

enum Env {
    None,
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
    serial: u64,
    creating: bool,
    live: Option<Live>,
    url: String,
    bounds: PhysRect,
    visible: bool,
}

/// A download the browser started and is holding until it is told yes or no.
struct Download {
    tab: String,
    args: ICoreWebView2DownloadStartingEventArgs,
    deferral: ICoreWebView2Deferral,
}

type Downloads = Rc<RefCell<HashMap<u64, Download>>>;

struct State {
    env: Env,
    views: HashMap<String, View>,
    serial: u64,
}

pub struct WebHost {
    parent: HWND,
    data_dir: PathBuf,
    sink: Sink,
    runtime: RuntimeCheck,
    virtual_hosts: Vec<(String, PathBuf)>,
    state: State,
    inbox: Rc<RefCell<VecDeque<Inbound>>>,
    filter: Rc<RefCell<Option<KeyFilter>>>,
    downloads: Downloads,
    download_seq: Rc<Cell<u64>>,
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
            state: State {
                env: Env::None,
                views: HashMap::new(),
                serial: 0,
            },
            inbox: Rc::new(RefCell::new(VecDeque::new())),
            filter: Rc::new(RefCell::new(None)),
            downloads: Rc::new(RefCell::new(HashMap::new())),
            download_seq: Rc::new(Cell::new(0)),
        }
    }

    /// Ends the downloads still waiting for a yes (all of them, or the tab's): none is saved.
    fn cancel_downloads(&self, tab: Option<&str>) {
        let ids: Vec<u64> = self
            .downloads
            .borrow()
            .iter()
            .filter(|(_, d)| tab.is_none_or(|t| d.tab == t))
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

    /// The tab's view cannot be made: nothing of it is kept (a later open tries again, so a runtime
    /// installed meanwhile is found), and the window is told why.
    fn fail(&mut self, tab: &str, why: &str, missing_runtime: bool) {
        self.state.views.remove(tab);
        (self.sink)(WebEvent::Failed {
            tab: tab.to_string(),
            why: why.to_string(),
            missing_runtime,
        });
    }

    /// Starts the environment (the first time) and, once it exists, the tab's controller.
    fn ensure(&mut self, tab: &str) {
        match &self.state.env {
            Env::None => {
                let (check, sink) = (Arc::clone(&self.runtime), Arc::clone(&self.sink));
                let job = move || sink(WebEvent::Runtime(check()));
                match std::thread::Builder::new()
                    .name("flare-web-runtime".into())
                    .spawn(job)
                {
                    Ok(_) => self.state.env = Env::Checking,
                    Err(e) => self.fail(
                        tab,
                        &format!("the runtime check could not start: {e}"),
                        false,
                    ),
                }
            }
            Env::Checking | Env::Creating => {}
            Env::Ready(_) => self.start_controller(tab),
        }
    }

    fn start_env(&self) -> Result<(), String> {
        let inbox = Rc::clone(&self.inbox);
        let sink = Arc::clone(&self.sink);
        let handler =
            CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(move |err, env| {
                let result = match err {
                    Ok(()) => env.ok_or_else(|| "the browser gave no environment".to_string()),
                    Err(e) => Err(e.message()),
                };
                inbox.borrow_mut().push_back(Inbound::Env(result));
                sink(WebEvent::Pump);
                Ok(())
            }));
        let folder = wide(&self.data_dir.to_string_lossy());
        // SAFETY: `folder` is a NUL-terminated UTF-16 string that outlives the call; the handler is a
        // live COM object the loader keeps alive until it fires.
        unsafe {
            CreateCoreWebView2EnvironmentWithOptions(
                // sync-ok: WebView2 objects are thread-affine to the UI STA; first open only
                PCWSTR::null(),
                PCWSTR(folder.as_ptr()),
                None::<&ICoreWebView2EnvironmentOptions>,
                &handler,
            )
        }
        .map_err(|e| e.message())
    }

    fn start_controller(&mut self, tab: &str) {
        let Env::Ready(env) = &self.state.env else {
            return;
        };
        let Some(view) = self.state.views.get_mut(tab) else {
            return;
        };
        if view.creating || view.live.is_some() {
            return;
        }
        view.creating = true;
        let (inbox, sink) = (Rc::clone(&self.inbox), Arc::clone(&self.sink));
        let (tab_name, serial) = (tab.to_string(), view.serial);
        let handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
            move |err, controller| {
                let result = match err {
                    Ok(()) => controller.ok_or_else(|| "the browser gave no view".to_string()),
                    Err(e) => Err(e.message()),
                };
                inbox.borrow_mut().push_back(Inbound::Controller {
                    tab: tab_name.clone(),
                    serial,
                    result,
                });
                sink(WebEvent::Pump);
                Ok(())
            },
        ));
        // SAFETY: `parent` is Flare's own live window on this thread; the handler is a live COM object.
        // sync-ok: WebView2 objects are thread-affine to the UI STA; one call per web panel
        let r = unsafe { env.CreateCoreWebView2Controller(self.parent, &handler) };
        if let Err(e) = r {
            let why = e.message();
            self.fail(tab, &why, false);
        }
    }

    /// A controller arrived: set it up and send it where it was asked to go.
    fn adopt(&mut self, tab: &str, serial: u64, controller: ICoreWebView2Controller) {
        let current = self
            .state
            .views
            .get(tab)
            .is_some_and(|v| v.serial == serial);
        if !current {
            // The panel closed (or reopened) while the browser was making this one.
            // SAFETY: closing a controller we own.
            let _ = unsafe { controller.Close() };
            return;
        }
        match self.configure(tab, &controller) {
            Ok(webview) => {
                let Some(view) = self.state.views.get_mut(tab) else {
                    return;
                };
                view.creating = false;
                let url = wide(&view.url);
                // SAFETY: `controller` and `webview` are live COM objects owned here; `url` is
                // NUL-terminated and outlives the call.
                unsafe {
                    let _ = controller.SetBounds(to_rect(view.bounds));
                    let _ = controller.SetIsVisible(view.visible);
                    let _ = webview.Navigate(PCWSTR(url.as_ptr()));
                }
                view.live = Some(Live {
                    controller,
                    webview,
                });
                (self.sink)(WebEvent::Ready {
                    tab: tab.to_string(),
                });
            }
            Err(why) => {
                // SAFETY: closing a controller we own.
                let _ = unsafe { controller.Close() };
                self.fail(tab, &why, false);
            }
        }
    }

    /// Settings and every handler of one view. The handlers hold the sink and the tab's name and
    /// nothing of the host.
    fn configure(
        &self,
        tab: &str,
        controller: &ICoreWebView2Controller,
    ) -> Result<ICoreWebView2, String> {
        // SAFETY: `controller` is a live COM object.
        let webview = unsafe { controller.CoreWebView2() }
            .map_err(|x| format!("the page: {}", x.message()))?;
        // SAFETY: `webview` is live; settings are plain flags.
        unsafe {
            if let Ok(s) = webview.Settings() {
                // A page cannot talk to the app.
                let _ = s.SetIsWebMessageEnabled(false);
                let _ = s.SetAreHostObjectsAllowed(false);
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
        let name = |s: &str| s.to_string();

        // ---- navigation ----
        {
            let (sink, tab) = (Arc::clone(&self.sink), name(tab));
            let h = NavigationStartingEventHandler::create(Box::new(move |_w, args| {
                let Some(args) = args else { return Ok(()) };
                // SAFETY: `args` is the live event argument of this callback.
                let uri = get_string(|p| unsafe { args.Uri(p) });
                if navigation_allowed(&uri) {
                    let mut nav = 0u64;
                    // SAFETY: as above.
                    let _ = unsafe { args.NavigationId(&mut nav) };
                    sink(WebEvent::Loading {
                        tab: tab.clone(),
                        url: uri,
                        nav,
                    });
                } else {
                    // SAFETY: as above.
                    let _ = unsafe { args.SetCancel(true) };
                    sink(WebEvent::Refused {
                        tab: tab.clone(),
                        url: uri,
                    });
                }
                Ok(())
            }));
            // SAFETY: `webview` and the handler are live COM objects; `token` is a valid out value.
            let _ = unsafe { webview.add_NavigationStarting(&h, &mut token) };
        }
        {
            let (sink, tab) = (Arc::clone(&self.sink), name(tab));
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
                sink(WebEvent::Loaded {
                    tab: tab.clone(),
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
            let (sink, tab) = (Arc::clone(&self.sink), name(tab));
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
                    sink(WebEvent::Url {
                        tab: tab.clone(),
                        url,
                    });
                } else {
                    sink(WebEvent::SameDocument {
                        tab: tab.clone(),
                        url,
                    });
                }
                Ok(())
            }));
            // SAFETY: as above.
            let _ = unsafe { webview.add_SourceChanged(&h, &mut token) };
        }
        {
            let (sink, tab) = (Arc::clone(&self.sink), name(tab));
            let h = HistoryChangedEventHandler::create(Box::new(move |w, _args| {
                let Some(w) = w else { return Ok(()) };
                // SAFETY: `w` is the live sender of this callback.
                let (can_back, can_forward) = unsafe {
                    (
                        get_bool(|b| w.CanGoBack(b)),
                        get_bool(|b| w.CanGoForward(b)),
                    )
                };
                sink(WebEvent::History {
                    tab: tab.clone(),
                    can_back,
                    can_forward,
                });
                Ok(())
            }));
            // SAFETY: as above.
            let _ = unsafe { webview.add_HistoryChanged(&h, &mut token) };
        }
        {
            let (sink, tab) = (Arc::clone(&self.sink), name(tab));
            let h = DocumentTitleChangedEventHandler::create(Box::new(move |w, _args| {
                let Some(w) = w else { return Ok(()) };
                // SAFETY: `w` is the live sender of this callback.
                let title = get_string(|p| unsafe { w.DocumentTitle(p) });
                sink(WebEvent::Title {
                    tab: tab.clone(),
                    title,
                });
                Ok(())
            }));
            // SAFETY: as above.
            let _ = unsafe { webview.add_DocumentTitleChanged(&h, &mut token) };
        }
        // A page cannot open a window: the address goes to this panel (if it may open at all).
        {
            let (sink, tab) = (Arc::clone(&self.sink), name(tab));
            let h = NewWindowRequestedEventHandler::create(Box::new(move |w, args| {
                let (Some(w), Some(args)) = (w, args) else {
                    return Ok(());
                };
                // SAFETY: both are the live arguments of this callback.
                unsafe {
                    let uri = get_string(|p| args.Uri(p));
                    let _ = args.SetHandled(true);
                    if navigation_allowed(&uri) {
                        let u = wide(&uri);
                        let _ = w.Navigate(PCWSTR(u.as_ptr()));
                    } else {
                        sink(WebEvent::Refused {
                            tab: tab.clone(),
                            url: uri,
                        });
                    }
                }
                Ok(())
            }));
            // SAFETY: as above.
            let _ = unsafe { webview.add_NewWindowRequested(&h, &mut token) };
        }
        // Every permission is denied (camera, microphone, location, notifications, clipboard ...).
        {
            let h = PermissionRequestedEventHandler::create(Box::new(move |_w, args| {
                if let Some(args) = args {
                    // SAFETY: `args` is the live argument of this callback.
                    let _ = unsafe { args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY) };
                }
                Ok(())
            }));
            // SAFETY: as above.
            let _ = unsafe { webview.add_PermissionRequested(&h, &mut token) };
        }
        {
            let (sink, tab, inbox) = (Arc::clone(&self.sink), name(tab), Rc::clone(&self.inbox));
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
                    Failure::Page => sink(WebEvent::Gone {
                        tab: tab.clone(),
                        why: "the page's process ended".to_string(),
                    }),
                    Failure::Browser => {
                        inbox.borrow_mut().push_back(Inbound::BrowserExited);
                        sink(WebEvent::Pump);
                    }
                }
                Ok(())
            }));
            // SAFETY: as above.
            let _ = unsafe { webview.add_ProcessFailed(&h, &mut token) };
        }
        // `window.close()` and a page asking for full screen of the whole window do nothing here.
        {
            let h = WindowCloseRequestedEventHandler::create(Box::new(move |_w, _a| Ok(())));
            // SAFETY: as above.
            let _ = unsafe { webview.add_WindowCloseRequested(&h, &mut token) };
            let h = ContainsFullScreenElementChangedEventHandler::create(Box::new(
                move |_w, _a| Ok(()),
            ));
            // SAFETY: as above.
            let _ = unsafe { webview.add_ContainsFullScreenElementChanged(&h, &mut token) };
        }

        // A download is held until the person has said yes (S6): the browser would otherwise save it
        // into Downloads at once. A runtime too old to hold one back cannot show a web panel at all.
        {
            let w4 = webview.cast::<ICoreWebView2_4>().map_err(|_| {
                "this WebView2 Runtime cannot hold a download for a question".to_string()
            })?;
            let (sink, tab) = (Arc::clone(&self.sink), name(tab));
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
                        tab: tab.clone(),
                        args: args.clone(),
                        deferral,
                    },
                );
                sink(WebEvent::Download {
                    tab: tab.clone(),
                    id,
                    file,
                    host: host_label(&uri),
                });
                Ok(())
            }));
            // SAFETY: `w4` and the handler are live COM objects; `token` is a valid out value.
            if unsafe { w4.add_DownloadStarting(&h, &mut token) }.is_err() {
                return Err("the download question could not be registered".to_string());
            }
        }

        // ---- keys and focus, on the controller ----
        {
            let (sink, tab, filter) = (Arc::clone(&self.sink), name(tab), Rc::clone(&self.filter));
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
                    sink(WebEvent::Key {
                        tab: tab.clone(),
                        key,
                    });
                }
                Ok(())
            }));
            // SAFETY: `controller` and the handler are live COM objects.
            let _ = unsafe { controller.add_AcceleratorKeyPressed(&h, &mut token) };
        }
        for has in [true, false] {
            let (sink, tab) = (Arc::clone(&self.sink), name(tab));
            let h = FocusChangedEventHandler::create(Box::new(move |_c, _a| {
                sink(WebEvent::Focus {
                    tab: tab.clone(),
                    has,
                });
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

    fn live(&self, tab: &str) -> Option<&Live> {
        self.state.views.get(tab).and_then(|v| v.live.as_ref())
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

impl Drop for WebHost {
    fn drop(&mut self) {
        // A download still waiting for a yes is never saved by the host going away.
        self.cancel_downloads(None);
    }
}

impl WebBackend for WebHost {
    fn open(&mut self, tab: &str, url: &str, bounds: PhysRect, visible: bool) {
        if let Some(v) = self.state.views.get_mut(tab) {
            v.url = url.to_string();
            v.bounds = bounds;
            v.visible = visible;
            if v.live.is_some() {
                self.set_bounds(tab, bounds);
                self.set_visible(tab, visible);
                self.navigate(tab, url);
            }
            return;
        }
        self.state.serial += 1;
        let serial = self.state.serial;
        self.state.views.insert(
            tab.to_string(),
            View {
                serial,
                creating: false,
                live: None,
                url: url.to_string(),
                bounds,
                visible,
            },
        );
        self.ensure(tab);
    }

    fn navigate(&mut self, tab: &str, url: &str) {
        if let Some(v) = self.state.views.get_mut(tab) {
            v.url = url.to_string();
        }
        if let Some(l) = self.live(tab) {
            let u = wide(url);
            // SAFETY: `webview` is live; `u` is NUL-terminated and outlives the call.
            let _ = unsafe { l.webview.Navigate(PCWSTR(u.as_ptr())) };
        }
    }

    fn go(&mut self, tab: &str, nav: Nav) {
        if let Some(l) = self.live(tab) {
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

    fn set_bounds(&mut self, tab: &str, bounds: PhysRect) {
        if let Some(v) = self.state.views.get_mut(tab) {
            v.bounds = bounds;
        }
        if let Some(l) = self.live(tab) {
            // SAFETY: `controller` is a live COM object.
            let _ = unsafe { l.controller.SetBounds(to_rect(bounds)) };
        }
    }

    fn set_visible(&mut self, tab: &str, visible: bool) {
        if let Some(v) = self.state.views.get_mut(tab) {
            v.visible = visible;
        }
        if let Some(l) = self.live(tab) {
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
        let Some(l) = self.live(tab) else {
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

    fn focus(&mut self, tab: &str) {
        if let Some(l) = self.live(tab) {
            // SAFETY: `controller` is a live COM object.
            let _ = unsafe {
                l.controller
                    .MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC)
            };
        }
    }

    fn close(&mut self, tab: &str) {
        self.cancel_downloads(Some(tab));
        let Some(view) = self.state.views.remove(tab) else {
            return;
        };
        if let Some(l) = view.live {
            // SAFETY: closing a controller we own; the page and its window go with it.
            let _ = unsafe { l.controller.Close() };
        }
        if self.state.views.is_empty() {
            // No web panel is open: the browser environment goes too (a later one makes it again).
            self.state.env = Env::None;
        }
    }

    fn close_all(&mut self) {
        self.cancel_downloads(None);
        for (_, view) in self.state.views.drain() {
            if let Some(l) = view.live {
                // SAFETY: closing a controller we own; the page and its window go with it.
                let _ = unsafe { l.controller.Close() };
            }
        }
        self.inbox.borrow_mut().clear();
        self.state.env = Env::None;
    }

    fn answer_download(&mut self, id: u64, save: bool) {
        self.finish_download(id, save);
    }

    fn runtime_checked(&mut self, result: Result<String, String>) {
        if !matches!(self.state.env, Env::Checking) {
            return;
        }
        let tabs: Vec<String> = self.state.views.keys().cloned().collect();
        self.state.env = Env::None;
        if tabs.is_empty() {
            // Everyone left while the question was out.
            return;
        }
        match result {
            Err(why) => {
                for t in tabs {
                    self.fail(&t, &why, true);
                }
            }
            Ok(version) => {
                log::info!("WebView2 runtime {version}");
                match self.start_env() {
                    Ok(()) => self.state.env = Env::Creating,
                    Err(why) => {
                        for t in tabs {
                            self.fail(&t, &why, false);
                        }
                    }
                }
            }
        }
    }

    fn pump(&mut self) {
        loop {
            let next = self.inbox.borrow_mut().pop_front();
            let Some(item) = next else { break };
            match item {
                Inbound::BrowserExited => {
                    // Every page is gone and the environment with them: close what is left, drop the
                    // environment (the next open makes a fresh one) and tell each panel.
                    self.cancel_downloads(None);
                    let tabs: Vec<String> = self.state.views.keys().cloned().collect();
                    for (_, view) in self.state.views.drain() {
                        if let Some(l) = view.live {
                            // SAFETY: closing a controller we own.
                            let _ = unsafe { l.controller.Close() };
                        }
                    }
                    self.state.env = Env::None;
                    for t in tabs {
                        (self.sink)(WebEvent::Gone {
                            tab: t,
                            why: "the browser's process ended".to_string(),
                        });
                    }
                }
                Inbound::Env(Ok(env)) => {
                    if self.state.views.is_empty() {
                        // Everyone left while it was being made.
                        self.state.env = Env::None;
                        continue;
                    }
                    self.state.env = Env::Ready(env);
                    let tabs: Vec<String> = self.state.views.keys().cloned().collect();
                    for t in tabs {
                        self.start_controller(&t);
                    }
                }
                Inbound::Env(Err(why)) => {
                    self.state.env = Env::None;
                    let tabs: Vec<String> = self.state.views.keys().cloned().collect();
                    for t in tabs {
                        self.fail(&t, &why, false);
                    }
                }
                Inbound::Controller {
                    tab,
                    serial,
                    result,
                } => match result {
                    Ok(c) => self.adopt(&tab, serial, c),
                    Err(why) => {
                        let mine = self
                            .state
                            .views
                            .get(&tab)
                            .is_some_and(|v| v.serial == serial);
                        if mine {
                            self.fail(&tab, &why, false);
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
        let Some(l) = self.live(tab) else {
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

    fn set_zoom(&mut self, tab: &str, factor: f64) {
        if let Some(l) = self.live(tab) {
            // SAFETY: `controller` is a live COM object.
            let _ = unsafe { l.controller.SetZoomFactor(factor) };
        }
    }

    fn run_script(&mut self, tab: &str, id: u64, js: &str) {
        let Some(l) = self.live(tab) else {
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

    fn probe(&self, tab: &str) -> Option<ControllerProbe> {
        let l = self.live(tab)?;
        let mut r = RECT::default();
        let mut pid = 0u32;
        // SAFETY: `controller` and `webview` are live COM objects; the out values are valid.
        let (visible, url, title) = unsafe {
            let _ = l.controller.Bounds(&mut r);
            let _ = l.webview.BrowserProcessId(&mut pid);
            (
                get_bool(|b| l.controller.IsVisible(b)),
                get_string(|p| l.webview.Source(p)),
                get_string(|p| l.webview.DocumentTitle(p)),
            )
        };
        Some(ControllerProbe {
            bounds: from_rect(r),
            visible,
            child_windows: self.child_windows(),
            url,
            title,
            browser_pid: pid,
        })
    }
}
