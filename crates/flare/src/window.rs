//! The Slint window: wires the chrome, the grid host, the renderer notifier, the winit filter,
//! the saved position and size, the hand-off server and the quit to [`AppCtl`].
//!
//! Source: M2 Task 27 (the winit filter, `shutdown`), Prism Terminal's `index.ts`
//! (`createWindow`, `win.on('close')`, `second-instance`) and `windowState.ts`.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flare_render::glyph::GlyphSource as _;
use flare_render::raster::{DEFAULT_FAMILIES, Rasterizer};
use flare_render::stats::{FrameTrace, TracePhase};
use flare_settings::kv::{FileBackend, PrefStore};
use flare_settings::look::LookStore;
use flare_tabs::tabs_store::TabsStore;
use flare_term::caret_hold::CaretPos;
use flare_term::handle::TermHandle;
use flare_term::keys::KeyEvent;
use flare_term::path_open::FsStat;
use flare_term::pty::PortableSpawner;
use flare_term::session::RealShells;
use flare_win::argv::{folders_from_argv, own_files};
use flare_win::clipboard_port::CLIPBOARD_WRITE_MAX;
use flare_win::dblclick::system_double_click;
use flare_win::edge_watch::{EdgeNear, EdgeWatch, FAR, PointF, RectF, real_cursor_phys};
#[cfg(not(target_os = "macos"))]
use flare_win::monitors::work_areas;
use flare_win::pick::{ParentWindow, PickOptions, pick_folder};
use flare_win::session_end::SessionEndWatch;
use flare_win::single_instance::{Handoff, InstanceGuard, InstanceNames, PipeServer, serve};
use flare_win::window_state::{Area, MIN_WINDOW, Observed, WindowState, WindowStateStore};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::winit_030::winit::dpi::{PhysicalPosition, PhysicalSize};
use slint::winit_030::winit::event::{
    ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent,
};
use slint::winit_030::winit::keyboard::ModifiersState;
use slint::winit_030::winit::window::WindowLevel;
use slint::winit_030::{EventResult, WinitWindowAccessor};
use slint::{CloseRequestResponse, ComponentHandle, GraphicsAPI, RenderingState};
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowLongPtrW, SetWindowLongPtrW, WS_EX_APPWINDOW, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW,
};

use crate::appctl::{AppCtl, Backends, CloseDecision, Deps, KeyOutcome, Post, TermPorts, UiMsg};
use crate::backend::BackendOutcome;
use crate::config::{Args, RendererChoice};
use crate::e2e;
use crate::font_ctl::FontCtl;
use crate::host::{GridHost, GridPainter};
use crate::ime::{CaretFollower, ime_area};
use crate::keymap::{key_event_from_parts, reserved_for_slint};
use crate::live::{LiveSource, WheelAccum, WheelGate, cell_at, wheel_is_terminals};
use crate::look_ctl::LookCtl;
use crate::material_ctl::{SlintWake, WindowMaterial};
use crate::memory::process_memory;
use crate::startup::{Profile, RaiseStep, raise_plan};
use crate::tab_host::TabHost;
use crate::{AppWindow, HelpBridge};
use flare_term::mac_keys::PtyKey;
use flare_win::os_build::os_version;

/// How long the state files wait before they are written (Prism Terminal's debounce).
const SAVE_DELAY: Duration = Duration::from_millis(400);

/// The first instance's claim on the profile: the lock, kept for the whole run, and its
/// hand-off server.
pub struct Instance {
    pub profile: Profile,
    /// The hand-off server, started by `main` right after the lock, BEFORE the backend and the
    /// window: a second launch that starts while this one is still creating its window must
    /// find the pipe within its wait. Declared before `_guard`, so it stops first.
    /// Shared with the controller, which stops it at the start of the end of life: a hand-off
    /// that lands while the shells are being killed would be written, acknowledged and dropped.
    pub server: Rc<RefCell<Option<PipeServer>>>,
    /// Where the server's hand-offs wait until the controller exists.
    pub sink: HandoffSink,
    /// Held only so the lock lasts as long as the run.
    pub _guard: Option<InstanceGuard>,
}

impl Instance {
    /// Starts the hand-off server for `names`; a failure is logged and the run goes on without it.
    pub fn start(profile: Profile, names: InstanceNames, guard: Option<InstanceGuard>) -> Self {
        let sink = HandoffSink::default();
        let server = match serve(&names, {
            let sink = sink.clone();
            move |h| sink.deliver(h)
        }) {
            Ok(s) => Some(s),
            Err(e) => {
                log::warn!("the hand-off server did not start: {e}");
                None
            }
        };
        Self {
            profile,
            server: Rc::new(RefCell::new(server)),
            sink,
            _guard: guard,
        }
    }
}

/// Hand-offs that arrive before the controller exists are kept here; once attached, each goes
/// straight to `post` (in arrival order: both paths post under the lock).
#[derive(Clone, Default)]
pub struct HandoffSink(Arc<Mutex<SinkState>>);

#[derive(Default)]
struct SinkState {
    early: Vec<Handoff>,
    post: Option<Post>,
}

impl HandoffSink {
    fn lock(&self) -> std::sync::MutexGuard<'_, SinkState> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The pipe thread's callback.
    pub fn deliver(&self, h: Handoff) {
        let mut s = self.lock();
        match &s.post {
            Some(post) => post(UiMsg::Handoff(h)),
            None => s.early.push(h),
        }
    }

    /// The controller is ready: what came early is posted now, the rest as it arrives.
    pub fn attach(&self, post: Post) {
        let mut s = self.lock();
        for h in std::mem::take(&mut s.early) {
            post(UiMsg::Handoff(h));
        }
        s.post = Some(post);
    }
}

/// Where the window opens, made safe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InitialWindow {
    /// Physical position, `None` for "let the system place it".
    pub pos: Option<(i32, i32)>,
    /// Logical size.
    pub logical_size: (u32, u32),
    pub maximise: bool,
}

/// The saved state made safe: never under the minimum, and a position only when some monitor
/// would still show the window (`parse_window_state` did the same work already; this is the
/// belt for a state that did not come from the file).
pub fn initial_window(state: &WindowState, areas: &[Area]) -> InitialWindow {
    let w = state.width.max(MIN_WINDOW.0);
    let h = state.height.max(MIN_WINDOW.1);
    let scale = if state.scale.is_finite() && state.scale > 0.0 {
        state.scale
    } else {
        1.0
    };
    let pos = match (state.x, state.y) {
        (Some(x), Some(y)) => {
            let pw = (f64::from(w) * scale).round() as i32;
            let ph = (f64::from(h) * scale).round() as i32;
            let shown = areas.is_empty()
                || areas.iter().any(|a| {
                    x < a.x + a.width && x + pw > a.x && y < a.y + a.height && y + ph > a.y
                });
            shown.then_some((x, y))
        }
        _ => None,
    };
    InitialWindow {
        pos,
        logical_size: (w as u32, h as u32),
        maximise: state.maximised,
    }
}

/// The extended style of an `--e2e` window: it can never take the focus or the foreground, and
/// it stays off the taskbar.
#[cfg(windows)]
pub fn e2e_ex_style(current: u32) -> u32 {
    (current | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW) & !WS_EX_APPWINDOW
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterAction {
    PreventDefault,
    Propagate,
}

/// What the winit filter does with an `AppCtl` key outcome, and whether the pty gets the key.
pub fn key_outcome_to_filter(o: KeyOutcome) -> (FilterAction, bool) {
    match o {
        KeyOutcome::Consumed => (FilterAction::PreventDefault, false),
        KeyOutcome::Pty => (FilterAction::PreventDefault, true),
        KeyOutcome::Slint => (FilterAction::Propagate, false),
    }
}

fn to_event_result(a: FilterAction) -> EventResult {
    match a {
        FilterAction::PreventDefault => EventResult::PreventDefault,
        FilterAction::Propagate => EventResult::Propagate,
    }
}

/// The overlay text: renderer, grid painter, frame statistics and memory.
fn overlay_text(
    effective: RendererChoice,
    fallback: Option<&str>,
    painter: GridPainter,
    frame: &str,
    memory: &str,
) -> String {
    let renderer = match (effective, fallback) {
        (RendererChoice::Gpu, _) => "GPU (femtovg-wgpu)".to_string(),
        (RendererChoice::Software, Some(reason)) => format!("software (fallback: {reason})"),
        (RendererChoice::Software, None) => "software".to_string(),
    };
    let grid = match painter {
        GridPainter::Gpu => "wgpu",
        GridPainter::Cpu => "cpu",
    };
    format!("renderer: {renderer}\ngrid: {grid}\nframe: {frame}\nmemory: {memory}")
}

/// Fires once: the first call to `fire` answers true, every later one false.
#[derive(Debug, Default)]
struct Once(bool);

impl Once {
    fn fire(&mut self) -> bool {
        !std::mem::replace(&mut self.0, true)
    }
}

/// Without a rendering notifier, a repaint is due when the grid is dirty.
fn repaint_due(dirty: bool) -> bool {
    dirty
}

/// How often the software path looks for a dirty grid (one display frame at 60 Hz).
const FRAME_POLL: Duration = Duration::from_millis(16);

/// The software path, a wake-up from a shell just landed: paint now when the grid is dirty and the
/// last paint is a whole frame ago; else leave it to the poll timer, which comes within one frame.
/// So output never waits for a wake AND then for the timer (M10 Task 67, step 3).
fn paint_on_wake(since_last_paint: Duration, dirty: bool) -> bool {
    dirty && since_last_paint >= FRAME_POLL
}

/// At most one `HostWake` is queued on the event loop however fast the output arrives: the shell
/// threads call `try_queue`, the UI thread calls `delivered` as it takes the message.
#[derive(Default)]
struct WakeGate(AtomicBool);

impl WakeGate {
    /// True when the caller must queue a wake-up; false when one is already on its way.
    fn try_queue(&self) -> bool {
        !self.0.swap(true, Ordering::SeqCst)
    }

    /// The queued wake-up is being handled: the next output may queue a new one.
    fn delivered(&self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

thread_local! {
    /// The software path's paint-on-wake hook (UI thread only); empty on the GPU path, where the
    /// notifier paints in `BeforeRendering` after the `request_redraw` a wake-up makes.
    static SOFTWARE_PAINT: RefCell<Option<Rc<dyn Fn()>>> = const { RefCell::new(None) };
}

type CtlRc = Rc<RefCell<AppCtl>>;
/// The shell the grid shows now: kept by the `set_grid_source` dependency, read by the filter.
pub(crate) type FrontCell = Rc<RefCell<Option<Arc<TermHandle>>>>;

thread_local! {
    /// The controller the worker threads' messages land on. Filled at startup, emptied at exit.
    static APP_CTL: RefCell<Option<(CtlRc, slint::Weak<AppWindow>)>> = const { RefCell::new(None) };
}

/// The production `post`: a worker's message hops onto the event loop and lands on `AppCtl`.
/// Wake-ups are coalesced: at most one is queued however fast the output arrives.
fn make_post() -> Post {
    let gate = Arc::new(WakeGate::default());
    Arc::new(move |msg: UiMsg| {
        let is_wake = matches!(msg, UiMsg::HostWake);
        if is_wake && !gate.try_queue() {
            return;
        }
        let gate = Arc::clone(&gate);
        let queued = slint::invoke_from_event_loop(move || {
            if is_wake {
                gate.delivered();
            }
            deliver_on_ui(msg);
            if is_wake {
                // The software path has no notifier: paint what the wake-up changed.
                let hook = SOFTWARE_PAINT.with(|h| h.borrow().clone());
                if let Some(paint) = hook {
                    paint();
                }
            }
        });
        if queued.is_err() {
            log::debug!("post: event loop closed");
        }
    })
}

/// On the UI thread: hands `msg` to the controller. A message after the window closed is
/// dropped; one that finds the controller busy (a re-entrant moment) goes to the back of the
/// queue instead of being lost (a lost `RestoreDone` would leave placeholders forever).
fn deliver_on_ui(msg: UiMsg) {
    let target = APP_CTL.with(|cell| cell.borrow().clone());
    let Some((ctl, weak)) = target else { return };
    if weak.upgrade().is_none() {
        return;
    }
    match ctl.try_borrow_mut() {
        Ok(mut c) => c.on_msg(msg),
        Err(_) => {
            if slint::invoke_from_event_loop(move || deliver_on_ui(msg)).is_err() {
                log::debug!("post: event loop closed");
            }
        }
    }
}

/// The window's HWND, when it has one.
pub(crate) fn hwnd_of(app: &AppWindow) -> Option<isize> {
    app.window()
        .with_winit_window(|w| match w.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
            _ => None,
        })
        .flatten()
}

/// The window's native handle for the window-level calls (park, material, chrome): the HWND on
/// Windows, the content `NSView` on macOS (`flare_win::glass`, `chrome` and `window_park` take
/// that). Everything that needs a real HWND keeps `hwnd_of`.
pub(crate) fn native_view_of(app: &AppWindow) -> Option<isize> {
    app.window()
        .with_winit_window(|w| match w.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
            RawWindowHandle::AppKit(h) => Some(h.ns_view.as_ptr() as isize),
            _ => None,
        })
        .flatten()
}

/// Where the window is put before its first show: the parking spot under `--e2e`, else nowhere
/// in particular (the saved or default place applies).
pub fn e2e_first_position(e2e: bool) -> Option<(i32, i32)> {
    e2e.then(flare_win::window_park::parked_origin)
}

/// Gives an `--e2e` window the no-activate style, so it can never take the foreground.
#[cfg(windows)]
fn apply_e2e_style_to(hwnd: isize) {
    let hwnd = hwnd as *mut std::ffi::c_void;
    // SAFETY: `hwnd` is this process's own live window on the UI thread; reading and writing its
    // extended style touches no memory of ours.
    unsafe {
        let current = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, e2e_ex_style(current) as isize);
    }
}

/// macOS: no window handle style to set (the Mac E2E is an accessory app, port plan "Ongoing").
#[cfg(not(windows))]
fn apply_e2e_style_to(_hwnd: isize) {}

/// How often the `--e2e` window looks for its handle, and how many looks it makes (about 10 s).
const E2E_HANDLE_POLL_MS: u64 = 10;
const E2E_HANDLE_POLLS: u32 = 1000;

/// What one look for the `--e2e` window's handle decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandlePoll {
    /// The window exists: style and park it.
    Found(isize),
    /// Not yet: look again.
    Wait,
    /// Looked `max` times in vain: say so and stop.
    GiveUp,
}

/// The debounced stores a run keeps: tabs, window, prefs and panels.
type Stores = (
    Arc<TabsStore>,
    Arc<WindowStateStore>,
    PrefStore,
    Arc<flare_surfaces::store::PanelStore>,
);

/// Every debounced store written now: the logoff closure and the end of `run`.
fn flush_stores(
    tabs: &TabsStore,
    window: &WindowStateStore,
    prefs: &PrefStore,
    panels: &flare_surfaces::store::PanelStore,
) {
    window.flush();
    tabs.flush();
    prefs.flush();
    panels.flush();
}

/// Pure: the decision of look number `polls` (counted from 0) given what `hwnd_of` answered.
pub fn poll_handle(polls: u32, max: u32, hwnd: Option<isize>) -> HandlePoll {
    match hwnd {
        Some(h) => HandlePoll::Found(h),
        None if polls + 1 >= max => HandlePoll::GiveUp,
        None => HandlePoll::Wait,
    }
}

/// Styles and parks the `--e2e` window as soon as it has a handle. The winit backend makes the
/// native window only once the event loop runs, so right after `show()` there is no handle yet:
/// a short timer looks until it exists. Keep the returned timer alive for the run.
fn park_e2e_window_when_ready(app: &AppWindow) -> Rc<slint::Timer> {
    let timer = Rc::new(slint::Timer::default());
    let weak_timer = Rc::downgrade(&timer);
    let weak = app.as_weak();
    let mut polls = 0u32;
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(E2E_HANDLE_POLL_MS),
        move || {
            let Some(app) = weak.upgrade() else { return };
            let decision = poll_handle(polls, E2E_HANDLE_POLLS, native_view_of(&app));
            polls += 1;
            match decision {
                HandlePoll::Wait => return,
                HandlePoll::Found(h) => {
                    apply_e2e_style_to(h);
                    flare_win::window_park::park(h);
                    log::info!("e2e: the window is styled and parked");
                }
                HandlePoll::GiveUp => {
                    log::warn!("e2e: the window never got a handle, it is not parked");
                }
            }
            if let Some(t) = weak_timer.upgrade() {
                t.stop();
            }
        },
    );
    timer
}

/// The window material (Task 25): the DWM backdrop and edge. It needs the handle of the window.
/// Returns false when there is no handle yet (nothing is attached then).
fn attach_window_material(
    app: &AppWindow,
    material: &Rc<RefCell<Option<WindowMaterial>>>,
    look: &Rc<RefCell<LookCtl>>,
    e2e: bool,
) -> bool {
    let now = Instant::now();
    let Some(mut m) = WindowMaterial::new(app, os_version(), e2e) else {
        return false;
    };
    // The handle exists: the frameless window gets its resize edges and corners (winit gives it no
    // non-client border, so Windows alone answers every point with the client). macOS: AppKit keeps
    // its own edges; the traffic lights move into the top row instead.
    if let Some(view) = native_view_of(app) {
        flare_win::resize_edges::install(view);
        #[cfg(target_os = "macos")]
        if !flare_win::chrome::install(view) {
            log::warn!("the traffic lights could not be placed in the top row");
        }
    }
    // No polling: the material arms a one-shot wake only while the border is due.
    m.set_wake(Box::new(SlintWake::new(Rc::downgrade(material))), now);
    m.on_window_state(
        app.window().is_maximized(),
        app.window().is_fullscreen(),
        now,
    );
    // A saved see-through level becomes glass only now: the backdrop first, then the ground.
    if let Ok(mut l) = look.try_borrow_mut() {
        l.attach_material(m);
    }
    true
}

/// Attaches the window material as soon as the window has a handle (looking as often as the
/// `--e2e` park timer does). Keep the returned timer alive for the run.
fn attach_material_when_ready(
    app: &AppWindow,
    material: &Rc<RefCell<Option<WindowMaterial>>>,
    look: &Rc<RefCell<LookCtl>>,
    e2e: bool,
) -> Rc<slint::Timer> {
    let timer = Rc::new(slint::Timer::default());
    let weak_timer = Rc::downgrade(&timer);
    let weak = app.as_weak();
    let (material, look) = (material.clone(), look.clone());
    let mut polls = 0u32;
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(E2E_HANDLE_POLL_MS),
        move || {
            let Some(app) = weak.upgrade() else { return };
            let decision = poll_handle(polls, E2E_HANDLE_POLLS, native_view_of(&app));
            polls += 1;
            match decision {
                HandlePoll::Wait => return,
                HandlePoll::Found(_) => {
                    attach_window_material(&app, &material, &look, e2e);
                }
                HandlePoll::GiveUp => {
                    log::warn!("the window never got a handle, so it has no glass");
                }
            }
            if let Some(t) = weak_timer.upgrade() {
                t.stop();
            }
        },
    );
    timer
}

/// Runs `f` with the window's handle as soon as it has one (looking as often as the `--e2e` park
/// timer does). Right after `show()` there is none: the winit backend makes the native window only
/// once the event loop runs, so a step that asked then got nothing and was skipped for good (the
/// taskbar badge never showed, 2026-10-26). Keep the returned timer alive for the run.
fn when_handle_ready(
    app: &AppWindow,
    what: &'static str,
    f: impl FnOnce(&AppWindow, isize) + 'static,
) -> Rc<slint::Timer> {
    let timer = Rc::new(slint::Timer::default());
    let weak_timer = Rc::downgrade(&timer);
    let weak = app.as_weak();
    let mut f = Some(f);
    let mut polls = 0u32;
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(E2E_HANDLE_POLL_MS),
        move || {
            let Some(app) = weak.upgrade() else { return };
            let decision = poll_handle(polls, E2E_HANDLE_POLLS, hwnd_of(&app));
            polls += 1;
            match decision {
                HandlePoll::Wait => return,
                HandlePoll::Found(h) => {
                    if let Some(f) = f.take() {
                        f(&app, h);
                    }
                }
                HandlePoll::GiveUp => {
                    log::warn!("the window never got a handle, so {what} is not set up");
                }
            }
            if let Some(t) = weak_timer.upgrade() {
                t.stop();
            }
        },
    );
    timer
}
/// Brings the window forward the way `raise_plan` says (under `--e2e` it only shows).
fn raise_window(app: &AppWindow, e2e: bool) {
    let minimised = app.window().is_minimized();
    for step in raise_plan(e2e, minimised) {
        app.window().with_winit_window(|w| match step {
            RaiseStep::ShowInactive | RaiseStep::Show => w.set_visible(true),
            RaiseStep::Restore => w.set_minimized(false),
            RaiseStep::TopmostOn => w.set_window_level(WindowLevel::AlwaysOnTop),
            RaiseStep::Focus => w.focus_window(),
            RaiseStep::TopmostOff => w.set_window_level(WindowLevel::Normal),
        });
    }
}

/// What the window says about itself, for the saved bounds.
fn observe(window: &slint::Window) -> Observed {
    let size = window.size();
    let pos = window.position();
    Observed {
        width_phys: size.width as i32,
        height_phys: size.height as i32,
        x: pos.x,
        y: pos.y,
        scale: f64::from(window.scale_factor()),
        maximised: window.is_maximized(),
        minimised: window.is_minimized(),
        fullscreen: window.is_fullscreen(),
    }
}

/// The Settings controller's ports (Task 41), made real in Task 44: the diagnostics control on the
/// profile folder, the glass support of this window and the copyright line of the licence.
#[allow(clippy::too_many_arguments)] // one call site; each port is a different thing
fn settings_deps(
    app: &AppWindow,
    prefs: &PrefStore,
    post: Post,
    profile: PathBuf,
    open_url: Rc<dyn Fn(&str)>,
    e2e_on: bool,
    glass: flare_settings::page::GlassSupport,
    verb: Arc<dyn flare_settings::verb::ExplorerVerb>,
) -> crate::settings_ctl::SettingsDeps {
    let look = LookStore::new(prefs.clone());
    let weak = app.as_weak();
    crate::settings_ctl::SettingsDeps {
        prefs: prefs.clone(),
        look: LookStore::new(prefs.clone()),
        diag: Rc::new(crate::startup::DiagPort(
            flare_diag::control::DiagControl::with_log(&profile, flare_diag::writer::main()),
        )),
        verb,
        dictation: Rc::new(crate::dictation_page::DictationPageProvider),
        pick_folder: Rc::new(move |default_dir, answer| {
            let parent = weak
                .upgrade()
                .and_then(|a| hwnd_of(&a))
                .map(|hwnd| ParentWindow { hwnd });
            // Under --e2e an unset seam is a cancel (an empty value): no real dialog shows.
            let seam = e2e::global().dialog_seam(&["FLARE_E2E", "_PICK"].concat(), "", |_| true);
            pick_folder(
                PickOptions {
                    parent,
                    default_dir,
                    seam,
                },
                move |p| answer(p),
            );
        }),
        open_path: Rc::new(move |p: &Path| {
            let text = p.to_string_lossy();
            if flare_term::web_url::is_web_url(&text) {
                open_url(&text);
            } else if e2e_on {
                // Under --e2e nothing opens; the attempt is recorded in the log instead.
                log::info!("e2e: open {text} (nothing opened)");
                crate::openers::record_window_open(p);
            } else {
                // Explorer by its absolute path (never the bare name, which a copy beside the exe
                // would answer), started on a worker so the window never waits on it.
                let path = text.into_owned();
                let spawned = std::thread::Builder::new()
                    .name("flare-open-path".into())
                    .spawn(move || {
                        if let Err(e) = flare_win::openers::explorer_open_path(&path) {
                            log::warn!("open failed: {e}");
                        }
                    });
                if let Err(e) = spawned {
                    log::warn!("open did not start: {e}");
                }
            }
        }),
        clipboard: Rc::new(|_: &str| {}),
        shell_list: Arc::new(|| {
            flare_shells::detect::cached_shells()
                .iter()
                .map(|d| (d.id.clone(), d.name.clone()))
                .collect()
        }),
        glass: Rc::new(move || glass),
        face_of: Rc::new(|id: &str| {
            flare_theme::fonts::font_families(id)
                .into_iter()
                .next()
                .unwrap_or_default()
        }),
        theme_colours: {
            let prefs = prefs.clone();
            Rc::new(move || flare_settings::page::appearance::theme_colours(&look, &prefs, false))
        },
        agent_colours: {
            let prefs = prefs.clone();
            Rc::new(move || crate::settings_ctl::agent_colours_in_force(&prefs))
        },
        agent_key_labels: Rc::new(|| {
            flare_tabs::keymap::agent_chords()
                .into_iter()
                .map(|id| flare_tabs::keymap::key_label(id, true))
                .collect::<Vec<_>>()
                .join(", ")
        }),
        post,
        version: crate::app_facts::version().to_string(),
        copyright: crate::startup::copyright_line(crate::startup::LICENCE),
        e2e: e2e_on,
        clock: Rc::new(crate::ui_clock::monotonic_ms),
        still: Rc::new(|| !crate::e2e::animations_enabled()),
    }
}

/// Hands the probes the renderer the run really got (`Hello` and `app.state` report it).
fn tell_renderer(effective: RendererChoice, fallback: Option<&str>, scale: f32) {
    let word = match (effective, fallback) {
        (_, Some(why)) => format!("software (fallback: {why})"),
        (RendererChoice::Gpu, None) => "gpu".to_string(),
        (RendererChoice::Software, None) => "software".to_string(),
    };
    crate::e2e_hook::set_hello_info(&word, scale);
}

/// Under `--e2e`, hands the probes the grid host's cell size after each frame.
fn note_cell_for_e2e(host: &GridHost) {
    if crate::e2e::global().on() {
        let m = host.metrics();
        crate::e2e_hook::set_cell(m.cell_w, m.cell_h);
    }
}

/// Runs the window until it closes. `start` is main's start time, for the first-frame log.
/// `args.sample` keeps M1's sample grid instead of the tabs; then `instance` is `None`.
pub fn run_window(
    outcome: BackendOutcome,
    start: Instant,
    args: &Args,
    instance: Option<Instance>,
) -> Result<(), slint::PlatformError> {
    let sample = args.sample;
    let app = AppWindow::new()?;
    crate::startup_trace::mark(crate::startup_trace::Phase::Component);
    let painter = match outcome.effective {
        RendererChoice::Gpu => GridPainter::Gpu,
        RendererChoice::Software => GridPainter::Cpu,
    };
    let mut rasterizer = Rasterizer::new(DEFAULT_FAMILIES, 1.0);
    {
        // A fallback font found off the UI thread must repaint an idle window (GPU path).
        let weak = app.as_weak();
        rasterizer.set_fallback_waker(std::sync::Arc::new(move || {
            let _ = weak.upgrade_in_event_loop(|app| app.window().request_redraw());
        }));
    }
    let host = Rc::new(RefCell::new(GridHost::new(Box::new(rasterizer), painter)));
    // Why the grid is not on the GPU even though it was asked for, set by the notifier.
    let note: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(outcome.fallback_reason.clone()));
    let effective = outcome.effective;

    let refresh_overlay = {
        let host = host.clone();
        let note = note.clone();
        let weak = app.as_weak();
        move || {
            let Some(app) = weak.upgrade() else { return };
            let host = host.borrow();
            let mut memory =
                process_memory().map_or_else(|| "unavailable".to_string(), |m| m.summary());
            if let Some(b) = crate::memory::breakdown() {
                memory = format!("{memory}\nparts: {}", b.line());
            }
            let text = overlay_text(
                effective,
                note.borrow().as_deref(),
                host.painter(),
                &host.stats.summary(),
                &memory,
            );
            app.set_overlay_text(text.into());
        }
    };

    // The tab in front. Declared here so every rendered frame can place the IME area (Task 24b).
    let front: FrontCell = Rc::new(RefCell::new(None));
    // The IME caret hold, and the grid size the last frame had (a resize releases the hold).
    let caret: Rc<RefCell<CaretFollower>> = Rc::new(RefCell::new(CaretFollower::new()));
    let ime_dims: Rc<Cell<(usize, usize)>> = Rc::new(Cell::new((0, 0)));

    // Renders into the window's image now.
    let no_notifier = Rc::new(Cell::new(false));
    let first_software_frame = Rc::new(RefCell::new(Once::default()));
    let render_now = {
        let host = host.clone();
        let weak = app.as_weak();
        let no_notifier = no_notifier.clone();
        let first_software_frame = first_software_frame.clone();
        let front = front.clone();
        let caret = caret.clone();
        let ime_dims = ime_dims.clone();
        move || {
            crate::e2e_glue::on_render(crate::e2e_glue::RenderPhase::Before);
            let Some(app) = weak.upgrade() else { return };
            sync_caret_shape(&app, &host);
            let image = host.borrow_mut().render();
            if let Some(image) = image {
                FrameTrace::global().time(TracePhase::SetImage, || app.set_grid_image(image));
                note_cell_for_e2e(&host.borrow());
                crate::e2e_glue::on_render(crate::e2e_glue::RenderPhase::After);
                FrameTrace::global().end_frame();
                place_ime(&app, &front, &caret, &host.borrow(), &ime_dims);
                crate::scrollbar::place(&app, &front);
                // No notifier means no AfterRendering: log the first frame from here.
                if no_notifier.get() && first_software_frame.borrow_mut().fire() {
                    crate::startup_trace::mark(crate::startup_trace::Phase::FirstFrame);
                    log::info!("first frame after {} ms", start.elapsed().as_millis());
                }
            }
        }
    };

    // The controller, once made: the GPU path's frames step the sidebar's slide (audit V3).
    let slide_ctl: Rc<RefCell<Option<std::rc::Weak<RefCell<AppCtl>>>>> = Rc::default();
    let notifier_result = {
        let host = host.clone();
        let note = note.clone();
        let slide_ctl = slide_ctl.clone();
        let weak = app.as_weak();
        let front = front.clone();
        let caret = caret.clone();
        let ime_dims = ime_dims.clone();
        let mut first_frame = true;
        // The frame trace (FLARE_TRACE_FRAME=1): when the render began and when the last one ended.
        let mut render_began: Option<std::time::Instant> = None;
        let mut last_ended: Option<std::time::Instant> = None;
        app.window()
            .set_rendering_notifier(move |state, api| match state {
                RenderingState::RenderingSetup => {
                    if let GraphicsAPI::WGPU30 { device, queue, .. } = api {
                        host.borrow_mut().attach_gpu(device, queue);
                        *note.borrow_mut() = None;
                    } else {
                        host.borrow_mut().detach_gpu();
                        *note.borrow_mut() = Some("Slint renderer has no wgpu device".to_string());
                        log::warn!("renderer: grid on the CPU (Slint renderer has no wgpu device)");
                    }
                    crate::startup_trace::mark(crate::startup_trace::Phase::RenderSetup);
                    if let Some(app) = weak.upgrade() {
                        tell_renderer(
                            effective,
                            note.borrow().as_deref(),
                            app.window().scale_factor(),
                        );
                    }
                }
                RenderingState::BeforeRendering => {
                    // The sidebar's slide takes its place here: after the swap chain gave the frame its
                    // buffer (the vsync wait), right before the items are drawn.
                    if let Some(rc) = slide_ctl.borrow().as_ref().and_then(std::rc::Weak::upgrade)
                        && let Ok(mut c) = rc.try_borrow_mut()
                    {
                        c.on_frame();
                    }
                    crate::e2e_glue::on_render(crate::e2e_glue::RenderPhase::Before);
                    let trace = FrameTrace::global();
                    if trace.on() {
                        let now = std::time::Instant::now();
                        render_began = Some(now);
                        if let Some(ended) = last_ended {
                            trace.add(TracePhase::Gap, now.duration_since(ended));
                        }
                    }
                    let Some(app) = weak.upgrade() else { return };
                    sync_caret_shape(&app, &host);
                    if host.borrow().is_dirty() {
                        let image = host.borrow_mut().render();
                        if let Some(image) = image {
                            trace.time(TracePhase::SetImage, || app.set_grid_image(image));
                            note_cell_for_e2e(&host.borrow());
                            place_ime(&app, &front, &caret, &host.borrow(), &ime_dims);
                            crate::scrollbar::place(&app, &front);
                            app.window().request_redraw();
                        }
                    }
                }
                RenderingState::AfterRendering => {
                    crate::e2e_glue::on_render(crate::e2e_glue::RenderPhase::After);
                    let trace = FrameTrace::global();
                    if trace.on() {
                        let now = std::time::Instant::now();
                        if let Some(began) = render_began.take() {
                            trace.add(TracePhase::Render, now.duration_since(began));
                        }
                        last_ended = Some(now);
                        trace.end_frame();
                    }
                    if first_frame {
                        first_frame = false;
                        crate::startup_trace::mark(crate::startup_trace::Phase::FirstFrame);
                        log::info!("first frame after {} ms", start.elapsed().as_millis());
                    }
                }
                RenderingState::RenderingTeardown => host.borrow_mut().detach_gpu(),
                _ => {}
            })
    };
    if let Err(e) = notifier_result {
        no_notifier.set(true);
        crate::e2e_glue::series_from_redraws();
        host.borrow_mut().detach_gpu();
        *note.borrow_mut() = Some(format!("no rendering notifier: {e}"));
        log::info!("grid on the CPU: no rendering notifier ({e})");
    }

    // ---- the native chrome (macOS): the traffic lights in the top row, applied BEFORE show() -----
    #[cfg(target_os = "macos")]
    {
        app.set_native_caption(true);
        app.set_caption_gap(flare_win::chrome::CAPTION_GAP);
    }

    // ---- the saved position and size, applied BEFORE show() -----------------------------------
    let profile_window_file = instance.as_ref().map(|i| i.profile.window_file());
    #[cfg(not(target_os = "macos"))]
    let initial = profile_window_file.as_ref().map(|file| {
        let areas = work_areas();
        initial_window(&WindowStateStore::read(file, &areas), &areas)
    });
    // macOS: the saved position is points times the saved window's scale, so the screens are asked
    // for in that same space (regression rule 238).
    #[cfg(target_os = "macos")]
    let initial = profile_window_file.as_ref().map(|file| {
        let (state, areas) = WindowStateStore::read_at(file, flare_win::monitors::work_areas_at);
        initial_window(&state, &areas)
    });
    if let Some(init) = &initial {
        apply_initial(&app, init, args.e2e);
    }

    // ---- the controller -----------------------------------------------------------------------
    let mut ctl: Option<CtlRc> = None;
    // The clipboard the controller uses; the e2e glue writes the in-memory one under `--e2e`.
    let mut backends_clip: Option<Arc<dyn flare_win::clipboard_port::ClipboardPort>> = None;
    let mut session_end = None;
    // The control channel's server (Surfaces stage 2); stopped right after the loop ends.
    let mut ctl_server: Option<crate::ctl_app::CtlServer> = None;
    // The Explorer verb (reconciled on a worker after the restore) and the update engine (started
    // after the window is shown), both made with the controller (M9 Task 32).
    let mut verb_switch: Option<Arc<flare_win::verb_switch::VerbSwitch>> = None;
    let mut update_engine: Option<(
        Arc<flare_update::engine::Engine>,
        Option<flare_update::types::Offer>,
    )> = None;
    let mut stores: Option<Stores> = None;
    if let (false, Some(inst)) = (sample, instance.as_ref()) {
        let post = make_post();
        let prefs = PrefStore::new(Arc::new(FileBackend::open(
            inst.profile.settings_file(),
            SAVE_DELAY,
        )));
        // The first-run import (Task 44) runs before anything reads a setting: the tabs, the look and
        // the controllers below all read the settings it may have just written.
        let import_seam = e2e::global().env(&["FLARE_E2E", "_IMPORT_APPDATA"].concat());
        let plan = crate::startup::import_plan(
            args,
            import_seam.as_deref(),
            std::env::var("APPDATA").ok().as_deref(),
        );
        if let Some(report) = crate::startup::import_at_start(&prefs, &plan, &std::env::temp_dir())
        {
            log::info!(
                "first-run import: {:?} in {} ms",
                report.status,
                report.elapsed_ms
            );
        }
        let tabs_store = Arc::new(TabsStore::new(inst.profile.tabs_file(), SAVE_DELAY));
        let window_store = Arc::new(WindowStateStore::new(
            inst.profile.window_file(),
            SAVE_DELAY,
        ));
        let waker: Arc<dyn Fn() + Send + Sync> = {
            let post = Arc::clone(&post);
            Arc::new(move || post(UiMsg::HostWake))
        };
        let e2e_on = args.e2e;
        // The control channel's name is per user and per profile; every shell is told it, with its own
        // secret (Surfaces stage 2). A run that cannot name it (no SID) has no channel.
        let ctl_pipe = crate::ctl_app::pipe_for(args.user_data_dir.as_deref(), &inst.profile.dir);
        let ctl_instance = ctl_pipe
            .clone()
            .map(|pipe| flare_term::session::CtlInstance {
                pipe,
                pid: std::process::id(),
                mcp_exe: std::env::current_exe() // sync-ok: startup, before the window shows
                    .ok()
                    .and_then(|exe| crate::ctl_app::mcp_exe_beside(&exe)),
            });
        // Shells are told about the channel only once its server is really serving (set below): a server
        // that failed to start leaves no tab with a secret and a pipe name someone else might own.
        let ctl_up = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let tab_host = TabHost::with_options(
            Arc::new(PortableSpawner),
            Arc::new(RealShells),
            prefs.clone(),
            waker,
            crate::startup::production_plugin_dir(e2e_on),
            flare_term::session::SessionOptions {
                ctl: ctl_instance.clone(),
                ctl_up: Some(Arc::clone(&ctl_up)),
                // The environment as the person started Flare with: a mini-app's browser takes some WEBVIEW2_*
                // variables out of this process for good (its walls), and a shell must still get them.
                base_env: Some(std::env::vars().collect()),
                ..flare_term::session::SessionOptions::default()
            },
        );
        let ctl_mgr = tab_host.manager();
        // The memory watch sums the shells' memory through the tab host's live pids.
        {
            let mgr = tab_host.manager();
            crate::diag_ctl::set_shell_pids(Arc::new(move || {
                mgr.live_pids().into_iter().map(|(_, pid)| pid).collect()
            }));
        }
        let argv: Vec<String> = std::env::args().collect();
        let cwd = std::env::current_dir().unwrap_or_default(); // sync-ok: startup, before any window
        let launch_folders = folders_from_argv(&argv, &own_files(), &cwd); // sync-ok: startup, before any window
        // The clipboard and the openers are the real ones; under --e2e in-memory stand-ins that never
        // touch the desktop, and a url click is only recorded.
        let backends = Backends::new(e2e_on);
        backends_clip = Some(Arc::clone(&backends.clip_port));
        // Filled right after the controller exists: the grid source paints the controller's decoration.
        let inputs_cell: Rc<RefCell<Option<Rc<RefCell<crate::term_ui::DecorInputs>>>>> =
            Rc::default();
        let deps = Deps {
            prefs: prefs.clone(),
            tabs_store: Arc::clone(&tabs_store),
            window_store: Arc::clone(&window_store),
            host: tab_host,
            // Under --e2e nothing resumes the owner's real conversations, unless a scenario hands
            // over a sandboxed HOME with FLARE_E2E_SESSIONS=1.
            lookup: crate::startup::session_lookup(
                e2e_on,
                e2e::global()
                    .env(&["FLARE_E2E", "_SESSIONS"].concat())
                    .as_deref(),
            ),
            // sync-ok: runs on the stat workers, never on the UI thread
            is_dir: Arc::new(|p: &str| Path::new(p).is_dir()),
            home: crate::app_facts::home().to_string(),
            clock: Rc::new(crate::ui_clock::monotonic_ms),
            post: Arc::clone(&post),
            pick_folder: {
                let weak = app.as_weak();
                Rc::new(move |default_dir, answer| {
                    let parent = weak
                        .upgrade()
                        .and_then(|a| hwnd_of(&a))
                        .map(|hwnd| ParentWindow { hwnd });
                    // Under --e2e an unset seam is a cancel: no real dialog shows.
                    let seam =
                        e2e::global().dialog_seam(&["FLARE_E2E", "_PICK"].concat(), "", |_| true);
                    pick_folder(
                        PickOptions {
                            parent,
                            default_dir,
                            seam,
                        },
                        move |p| answer(p),
                    );
                })
            },
            clipboard: {
                let port = Arc::clone(&backends.clip_port);
                Rc::new(move |text: &str| {
                    // The clipboard can be busy for a moment: a worker does the retries.
                    let text = text.to_string();
                    let port = Arc::clone(&port);
                    let r = std::thread::Builder::new()
                        .name("flare-clipboard".into())
                        .spawn(move || {
                            if let Err(e) = port.write_text(&text, CLIPBOARD_WRITE_MAX) {
                                log::warn!("copy failed: {e}");
                            }
                        });
                    if let Err(e) = r {
                        log::warn!("could not start the clipboard worker: {e}");
                    }
                })
            },
            open_url: backends.open_url(),
            set_grid_source: {
                let host = host.clone();
                let front = front.clone();
                let weak = app.as_weak();
                let inputs_cell = inputs_cell.clone();
                Rc::new(move |handle: Option<Arc<TermHandle>>| {
                    *front.borrow_mut() = handle.clone();
                    match handle {
                        Some(h) => {
                            let source = match inputs_cell.borrow().clone() {
                                Some(inputs) => LiveSource::with_inputs(h, inputs),
                                None => LiveSource::new(h),
                            };
                            host.borrow_mut().set_source(Box::new(source));
                        }
                        None => host
                            .borrow_mut()
                            .set_source(Box::new(crate::host::SampleSource::new())),
                    }
                    if let Some(app) = weak.upgrade() {
                        app.window().request_redraw();
                    }
                })
            },
            raise: {
                let weak = app.as_weak();
                Rc::new(move || {
                    if let Some(app) = weak.upgrade() {
                        raise_window(&app, e2e_on);
                    }
                })
            },
            quit: Rc::new(|| {
                if let Err(e) = slint::quit_event_loop() {
                    log::debug!("quit: {e}");
                }
            }),
            hide: {
                let weak = app.as_weak();
                Rc::new(move || {
                    if let Some(app) = weak.upgrade() {
                        let _ = app.hide();
                    }
                })
            },
            // The folder menu's starters are the owner's real folders: under --e2e there are none, so a scenario
            // sees only what it set up.
            starter_folders: Rc::new(move |answer| {
                if e2e_on {
                    return;
                }
                let r = std::thread::Builder::new()
                    .name("flare-starters".into())
                    .spawn(move || {
                        let found = flare_win::paths::starter_folders();
                        answer(
                            found
                                .into_iter()
                                .map(|p| p.to_string_lossy().into_owned())
                                .collect(),
                        );
                    });
                if let Err(e) = r {
                    log::warn!("could not start the starter folders worker: {e}");
                }
            }),
            launch_folders,
            e2e: e2e_on,
            term: TermPorts {
                clip_port: Arc::clone(&backends.clip_port),
                openers: Arc::clone(&backends.openers),
                agent: None,
                dbl: system_double_click(),
                stat: Arc::new(FsStat),
                metrics: {
                    let host = host.clone();
                    Rc::new(move || host.borrow().metrics())
                },
                mark_grid_dirty: {
                    let host = host.clone();
                    Rc::new(move || {
                        if let Ok(mut h) = host.try_borrow_mut() {
                            h.mark_dirty();
                        }
                    })
                },
            },
        };
        let rc = AppCtl::new(&app, deps);
        // The server answers on its own threads and reaches the window through `post`. A failure
        // (the name is taken, no SID) is logged and the run goes on without the channel.
        // Mini-apps (stage 5) live in `<profile>\apps`. The folders of an earlier run have no panel: the
        // folder is renamed aside (one cheap call, so the channel can write a fresh root at once) and
        // the renamed ones are deleted on a worker.
        let apps_root = flare_surfaces::app::apps_root(&inst.profile.dir);
        flare_surfaces::app::retire_root(&apps_root); // sync-ok: startup, before the window shows
        rc.borrow_mut().set_apps_root(apps_root.clone());
        // Mods (stage 8) and plugins (plugins phase 2): all in `<profile>\plugins` (phase 2c), found on a worker at
        // start and when Settings > Plugins opens; the first scan moves the old `<profile>\mods` in. Off until the
        // person turns one on.
        let mods_hub = Arc::new(flare_mods::Hub::new(flare_mods::hub::Config {
            dir: inst.profile.plugins_dir(),
            old_mods_dir: Some(inst.profile.old_mods_dir()),
            state_file: inst.profile.mods_file(),
        }));
        // Safe mode (design 13): `--no-plugins`, Shift held at launch, or two unclean ends in a row with plugins on
        // keep every plugin off this start. The e2e runs are killed at will, so only the flag counts there.
        let asked = args.no_plugins || (!args.e2e && crate::plugin_safe::shift_held());
        let safe = if args.e2e {
            asked.then_some(crate::plugin_safe::WHY_ASKED)
        } else {
            crate::plugin_safe::start(&inst.profile.dir, asked)
        };
        if let Some(why) = safe {
            log::warn!("plugins are off for this start (safe mode)");
            mods_hub.set_safe_mode(why);
        }
        let run_record = (!args.e2e).then(|| crate::plugin_safe::path_in(&inst.profile.dir));
        // A removal (or a change of maker) deletes the plugin's browser profile in both of the plugins' browsers.
        {
            let web_dir = flare_surfaces::web::webview_folder(
                args.user_data_dir.as_deref(),
                flare_win::paths::local_app_data()
                    .map(|p| p.to_string_lossy().into_owned())
                    .as_deref(),
            )
            .unwrap_or_else(|| inst.profile.dir.join("WebView2"));
            mods_hub.set_profile_dirs(Arc::new(move |id: &str| {
                let name = flare_surfaces::modview::profile_name(id);
                [
                    flare_surfaces::modview::env_folder(&web_dir),
                    flare_surfaces::modview::net_env_folder(&web_dir),
                ]
                .into_iter()
                .map(|env| env.join("EBWebView").join(&name))
                .collect()
            }));
        }
        let mut mods_dispatcher = None;
        let mut owner_events = None;
        if let Some(instance) = &ctl_instance {
            let tabs = Arc::clone(&ctl_mgr);
            match crate::ctl_app::CtlServer::start(
                &instance.pipe,
                Arc::clone(&post),
                Arc::new(move |token: &str| tabs.tab_for_ctl_token(token)),
                crate::app_facts::home().to_string(),
                {
                    let prefs = prefs.clone();
                    Arc::new(move || flare_settings::term_prefs::agent_eval(&prefs))
                },
                {
                    let cfg = crate::ctl_app::AppsCfg::new(
                        apps_root,
                        Arc::new(flare_term::path_open::resolve_html_file),
                        {
                            let prefs = prefs.clone();
                            Arc::new(move || flare_settings::term_prefs::app_network(&prefs))
                        },
                        crate::ctl_app::MERMAID,
                    );
                    // The e2e's seam (read only under --e2e): pages without their policy.
                    if crate::ship_ctl::e2e_var("APP_NO_POLICY").is_some() {
                        cfg.without_policy()
                    } else {
                        cfg
                    }
                },
                Some(Arc::clone(&mods_hub)),
            ) {
                Ok(server) => {
                    mods_dispatcher = Some(server.dispatcher());
                    owner_events = Some(server.owner_events());
                    rc.borrow_mut().set_ctl(
                        server.events(),
                        crate::appctl::CtlInfo {
                            pipe: instance.pipe.clone(),
                            mcp_exe: instance.mcp_exe.clone(),
                        },
                    );
                    ctl_server = Some(server);
                    ctl_up.store(true, std::sync::atomic::Ordering::Release);
                }
                Err(e) => log::warn!("the control channel did not start: {e}"),
            }
        }
        let plugin_limits = mods_hub.limits();
        let plugin_logs = mods_hub.logs_dir();
        rc.borrow_mut().set_mods(mods_hub, mods_dispatcher);
        rc.borrow_mut().set_plugin_run_record(run_record);
        // Plugins phase 7: helpers need the channel (their pipe), and Install from file its picker.
        if let (Some(events), Some(instance)) = (owner_events, &ctl_instance) {
            rc.borrow_mut().set_plugin_owner_events(events);
            rc.borrow_mut()
                .set_plugin_helpers(plugin_logs, instance.pipe.clone());
        }
        {
            let weak = app.as_weak();
            rc.borrow_mut().set_pick_zip(Rc::new(move |answer| {
                let parent = weak
                    .upgrade()
                    .and_then(|a| hwnd_of(&a))
                    .map(|hwnd| ParentWindow { hwnd });
                // Under --e2e an unset seam is a cancel: no real dialog shows.
                let seam =
                    e2e::global().dialog_seam(&["FLARE_E2E", "_PICK_ZIP"].concat(), "", |_| true);
                flare_win::pick::pick_file(
                    PickOptions {
                        parent,
                        default_dir: None,
                        seam,
                    },
                    ("Plugin (.zip)", &["zip"]),
                    move |p| answer(p),
                );
            }));
        }
        // The panels' file is read here, at start, before the restore gives the tabs their panels.
        let panels_store = Arc::new(flare_surfaces::store::PanelStore::new(
            inst.profile.panels_file(),
            SAVE_DELAY,
        ));
        rc.borrow_mut().set_panel_store(Arc::clone(&panels_store));
        // The web panel's browser is made by the first web panel, never before.
        rc.borrow_mut()
            .set_web_factory(crate::web_ctl::production_factory(
                app.as_weak(),
                args.user_data_dir.clone(),
                inst.profile.dir.clone(),
                Some(plugin_limits),
            ));
        // Windows' notifications and the system's listening table (stage 7); under --e2e a recorder.
        crate::appctl::install_surfaces7(&rc, Arc::clone(&post), args.e2e);
        // The sidebar's slide reads the curve at each frame's own moment, to the fraction of a ms.
        rc.borrow_mut()
            .set_frame_clock(Rc::new(crate::ui_clock::monotonic_ms_f64));
        *slide_ctl.borrow_mut() = Some(Rc::downgrade(&rc));
        crate::dictation_ctl::install(&app, &rc, args);
        // What this run is: installed or not, e2e or not (M9 Task 32). The verb and the update
        // engine follow from it.
        let ship = crate::ship_ctl::Ship::detect(
            std::env::current_exe().unwrap_or_default(), // sync-ok: startup, before the window shows
            args.e2e,
            args.preview_update,
        );
        let switch = ship.real_verb_switch(&inst.profile.dir);
        verb_switch = Some(Arc::clone(&switch));
        if args.e2e {
            crate::e2e_probes::chrome::note_verb_switch(Arc::clone(&switch), ship.root.clone());
        }
        {
            let choice =
                crate::ship_ctl::choose_feed(&ship, &|suffix| crate::ship_ctl::e2e_var(suffix));
            let engine = crate::ship_ctl::build_engine(
                &choice,
                crate::app_facts::version(),
                args.user_data_dir.clone(),
                Arc::new(crate::update_ctl::PostEvents {
                    post: Arc::clone(&post),
                }),
            );
            AppCtl::attach_update(
                &app,
                &rc,
                crate::appctl::UpdateParts {
                    engine: Arc::clone(&engine),
                    current_version: crate::app_facts::version().to_string(),
                    e2e_dir: args.e2e.then(|| inst.profile.dir.clone()),
                    spawn: crate::update_ctl::std_spawn(),
                },
            );
            update_engine = Some((engine, choice.preset_offer));
        }
        // Install when idle (2026-10-29): an installed copy watches for a build the agent staged.
        if let Some((layout, start)) = crate::appctl::staged_glue::started() {
            AppCtl::attach_staged(
                &rc,
                crate::appctl::staged_glue::StagedParts {
                    layout,
                    start,
                    times: crate::appctl::staged_glue::times(),
                    spawn: crate::update_ctl::std_spawn(),
                    restart: crate::appctl::staged_glue::spawn_exe(),
                    watch: true,
                    user_data_dir: args.user_data_dir.clone(),
                    argv: std::env::args().skip(1).collect(),
                    e2e_dir: args.e2e.then(|| inst.profile.dir.clone()),
                },
            );
        }
        // The Settings controller (Task 41). Task 44 swaps the stand-ins (the diagnostics control,
        // the shell list) for the real ones.
        {
            let glass = crate::startup::glass_support(effective, flare_win::os_build::os_version());
            let sdeps = settings_deps(
                &app,
                &prefs,
                Arc::clone(&post),
                inst.profile.dir.clone(),
                backends.open_url(),
                e2e_on,
                glass,
                Arc::new(flare_win::verb_switch::RegistryVerb::new(switch)),
            );
            let settings = crate::settings_ctl::SettingsCtl::new(&app, sdeps);
            if let Some(port) = rc.borrow().mods_port() {
                settings.borrow_mut().set_mods(port);
            }
            rc.borrow_mut().set_settings(settings);
        }
        *inputs_cell.borrow_mut() = Some(rc.borrow().decor_inputs());
        APP_CTL.with(|cell| *cell.borrow_mut() = Some((rc.clone(), app.as_weak())));
        // The second launch's folders arrive on the pipe thread (running since main took the
        // lock) and hop onto the event loop; any that came while the window was being made go now.
        inst.sink.attach(Arc::clone(&post));
        // A logoff or shutdown flushes both stores synchronously (the OS ends us right after).
        session_end = match SessionEndWatch::start_with_power(
            {
                let tabs = Arc::clone(&tabs_store);
                let window = Arc::clone(&window_store);
                let prefs = prefs.clone();
                let panels = Arc::clone(&panels_store);
                move || {
                    flush_stores(&tabs, &window, &prefs, &panels);
                    // The log says so last, and is on the disk before the OS ends us.
                    crate::diag_ctl::session_end();
                }
            },
            // A sleep is not a stall: the watchdog drops the gap across it.
            crate::diag_ctl::power_events(),
        ) {
            Ok(w) => Some(w),
            Err(e) => {
                log::warn!("no session-end watcher: {e}");
                None
            }
        };
        // The grid's face and size (Task 27): the settings, or this run's --font and --font-pct.
        rc.borrow_mut().set_font_ctl(
            FontCtl::new(LookStore::new(prefs.clone()), host.clone())
                .with_overrides(args.look.font.clone(), args.look.font_pct),
        );
        // A font setting changed: the controller's own look listener (`LookChanged`) has the grid
        // wear the new face and size, Settings open or not.
        stores = Some((tabs_store, window_store, prefs, panels_store));
        crate::startup_trace::mark(crate::startup_trace::Phase::Profile);
        // The memory breakdown (F12 overlay, probe `memory.breakdown`) reads the grid host and each
        // tab's scrollback.
        {
            let host = host.clone();
            let weak = Rc::downgrade(&rc);
            crate::memory::set_breakdown_source(Rc::new(move || {
                let mut b = crate::memory::Breakdown::default();
                if let Ok(h) = host.try_borrow() {
                    h.fill_breakdown(&mut b);
                }
                if let Some(c) = weak.upgrade()
                    && let Ok(c) = c.try_borrow()
                {
                    b.scrollback = c.scrollbacks();
                }
                b.help_index = flare_help::search::index_bytes();
                b.settings_catalogue = flare_settings::rows::all_rows().count()
                    * size_of::<flare_settings::rows::RowDef>();
                b
            }));
        }
        {
            // The end of life stops listening first, so a late launch finds no pipe, waits for
            // the lock and becomes the first instance (rule 32: every argv path is taken).
            let slot = inst.server.clone();
            rc.borrow_mut().set_handoff_stop(Rc::new(move || {
                let s = slot.borrow_mut().take();
                if let Some(s) = s {
                    s.stop();
                }
            }));
        }
        ctl = Some(rc);
    }

    // ---- the callbacks the controller leaves to the window -----------------------------------
    app.on_grid_resized({
        let host = host.clone();
        let render_now = render_now.clone();
        let ctl = ctl.as_ref().map(Rc::downgrade);
        move |w, h, scale| {
            crate::diag_ctl::timed("on_grid_resized", || {
                host.borrow_mut()
                    .resize(w.max(0) as u32, h.max(0) as u32, scale);
                render_now();
                let (cols, rows) = host.borrow().dims();
                if cols == 0 || rows == 0 {
                    return;
                }
                if let Some(rc) = ctl.as_ref().and_then(std::rc::Weak::upgrade)
                    && let Ok(mut c) = rc.try_borrow_mut()
                {
                    c.on_grid_dims(cols, rows);
                }
            });
        }
    });
    app.on_minimize_clicked({
        let weak = app.as_weak();
        move || {
            if let Some(app) = weak.upgrade() {
                app.window().set_minimized(true);
            }
        }
    });
    app.on_maximize_clicked({
        let weak = app.as_weak();
        move || {
            if let Some(app) = weak.upgrade() {
                let window = app.window();
                window.set_maximized(!window.is_maximized());
            }
        }
    });
    // Every route out of the window ends at the controller's close handler (rule 27).
    let close_decision = {
        let ctl = ctl.as_ref().map(Rc::downgrade);
        move || -> CloseDecision {
            match &ctl {
                None => CloseDecision::Close,
                Some(weak) => match weak.upgrade() {
                    None => CloseDecision::Close,
                    Some(rc) => match rc.try_borrow_mut() {
                        Ok(mut c) => c.on_close_requested(),
                        Err(_) => CloseDecision::KeepOpen,
                    },
                },
            }
        }
    };
    app.on_close_clicked({
        let weak = app.as_weak();
        let close_decision = close_decision.clone();
        move || {
            if close_decision() == CloseDecision::Close {
                if let Some(app) = weak.upgrade() {
                    let _ = app.hide();
                }
                let _ = slint::quit_event_loop();
            }
        }
    });
    app.window().on_close_requested(move || {
        crate::diag_ctl::timed("on_close_requested", || match close_decision() {
            CloseDecision::KeepOpen => CloseRequestResponse::KeepWindowShown,
            CloseDecision::Close => CloseRequestResponse::HideWindow,
        })
    });
    let material: Rc<RefCell<Option<WindowMaterial>>> = Rc::new(RefCell::new(None));
    let key_path: Option<Rc<KeyPath>> = ctl.as_ref().map(|rc| {
        install_window_filter(
            &app,
            FilterState {
                ctl: Rc::downgrade(rc),
                front: front.clone(),
                host: host.clone(),
                initial: initial.clone(),
                material: material.clone(),
                caret: caret.clone(),
                e2e: args.e2e,
                software: no_notifier.clone(),
            },
        )
    });
    // macOS: the menu bar, once the event loop runs (winit sets its own default menu at launch).
    #[cfg(target_os = "macos")]
    if let Some(kp) = &key_path {
        install_mac_menu(&app, kp.clone());
    }
    app.on_toggle_grid_renderer({
        let host = host.clone();
        let render_now = render_now.clone();
        let refresh_overlay = refresh_overlay.clone();
        move || {
            crate::diag_ctl::timed("on_toggle_grid_renderer", || {
                host.borrow_mut().toggle_painter();
                render_now();
                refresh_overlay();
            });
        }
    });

    // The overlay refreshes twice a second, and only while it is shown.
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(500), {
        let weak = app.as_weak();
        let refresh_overlay = refresh_overlay.clone();
        move || {
            if weak.upgrade().is_some_and(|app| app.get_overlay_visible()) {
                crate::diag_ctl::timed("overlay_refresh", &refresh_overlay);
            }
        }
    });

    // Without a notifier nothing re-renders a dirty grid, so poll for it (M2's live output).
    let frame_timer = slint::Timer::default();
    if no_notifier.get() {
        // When the grid was last painted, by the timer or by a wake-up.
        let last_paint = Rc::new(Cell::new(std::time::Instant::now()));
        // The caret shape is read here too: a popup opening does not dirty the grid by itself.
        let caret_weak = app.as_weak();
        frame_timer.start(slint::TimerMode::Repeated, FRAME_POLL, {
            let host = host.clone();
            let render_now = render_now.clone();
            let last_paint = last_paint.clone();
            let caret_weak = caret_weak.clone();
            move || {
                if let Some(app) = caret_weak.upgrade() {
                    sync_caret_shape(&app, &host);
                }
                if repaint_due(host.borrow().is_dirty()) {
                    last_paint.set(std::time::Instant::now());
                    crate::diag_ctl::timed("frame_render", &render_now);
                }
            }
        });
        let hook: Rc<dyn Fn()> = {
            let host = host.clone();
            let render_now = render_now.clone();
            let caret_weak = caret_weak.clone();
            Rc::new(move || {
                if let Some(app) = caret_weak.upgrade() {
                    sync_caret_shape(&app, &host);
                }
                if paint_on_wake(last_paint.get().elapsed(), host.borrow().is_dirty()) {
                    last_paint.set(std::time::Instant::now());
                    crate::diag_ctl::timed("frame_render", &render_now);
                }
            })
        };
        SOFTWARE_PAINT.with(|h| *h.borrow_mut() = Some(hook));
    }

    // The look (Task 26): the first frame wears the saved theme, opaque (no flash of the default), because
    // the controller applies BEFORE the show. The window material needs a handle, so it joins after.
    let look_prefs = stores
        .as_ref()
        .map_or_else(PrefStore::memory, |(_, _, p, _)| p.clone());
    let look = {
        let ctl = LookCtl::new(
            &app,
            LookStore::new(look_prefs),
            host.clone(),
            material.clone(),
            &outcome,
            args.look.clone(),
        );
        if sample {
            // Headless-style runs apply once, with their overrides, and never listen. An `--e2e` run
            // listens like the real app: PT's e2e drives Settings and reads the live chrome, and a
            // picked accent or background must reach the window (FLARE-DEFECT, Task 34).
            let rc = ctl.install_quiet();
            rc.borrow_mut().apply_now();
            rc
        } else {
            let rc = ctl.install();
            rc.borrow_mut().apply_now();
            rc
        }
    };
    // An `--e2e` window is placed on the parking spot BEFORE it first shows, so it is never on a
    // monitor, not even for the few ticks until its handle exists and the timer styles it.
    if let Some((x, y)) = e2e_first_position(args.e2e) {
        app.window()
            .set_position(slint::PhysicalPosition::new(x, y));
    }
    app.show()?;
    // The watchdog: the UI thread beats every 50 ms, and the hung-window poll looks at this window.
    let beat_timer = crate::diag_ctl::beat_timer();
    // A bell flashes the taskbar button while the window is not in front (no sound). Under --e2e the
    // window is parked and has no button, so the flash is only counted.
    if let Some(rc) = &ctl {
        let weak = app.as_weak();
        let e2e = args.e2e;
        rc.borrow_mut().set_flash(Rc::new(move || {
            // The HWND on Windows; on macOS the content view, which only says the window exists (the
            // Dock's attention is the app's, `flare_win::dock`).
            let Some(hwnd) = weak.upgrade().and_then(|a| native_view_of(&a)) else {
                return;
            };
            if e2e {
                if !flare_win::taskbar_flash::is_foreground(hwnd) {
                    crate::e2e_probes::core::record_flash();
                }
            } else {
                flare_win::taskbar_flash::flash_unless_foreground(hwnd);
            }
        }));
    }
    // The handle does not exist before the event loop runs: style and park when it appears.
    let park_timer = args.e2e.then(|| park_e2e_window_when_ready(&app));
    if args.e2e {
        tell_renderer(
            effective,
            note.borrow().as_deref(),
            app.window().scale_factor(),
        );
        if let (Some(rc), Some(kp)) = (&ctl, &key_path) {
            crate::e2e_glue::install(&app, rc, args, kp, backends_clip.clone());
        }
    }
    // The agent: the process poll, and the taskbar overlay. Under --e2e the real overlay is never
    // touched: a recording shell stands in, so the badge's text can be read.
    if let Some(rc) = &ctl {
        let mut c = rc.borrow_mut();
        c.start_agent_poll();
        if args.e2e {
            c.set_taskbar_badge(flare_win::taskbar::TaskbarBadge::with_shell(Box::new(
                flare_win::taskbar::RecordShell,
            )));
        } else {
            // macOS: the badge is the Dock tile's, which needs no window handle (Windows waits for
            // the HWND below).
            #[cfg(target_os = "macos")]
            c.set_taskbar_badge(flare_win::taskbar::TaskbarBadge::new(0));
        }
    }
    // What needs the window's handle, once it has one: the watchdog's hung-window poll, the exe's
    // own icon on the window (again on every scale-factor change) and the taskbar overlay.
    let handle_timer = {
        let ctl = ctl.clone();
        let e2e = args.e2e;
        when_handle_ready(
            &app,
            "the watchdog, the icon and the badge",
            move |_, hwnd| {
                crate::diag_ctl::set_window(hwnd);
                crate::startup::apply_window_icon(Some(hwnd));
                // No subclass for `TaskbarButtonCreated`: one beside AccessKit's crashed the window
                // as the first overlay was set (2026-10-26). A gained focus redraws the badge instead.
                if let Some(rc) = ctl.filter(|_| !e2e) {
                    let mut c = rc.borrow_mut();
                    c.set_taskbar_badge(flare_win::taskbar::TaskbarBadge::new(hwnd));
                    // The agent's cursor and the border round the page while it works: windows owned
                    // by this one (under --e2e the recording stand-ins WebCtl starts with stay).
                    // macOS: phase 5 of the port plan; the recording stand-ins stay until then.
                    #[cfg(windows)]
                    {
                        c.set_agent_border(Box::new(flare_win::agent_cursor::GlowWindow::new(
                            hwnd,
                        )));
                        c.set_agent_cursor(Box::new(flare_win::agent_cursor::CursorWindow::new(
                            hwnd,
                        )));
                    }
                }
            },
        )
    };
    if initial.as_ref().is_some_and(|i| i.maximise) {
        app.window().set_maximized(true);
    }
    if !sample {
        app.window().with_winit_window(|w| w.set_ime_allowed(true));
    }
    // The window material (Task 25): the DWM backdrop and edge. It needs the handle of the window,
    // so it is made after the show; the look controller (Task 26) drives it from the settings.
    // The handle can be missing right after the show (the winit backend makes the native window
    // once the event loop runs, as under `--e2e`): a short timer then attaches the material the
    // moment the handle exists, or the window would stay solid for the whole run.
    let material_timer = (!attach_window_material(&app, &material, &look, args.e2e))
        .then(|| attach_material_when_ready(&app, &material, &look, args.e2e));
    // The first size, before Slint's `changed` callbacks have fired.
    host.borrow_mut().resize(
        app.get_grid_phys_width().max(0) as u32,
        app.get_grid_phys_height().max(0) as u32,
        app.get_scale(),
    );
    render_now();
    refresh_overlay();
    if let Some(rc) = &ctl {
        // Placeholders are drawn already; the restore answers on a worker and settles in place.
        AppCtl::start_restore(rc);
    }
    // The window is up and the restore is under way: the update engine starts (an e2e offer is set
    // once, so the chip shows with no check made), and the verb is repaired on a worker.
    if let Some((engine, preset)) = &update_engine {
        engine.start();
        if let Some(offer) = preset {
            engine.set_offer(offer.clone());
        }
    }
    if let Some(switch) = verb_switch.take() {
        // The Explorer verb is repaired on a worker once the first frame is up (M10 Task 63).
        crate::startup_trace::after_first_frame("verb_reconcile", move || switch.reconcile());
    }
    let result = app.run();
    // The control channel stops first of all: no call may reach the window once it is going (a waiting
    // one answers `unavailable` within 50 ms; the stop is bounded by its own 2 s, inside the quit).
    if let Some(server) = ctl_server.take() {
        let left = server.stop();
        if left > 0 {
            log::warn!("control channel: {left} call(s) still running at quit");
        }
    }
    // Dictation stops next: engine, host, hook and link are gone before the shells are killed.
    crate::dictation_ctl::stop_all();
    if let Some((engine, _)) = &update_engine {
        engine.stop();
    }

    // After run(): the end of life once more (idempotent), so no path leaves a shell behind.
    // Never through on_close_requested: a working agent would ask again with no window left to
    // answer it, and the shells would outlive the app.
    if let Some(rc) = &ctl
        && let Ok(mut c) = rc.try_borrow_mut()
    {
        c.end();
    }
    if let Some((tabs, window, prefs, panels)) = &stores {
        flush_stores(tabs, window, prefs, panels);
    }
    // The mini-apps' folders go with the run (one a browser still holds is renamed aside and deleted
    // by the next start).
    if let Some(inst) = instance.as_ref() {
        flare_surfaces::app::clean_root(&flare_surfaces::app::apps_root(&inst.profile.dir)); // sync-ok: quit path, after the window and the channel are gone
        // A clean quit: the plugins' run record is closed (safe mode counts the runs that end without this).
        if !args.e2e {
            crate::plugin_safe::clean_exit(&inst.profile.dir); // sync-ok: quit path, after the window and the channel are gone
        }
    }
    APP_CTL.with(|cell| *cell.borrow_mut() = None);
    LookCtl::uninstall();
    let mut instance = instance;
    let server = instance.as_mut().and_then(|i| i.server.borrow_mut().take());
    if let Some(s) = server {
        s.stop();
    }
    if let Some(w) = session_end {
        w.stop();
    }
    drop(timer);
    drop(frame_timer);
    SOFTWARE_PAINT.with(|h| *h.borrow_mut() = None);
    drop(beat_timer);
    drop(park_timer);
    drop(handle_timer);
    drop(material_timer);
    drop(instance); // the lock is released last
    result
}

/// The size (logical) and position (physical) the window opens with.
/// The saved position to apply, if any. An `--e2e` run never applies one: its window stays parked
/// off every monitor (the size is still applied).
pub fn saved_position(init: &InitialWindow, e2e: bool) -> Option<(i32, i32)> {
    if e2e { None } else { init.pos }
}

fn apply_initial(app: &AppWindow, init: &InitialWindow, e2e: bool) {
    let window = app.window();
    window.set_size(slint::LogicalSize::new(
        init.logical_size.0 as f32,
        init.logical_size.1 as f32,
    ));
    if let Some((x, y)) = saved_position(init, e2e) {
        window.set_position(slint::PhysicalPosition::new(x, y));
    }
}

/// How long after the window is made a scale change may still re-apply the saved bounds.
const REAPPLY_WINDOW: Duration = Duration::from_secs(3);

/// The saved bounds, re-applied ONCE on a scale change at startup (the saved scale may differ
/// from the monitor the window opens on). Later, a scale change means the user moved the window
/// to another monitor, and snapping it back to the saved spot would be wrong: so the chance
/// ends at the first key or click, or after `REAPPLY_WINDOW`.
struct Reapply<T> {
    value: Option<T>,
    since: Instant,
}

impl<T> Reapply<T> {
    fn new(value: Option<T>, since: Instant) -> Self {
        Self { value, since }
    }

    fn user_acted(&mut self) {
        self.value = None;
    }

    fn take(&mut self, now: Instant) -> Option<T> {
        let v = self.value.take()?;
        (now.saturating_duration_since(self.since) <= REAPPLY_WINDOW).then_some(v)
    }
}

struct FilterState {
    ctl: std::rc::Weak<RefCell<AppCtl>>,
    front: FrontCell,
    host: Rc<RefCell<GridHost>>,
    initial: Option<InitialWindow>,
    /// Filled after the show, when the window has its handle.
    material: Rc<RefCell<Option<WindowMaterial>>>,
    /// The IME caret hold (Task 24b): a typed key opens its window.
    caret: Rc<RefCell<CaretFollower>>,
    /// `--e2e`: the saved position is never applied.
    e2e: bool,
    /// Slint's software renderer draws (it has no rendering notifier).
    software: Rc<Cell<bool>>,
}

/// Everything a key (and a focus change) needs: the winit filter and the e2e glue both go through
/// it, so a test key takes exactly the path of a real one.
pub(crate) struct KeyPath {
    pub ctl: std::rc::Weak<RefCell<AppCtl>>,
    pub front: FrontCell,
    /// The IME caret hold (Task 24b): a typed key opens its window.
    pub caret: Rc<RefCell<CaretFollower>>,
    /// Codes whose press the app consumed: their release must not reach the shell either.
    pub consumed: RefCell<HashSet<String>>,
    pub app: slint::Weak<AppWindow>,
    /// The grid's cell metrics and size: a wheel turn is placed on a cell with them.
    pub host: Rc<RefCell<GridHost>>,
    /// The wheel's fractional remainders (lines and touchpad pixels).
    pub wheel_accum: RefCell<WheelAccum>,
}

/// One wheel turn as the window or the e2e glue receives it.
#[derive(Debug, Clone, Copy)]
pub(crate) enum WheelDelta {
    /// Wheel lines (a mouse notch is 1.0; a smooth touchpad sends fractions).
    Lines(f32),
    /// Touchpad pixels, physical.
    Pixels(f64),
}

impl KeyPath {
    /// Whether a point (css px) is the terminal's to scroll: the one gate the real wheel and the e2e
    /// wheel share. Over the strip, the sidebar's tab list, the help sheet, a menu or any popup the
    /// wheel stays with Slint (audit 2026-10-20: M5).
    pub(crate) fn wheel_is_terminals_at(&self, app: &AppWindow, p_css: (f32, f32)) -> bool {
        let k = app.get_scale().max(0.01);
        let help_open = app.global::<HelpBridge>().get_open();
        let window_w = app.window().size().width as f32 / k;
        let gate = WheelGate {
            front_is_grid: app.get_front_kind() == 0,
            chrome_focus: app.get_chrome_has_focus(),
            menu_open: app.get_menu_open(),
            grid: (
                app.get_grid_left(),
                app.get_grid_top(),
                app.get_grid_phys_width() as f32 / k,
                app.get_grid_phys_height() as f32 / k,
            ),
            help_from: help_open.then(|| window_w - app.get_help_sheet_w()),
        };
        wheel_is_terminals(p_css, &gate)
    }

    /// One wheel turn at `cursor_phys` (window pixels), through the path the real window takes and
    /// the e2e glue shares: the gate, the notches (whole ones, the remainder kept), Ctrl zooms, else
    /// the front shell scrolls at the cell under the pointer (the grid's own origin, not a repeated
    /// layout) and the GPU path is woken.
    pub(crate) fn wheel(
        &self,
        delta: WheelDelta,
        cursor_phys: (f64, f64),
        ctrl: bool,
    ) -> EventResult {
        let Some(app) = self.app.upgrade() else {
            return EventResult::Propagate;
        };
        let Some(handle) = self.front.borrow().clone() else {
            return EventResult::Propagate;
        };
        let k = f64::from(app.get_scale()).max(0.01);
        let (x, y) = cursor_phys;
        if !self.wheel_is_terminals_at(&app, ((x / k) as f32, (y / k) as f32)) {
            return EventResult::Propagate;
        }
        let (m, (cols, rows)) = {
            let h = self.host.borrow();
            (h.metrics(), h.dims())
        };
        let notches = match delta {
            WheelDelta::Lines(l) => self.wheel_accum.borrow_mut().add_lines(l),
            WheelDelta::Pixels(px) => self.wheel_accum.borrow_mut().add(px, m.cell_h),
        };
        // Ctrl+wheel zooms the tab in front, one pixel a notch, never saved (Task 27).
        if ctrl {
            for _ in 0..notches.unsigned_abs().min(64) {
                self.with_ctl(&mut |c| {
                    c.zoom_front(f64::from(notches.signum()));
                });
            }
            return EventResult::PreventDefault;
        }
        if notches != 0 {
            let origin = (
                f64::from(app.get_grid_left()) * k,
                f64::from(app.get_grid_top()) * k,
            );
            let (col, row) = cell_at(x, y, origin, &m, cols, rows);
            if handle.wheel(notches, col, row) {
                app.window().request_redraw();
            }
        }
        EventResult::PreventDefault
    }

    fn with_ctl(&self, f: &mut dyn FnMut(&mut AppCtl)) {
        if let Some(rc) = self.ctl.upgrade()
            && let Ok(mut c) = rc.try_borrow_mut()
        {
            f(&mut c);
        }
    }
}

/// Makes Flare's menu the Mac menu bar (`mac_menu`), after winit's launch has set its default one.
/// Each item runs on the next turn of the loop, never inside AppKit's menu tracking.
#[cfg(target_os = "macos")]
fn install_mac_menu(app: &AppWindow, kp: Rc<KeyPath>) {
    let weak = app.as_weak();
    slint::Timer::single_shot(Duration::ZERO, move || {
        let kp = kp.clone();
        let weak = weak.clone();
        let handler: flare_win::menu_bar::MenuHandler = Rc::new(move |tag| {
            let Some(cmd) = crate::mac_menu::from_tag(tag) else {
                return;
            };
            let kp = kp.clone();
            let weak = weak.clone();
            slint::Timer::single_shot(Duration::ZERO, move || run_menu_cmd(&kp, &weak, cmd));
        });
        if !flare_win::menu_bar::install(&crate::mac_menu::main_menu(), handler) {
            log::warn!("the menu bar could not be installed");
        }
    });
}

/// A menu item: a key item is its key press, through the same path a typed key takes (a key a
/// Slint field has goes on to Slint, as Cmd, which Slint calls Control on macOS); Quit is the
/// window's close route; Command help is the title button's.
#[cfg(target_os = "macos")]
fn run_menu_cmd(kp: &KeyPath, app: &slint::Weak<AppWindow>, cmd: crate::mac_menu::MenuCmd) {
    use crate::mac_menu::MenuCmd;
    match cmd {
        MenuCmd::Quit => {
            if let Some(a) = app.upgrade() {
                a.invoke_close_clicked();
            }
        }
        MenuCmd::CommandHelp => kp.with_ctl(&mut |c| c.menu_command_help()),
        _ => {
            let Some(ev) = crate::mac_menu::key_event(cmd) else {
                return;
            };
            let down = handle_key(kp, &ev);
            let mut up = ev.clone();
            up.pressed = false;
            let _ = handle_key(kp, &up);
            if matches!(down, EventResult::Propagate)
                && let Some(a) = app.upgrade()
            {
                forward_to_slint(&a, &ev);
            }
        }
    }
}

/// A Cmd key press for a Slint field (copy, paste, select all there).
#[cfg(target_os = "macos")]
fn forward_to_slint(app: &AppWindow, ev: &KeyEvent) {
    use slint::platform::{Key, WindowEvent};
    let w = app.window();
    let mut mods: Vec<slint::SharedString> = vec![Key::Control.into()];
    if ev.shift {
        mods.push(Key::Shift.into());
    }
    let key: slint::SharedString = ev.key.to_lowercase().into();
    for m in &mods {
        w.dispatch_event(WindowEvent::KeyPressed { text: m.clone() });
    }
    w.dispatch_event(WindowEvent::KeyPressed { text: key.clone() });
    w.dispatch_event(WindowEvent::KeyReleased { text: key });
    for m in mods.iter().rev() {
        w.dispatch_event(WindowEvent::KeyReleased { text: m.clone() });
    }
}

/// One key event, from winit or from the test hook: the controller first, then the shell.
/// Returns what the window should do with the event.
pub(crate) fn handle_key(kp: &KeyPath, ev: &KeyEvent) -> EventResult {
    if reserved_for_slint(&ev.code) {
        return EventResult::Propagate;
    }
    if ev.pressed {
        // A typed key opens the IME caret hold (Task 24b).
        kp.caret
            .borrow_mut()
            .on_key(crate::ui_clock::monotonic_ms() as i64);
    }
    let chrome_focus = kp
        .app
        .upgrade()
        .is_some_and(|a| a.get_chrome_has_focus() || a.get_find_input_focused());
    let outcome = if ev.pressed {
        let mut out = KeyOutcome::Slint;
        kp.with_ctl(&mut |c| out = c.on_key(ev));
        if out == KeyOutcome::Consumed {
            kp.consumed.borrow_mut().insert(ev.code.clone());
        }
        out
    } else {
        kp.with_ctl(&mut |c| c.on_key_up(ev));
        if kp.consumed.borrow_mut().remove(&ev.code) {
            KeyOutcome::Consumed
        } else if chrome_focus {
            KeyOutcome::Slint
        } else {
            KeyOutcome::Pty
        }
    };
    let (action, to_pty) = key_outcome_to_filter(outcome);
    if !to_pty {
        return to_event_result(action);
    }
    let Some(handle) = kp.front.borrow().clone() else {
        // Nothing to type into (Settings, the start screen): swallow the key.
        return EventResult::PreventDefault;
    };
    // AppCtl decided (copy, paste, delete over a selection and the agent's newline are
    // its own); what is left for the shell is typed here, the Mac way on macOS (Cmd never types).
    let mut os = crate::keymap::app_key_os();
    let mut option_as_meta = false;
    kp.with_ctl(&mut |c| {
        os = c.key_os();
        option_as_meta = c.option_as_meta();
    });
    match crate::keymap::pty_key(os, ev, option_as_meta) {
        PtyKey::Event(e) => {
            handle.send_key(&e);
        }
        PtyKey::Text(t) => handle.send_text(&t),
        PtyKey::Nothing => {}
    }
    EventResult::PreventDefault
}

/// The window gained or lost the focus (winit's `Focused`, or the test hook's `Focus`).
pub(crate) fn handle_focus(kp: &KeyPath, on: bool) {
    if let Some(app) = kp.app.upgrade() {
        system_caret(&app, |p| p.focus(on));
    }
    if on {
        // Coming back to the window is a look at the tab in front.
        kp.with_ctl(&mut |c| c.on_focus(true));
    } else {
        kp.with_ctl(&mut |c| c.on_focus_lost());
    }
}

thread_local! {
    /// The Win32 system caret (issue #70): it follows the IME area, so magnifiers and screen readers see where the
    /// typing goes. Kept on the UI thread, which owns the window and so the caret.
    static SYSTEM_CARET: RefCell<flare_win::system_caret::CaretPlan> = RefCell::default();
}

/// Plans a change to the system caret and makes its calls on the window.
fn system_caret(
    app: &AppWindow,
    step: impl FnOnce(&mut flare_win::system_caret::CaretPlan) -> Vec<flare_win::system_caret::CaretOp>,
) {
    let ops = SYSTEM_CARET.with(|p| step(&mut p.borrow_mut()));
    if ops.is_empty() {
        return;
    }
    if let Some(hwnd) = hwnd_of(app) {
        flare_win::system_caret::apply(hwnd, &ops);
    }
}

/// The caret follows the keyboard: an outline while a popup, a dialog or a field holds it (audit V11).
fn sync_caret_shape(app: &AppWindow, host: &RefCell<GridHost>) {
    let hollow = crate::live::caret_is_hollow(
        app.get_chrome_has_focus(),
        app.get_find_input_focused(),
        app.get_menu_open(),
    );
    host.borrow_mut().set_hollow_cursor(hollow);
}

/// Each rendered frame: the IME rectangle goes to the cell the caret holds (Task 24b). The hold is
/// released on the alternate screen, after a resize and on a scrolled-back view. Nothing to place
/// without a tab in front.
fn place_ime(
    app: &AppWindow,
    front: &FrontCell,
    caret: &RefCell<CaretFollower>,
    host: &GridHost,
    last_dims: &Cell<(usize, usize)>,
) {
    let Some(handle) = front.borrow().clone() else {
        return;
    };
    let dims = host.dims();
    let resized = last_dims.replace(dims) != dims;
    let (cursor, scrolled, alt) = handle.with_emu_quiet(|e| {
        let (line, col) = e.cursor();
        (
            CaretPos {
                line: i64::from(line),
                x: col as i64,
            },
            e.display_offset() > 0,
            e.mode().alt_screen,
        )
    });
    let stops: Vec<(CaretPos, i64)> = handle
        .take_cursor_stops()
        .into_iter()
        .map(|s| {
            (
                CaretPos {
                    line: i64::from(s.line),
                    x: s.col as i64,
                },
                s.at_ms,
            )
        })
        .collect();
    caret.borrow_mut().on_stops(&stops);
    let held = caret.borrow_mut().on_frame(
        cursor,
        resized || scrolled || alt,
        crate::ui_clock::monotonic_ms() as i64,
    );
    let scale = app.get_scale();
    let origin = (
        (app.get_grid_left() * scale).round() as i32,
        (app.get_grid_top() * scale).round() as i32,
    );
    let area = ime_area(
        held.line.max(0) as usize,
        held.x.max(0) as usize,
        &host.metrics(),
        origin,
    );
    crate::ime::record_area(area);
    // The focus is read here too, from Windows: a window that had it before the filter was installed never saw the
    // event.
    let focused = hwnd_of(app).is_some_and(flare_win::system_caret::has_focus);
    system_caret(app, |p| {
        let mut ops = p.focus(focused);
        ops.extend(p.place(area.x, area.y, area.h as i32));
        ops
    });
    let _ = app.window().with_winit_window(|w| {
        w.set_ime_cursor_area(
            PhysicalPosition::new(area.x, area.y),
            PhysicalSize::new(area.w, area.h),
        );
    });
}

/// One winit filter. It only converts: keys go to `AppCtl::on_key`, and the pointer, the files and
/// the window's own movements are reported to it. Physical keys, IME text and the wheel reach the
/// FRONT shell; a chord never does (the controller consumes it first).
fn install_window_filter(app: &AppWindow, st: FilterState) -> Rc<KeyPath> {
    let FilterState {
        ctl,
        front,
        host,
        initial,
        material,
        caret,
        e2e,
        software,
    } = st;
    let mods = Cell::new(ModifiersState::empty());
    let cursor = Cell::new((0.0_f64, 0.0_f64));
    // The key path (and the codes whose press the app consumed) is shared with the e2e glue.
    let kp = Rc::new(KeyPath {
        ctl: ctl.clone(),
        front: front.clone(),
        caret,
        consumed: RefCell::new(HashSet::new()),
        app: app.as_weak(),
        host: host.clone(),
        wheel_accum: RefCell::new(WheelAccum::new()),
    });
    let kp_out = kp.clone();
    let reapply = RefCell::new(Reapply::new(
        initial.filter(|i| !i.maximise),
        Instant::now(),
    ));
    let weak = app.as_weak();

    // While the pointer is OUTSIDE the window and the sidebar is collapsed, the cursor is read from
    // the system every 50 ms so the reveal zone just outside the edge still works.
    let edge_timer = Rc::new(slint::Timer::default());
    let edge = Rc::new(RefCell::new(EdgeWatch::new(
        Box::new({
            let weak = weak.clone();
            move || {
                let app = weak.upgrade()?;
                let w = app.window();
                let scale = f64::from(w.scale_factor()).max(0.01);
                let (p, s) = (w.position(), w.size());
                Some(RectF {
                    x: f64::from(p.x) / scale,
                    y: f64::from(p.y) / scale,
                    width: f64::from(s.width) / scale,
                    height: f64::from(s.height) / scale,
                })
            }
        }),
        Box::new({
            let weak = weak.clone();
            move || {
                let scale = weak
                    .upgrade()
                    .map_or(1.0, |a| f64::from(a.window().scale_factor()))
                    .max(0.01);
                real_cursor_phys().map_or(
                    PointF {
                        x: -1.0e6,
                        y: -1.0e6,
                    },
                    |(x, y)| PointF {
                        x: f64::from(x) / scale,
                        y: f64::from(y) / scale,
                    },
                )
            }
        }),
        Box::new({
            let ctl = ctl.clone();
            move |near: EdgeNear| {
                let Some(rc) = ctl.upgrade() else { return };
                let Ok(mut c) = rc.try_borrow_mut() else {
                    return;
                };
                if near.x == FAR.x && near.y == FAR.y {
                    c.on_cursor_left();
                } else {
                    c.on_cursor(near.x as f32, near.y as f32);
                }
            }
        }),
    )));

    // The pointer left the window: hover ends, and a collapsed sidebar starts watching the edge.
    let leave_held = Rc::new(Cell::new(false));
    let pointer_left: Rc<dyn Fn()> = {
        let ctl = ctl.clone();
        let weak = app.as_weak();
        let edge = edge.clone();
        let edge_timer = edge_timer.clone();
        Rc::new(move || {
            if let Some(rc) = ctl.upgrade()
                && let Ok(mut c) = rc.try_borrow_mut()
            {
                c.on_cursor_left();
            }
            if weak.upgrade().is_some_and(|a| a.get_sidebar_collapsed()) {
                edge.borrow_mut().set(true);
                let edge = edge.clone();
                edge_timer.start(
                    slint::TimerMode::Repeated,
                    Duration::from_millis(flare_win::edge_watch::EDGE_EVERY_MS),
                    move || edge.borrow_mut().tick(),
                );
            }
        })
    };
    // untimed: winit events arrive per pixel, so the handler keeps its own cheap path
    app.window().on_winit_window_event(move |window, event| {
        let with_ctl = |f: &mut dyn FnMut(&mut AppCtl)| {
            if let Some(rc) = ctl.upgrade()
                && let Ok(mut c) = rc.try_borrow_mut()
            {
                f(&mut c);
            }
        };
        let scale = f64::from(window.scale_factor()).max(0.01);
        match event {
            WindowEvent::RedrawRequested => {
                // The sidebar's slide (audit V3): while it runs, the next frame is asked for straight from
                // the OS window, not through Slint's frame timer, and each present paces it (the GPU path on
                // vsync, the software path on the compositor; measured 2026-10-20, an added wait for the next
                // refresh only doubled some gaps). The software renderer has no notifier, so its frame steps
                // the slide here (its start the first time, its place on the curve now); the GPU path's
                // BeforeRendering does, after the vsync wait.
                let mut more = false;
                with_ctl(&mut |c| {
                    more = if software.get() {
                        c.on_frame()
                    } else {
                        c.slide_running()
                    }
                });
                if more {
                    window.request_redraw();
                }
                // The software renderer's frames, for the e2e's series (a no-op unless it asked for them).
                crate::e2e_glue::on_redraw();
                EventResult::Propagate
            }
            WindowEvent::ModifiersChanged(m) => {
                mods.set(m.state());
                EventResult::Propagate
            }
            WindowEvent::CursorEntered { .. } => {
                // The pointer is back: a leave held back during a drag is no longer owed.
                leave_held.set(false);
                edge_timer.stop();
                edge.borrow_mut().set(false);
                EventResult::Propagate
            }
            WindowEvent::CursorLeft { .. } => {
                // A splitter drag keeps tracking under the OS capture, as every native one does: the
                // pointer past the window's edge is still the drag's, and the release settles it (rule 219).
                if weak.upgrade().is_some_and(|a| a.get_panel_splitting()) {
                    leave_held.set(true);
                    return EventResult::PreventDefault;
                }
                pointer_left();
                EventResult::Propagate
            }
            WindowEvent::CursorMoved { position, .. } => {
                cursor.set((position.x, position.y));
                let (x, y) = ((position.x / scale) as f32, (position.y / scale) as f32);
                with_ctl(&mut |c| c.on_cursor(x, y));
                EventResult::Propagate
            }
            // Slint's TouchArea drops its grab on ANY button's release, so another button's press or
            // release mid-drag never reaches it: the drag would freeze.
            WindowEvent::MouseInput { button, .. }
                if !matches!(button, MouseButton::Left)
                    && weak.upgrade().is_some_and(|a| a.get_panel_splitting()) =>
            {
                EventResult::PreventDefault
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                ..
            } => {
                reapply.borrow_mut().user_acted();
                with_ctl(&mut |c| c.on_pointer_down());
                EventResult::Propagate
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => {
                // The left button up ends a splitter drag wherever the pointer is, before Slint sees it:
                // Slint drops its grab on any button's release, so this is the drag's one sure end.
                if let Some(a) = weak.upgrade() {
                    a.invoke_end_splitter();
                }
                // A leave held back during the drag is owed now, once Slint has the release.
                if leave_held.replace(false) {
                    let (weak, pointer_left) = (weak.clone(), pointer_left.clone());
                    slint::Timer::single_shot(Duration::ZERO, move || {
                        if let Some(a) = weak.upgrade() {
                            a.window()
                                .dispatch_event(slint::platform::WindowEvent::PointerExited);
                        }
                        pointer_left();
                    });
                }
                EventResult::Propagate
            }
            WindowEvent::HoveredFile(_) => {
                with_ctl(&mut |c| c.on_hovered_file());
                EventResult::Propagate
            }
            WindowEvent::HoveredFileCancelled => {
                with_ctl(&mut |c| c.on_hovered_file_cancelled());
                EventResult::Propagate
            }
            WindowEvent::DroppedFile(path) => {
                // File drags send no CursorMoved: read where the pointer is now.
                let at = real_cursor_phys()
                    .map(|(x, y)| {
                        let p = window.position();
                        (f64::from(x - p.x) / scale, f64::from(y - p.y) / scale)
                    })
                    .unwrap_or_else(|| (cursor.get().0 / scale, cursor.get().1 / scale));
                let path: PathBuf = path.clone();
                with_ctl(&mut |c| c.on_dropped_path(path.clone(), (at.0 as f32, at.1 as f32)));
                EventResult::Propagate
            }
            WindowEvent::Focused(on) => {
                handle_focus(&kp, *on);
                EventResult::Propagate
            }
            WindowEvent::Moved(_) | WindowEvent::Resized(_) => {
                // The software window reads 1.0 until its first resize at the monitor's scale (audit V6).
                crate::e2e_hook::set_hello_scale(window.scale_factor());
                let (maximised, fullscreen) = (window.is_maximized(), window.is_fullscreen());
                // The look controller keeps the UI and the material in step (D7); without one
                // the material alone follows.
                if !LookCtl::with_current(|l| l.window_state(maximised, fullscreen, Instant::now()))
                    && let Ok(mut m) = material.try_borrow_mut()
                    && let Some(m) = m.as_mut()
                {
                    m.on_window_state(maximised, fullscreen, Instant::now());
                }
                let o = observe(window);
                with_ctl(&mut |c| c.on_geometry(o));
                EventResult::Propagate
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                // Hello reports the window's own scale, not the 1.0 read before the monitor's arrived.
                crate::e2e_hook::set_hello_scale(*scale_factor as f32);
                // The saved scale may differ from the monitor the window opened on: set the
                // saved logical size and physical position once more at the real scale.
                if let Some(init) = reapply.borrow_mut().take(Instant::now()) {
                    window.set_size(slint::LogicalSize::new(
                        init.logical_size.0 as f32,
                        init.logical_size.1 as f32,
                    ));
                    if let Some((x, y)) = saved_position(&init, e2e) {
                        window.set_position(slint::PhysicalPosition::new(x, y));
                    }
                }
                // The icon sizes follow the new monitor's DPI.
                let hwnd = window
                    .with_winit_window(|w| match w.window_handle().ok()?.as_raw() {
                        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
                        _ => None,
                    })
                    .flatten();
                crate::startup::apply_window_icon(hwnd);
                let o = observe(window);
                with_ctl(&mut |c| c.on_geometry(o));
                EventResult::Propagate
            }
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } => {
                reapply.borrow_mut().user_acted();
                let ev = key_event_from_parts(
                    event.state == ElementState::Pressed,
                    &event.physical_key,
                    &event.logical_key,
                    event.text.as_deref(),
                    mods.get(),
                );
                handle_key(&kp, &ev)
            }
            WindowEvent::Ime(Ime::Commit(s)) => {
                if weak
                    .upgrade()
                    .is_some_and(|a| a.get_chrome_has_focus() || a.get_find_input_focused())
                {
                    return EventResult::Propagate;
                }
                match front.borrow().as_ref() {
                    Some(handle) => {
                        handle.send_text(s);
                        EventResult::PreventDefault
                    }
                    None => EventResult::Propagate,
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let delta = match delta {
                    MouseScrollDelta::LineDelta(_, y) => WheelDelta::Lines(*y),
                    MouseScrollDelta::PixelDelta(p) => WheelDelta::Pixels(p.y),
                };
                let mut os = crate::keymap::app_key_os();
                kp.with_ctl(&mut |c| os = c.key_os());
                let zoom = crate::keymap::zoom_wheel_held(os, mods.get());
                kp.wheel(delta, cursor.get(), zoom)
            }
            _ => EventResult::Propagate,
        }
    });
    kp_out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_end_flushes_the_panels_file_too() {
        // Rule 36: a panel change still inside its debounce must be on the disk at logoff.
        let dir = tempfile::tempdir().unwrap();
        let long = Duration::from_secs(60);
        let tabs = TabsStore::new(dir.path().join("tabs.json"), long);
        let window = WindowStateStore::new(dir.path().join("window.json"), long);
        let prefs = PrefStore::memory();
        let file = dir.path().join("panels.json");
        let panels = Arc::new(flare_surfaces::store::PanelStore::new(&file, long));
        panels.save(&[flare_surfaces::store::SavedTabPanel {
            cwd: "C:/a".into(),
            panel: flare_surfaces::state::TabPanel::new(
                flare_surfaces::PanelKind::Text,
                "C:/a/n.txt",
            ),
        }]);
        assert!(!file.exists(), "inside the debounce nothing is written yet");
        flush_stores(&tabs, &window, &prefs, &panels);
        assert!(file.exists());
    }

    #[test]
    fn the_e2e_window_is_parked_when_its_handle_appears_not_before() {
        // No handle yet (the first looks after show()): wait, until the budget is spent.
        assert_eq!(poll_handle(0, 3, None), HandlePoll::Wait);
        assert_eq!(poll_handle(1, 3, None), HandlePoll::Wait);
        assert_eq!(poll_handle(2, 3, None), HandlePoll::GiveUp);
        // The handle shows up on any look: that look parks it.
        assert_eq!(poll_handle(0, 3, Some(77)), HandlePoll::Found(77));
        assert_eq!(poll_handle(2, 3, Some(77)), HandlePoll::Found(77));
        // The wait is long enough for a slow start and short enough to give up.
        assert!(E2E_HANDLE_POLL_MS * u64::from(E2E_HANDLE_POLLS) >= 5_000);
    }

    #[test]
    fn the_e2e_window_is_placed_off_screen_before_its_first_show() {
        assert_eq!(e2e_first_position(true), Some((-32000, -32000)));
        assert_eq!(e2e_first_position(false), None);
        let src = include_str!("window.rs").replace("\r\n", "\n");
        let set = src.find("e2e_first_position(args.e2e)").expect("used");
        let show = src
            .find("    app.show()?;\n    // The watchdog")
            .expect("show");
        assert!(set < show, "the position is set before show()");
    }

    fn h(cwd: &str) -> Handoff {
        Handoff {
            cwd: cwd.to_string(),
            args: vec![],
        }
    }

    #[test]
    fn a_handoff_before_the_controller_waits_and_arrives_in_order() {
        let sink = HandoffSink::default();
        sink.deliver(h("C:\\one"));
        sink.deliver(h("C:\\two"));
        let got: Arc<Mutex<Vec<String>>> = Arc::default();
        let post: Post = {
            let got = Arc::clone(&got);
            Arc::new(move |m| {
                if let UiMsg::Handoff(x) = m {
                    got.lock().unwrap().push(x.cwd);
                }
            })
        };
        assert!(got.lock().unwrap().is_empty());
        sink.attach(post);
        sink.deliver(h("C:\\three"));
        assert_eq!(*got.lock().unwrap(), ["C:\\one", "C:\\two", "C:\\three"]);
    }

    #[test]
    fn an_e2e_run_never_applies_a_saved_position() {
        let init = InitialWindow {
            pos: Some((120, 80)),
            logical_size: (900, 600),
            maximise: false,
        };
        assert_eq!(saved_position(&init, false), Some((120, 80)));
        assert_eq!(saved_position(&init, true), None);
    }

    /// A wheel scroll makes no output, so nothing else wakes the GPU path: the shared wheel asks for
    /// the redraw itself when the view moved (M10 Task 70, B5 counted no GPU frame at all before
    /// this). The winit arm and the e2e wheel both go through that one function (audit 2026-10-20).
    #[test]
    fn a_wheel_that_moves_the_view_asks_for_a_redraw() {
        let src = include_str!("window.rs").replace("\r\n", "\n");
        let from = src.find("pub(crate) fn wheel(").expect("the shared wheel");
        let body = &src[from..from + src[from..].find("\n    }\n").expect("end")];
        let call = body
            .find("if handle.wheel(notches, col, row) {")
            .expect("the wheel's answer is used");
        assert!(
            body[call..].contains("app.window().request_redraw();"),
            "the wheel asks for a redraw"
        );
        let arm = &src[src
            .find("WindowEvent::MouseWheel {")
            .expect("the filter has a wheel arm")..];
        let arm = &arm[..arm.find("_ => EventResult::Propagate").expect("end")];
        assert!(
            arm.contains("kp.wheel("),
            "the real arm uses the shared wheel"
        );
        let glue = include_str!("e2e_glue.rs").replace("\r\n", "\n");
        let from = glue.find("fn term_wheel(").expect("the e2e wheel");
        let body = &glue[from..from + glue[from..].find("\n    }\n").expect("end")];
        assert!(
            body.contains(".wheel(") && !body.contains("handle.wheel("),
            "the e2e wheel uses the shared wheel, not its own copy"
        );
    }

    #[test]
    fn the_winit_filter_calls_handle_key_and_nothing_else() {
        let src = include_str!("window.rs");
        let from = src
            .find("WindowEvent::KeyboardInput {")
            .expect("the filter has a keyboard arm");
        let rest = &src[from..];
        let to = rest.find("WindowEvent::Ime(").expect("the Ime arm follows");
        let arm = &rest[..to];
        for needed in ["user_acted", "key_event_from_parts", "handle_key"] {
            assert!(arm.contains(needed), "the arm lacks {needed}");
        }
        for banned in [".on_key(", ".on_key_up(", ".send_key(", "consumed"] {
            assert!(!arm.contains(banned), "the arm still has {banned}");
        }
    }

    #[test]
    fn the_saved_bounds_are_reapplied_only_at_startup() {
        let t0 = Instant::now();
        let mut r = Reapply::new(Some(5), t0);
        // A user action ends the startup window: a later monitor move must not snap back.
        r.user_acted();
        assert_eq!(r.take(t0), None);
        let mut r = Reapply::new(Some(5), t0);
        assert_eq!(r.take(t0 + REAPPLY_WINDOW + Duration::from_millis(1)), None);
        let mut r = Reapply::new(Some(5), t0);
        assert_eq!(r.take(t0 + Duration::from_millis(100)), Some(5));
        assert_eq!(r.take(t0 + Duration::from_millis(200)), None);
    }
    use flare_win::window_state::parse_window_state;

    const NO_AREAS: &[Area] = &[];

    #[test]
    fn overlay_names_the_fallback() {
        let text = overlay_text(
            RendererChoice::Software,
            Some("no adapter"),
            GridPainter::Cpu,
            "no frames",
            "WS 1.0 MB, private 2.0 MB",
        );
        assert!(text.starts_with("renderer: software (fallback: no adapter)\n"));
        assert!(text.contains("grid: cpu"));
        assert!(text.contains("frame: no frames"));
    }

    #[test]
    fn once_fires_a_single_time() {
        let mut once = Once::default();
        assert!(once.fire());
        assert!(!once.fire());
        assert!(!once.fire());
    }

    #[test]
    fn repaint_follows_dirty() {
        assert!(repaint_due(true));
        assert!(!repaint_due(false));
    }

    #[test]
    fn software_output_after_a_quiet_frame_is_painted_at_once() {
        assert!(paint_on_wake(Duration::from_millis(16), true));
        assert!(paint_on_wake(Duration::from_secs(5), true));
        assert!(!paint_on_wake(Duration::from_millis(15), true));
        assert!(!paint_on_wake(Duration::ZERO, true));
        assert!(!paint_on_wake(Duration::from_secs(5), false), "clean grid");
    }

    /// The moment (ms) output that arrives at `wake_at` is painted on the software path: at once when
    /// the last paint is a frame old, else at the poll timer's next tick (ticks at every multiple of
    /// 16 ms; a tick paints only a dirty grid, and `last_paint` is when the grid was last painted).
    fn painted_at(wake_at: u64, last_paint: u64) -> u64 {
        let frame = FRAME_POLL.as_millis() as u64;
        if paint_on_wake(Duration::from_millis(wake_at - last_paint), true) {
            wake_at
        } else {
            wake_at.div_ceil(frame) * frame
        }
    }

    #[test]
    fn software_output_is_painted_by_the_next_timer_tick() {
        // However the wake-up falls against the timer, output waits at most one frame: never the wake
        // and then a second 16 ms wait for the timer (the old path waited for the tick alone, and a
        // wake that landed just after a tick waited almost 16 ms for nothing else to happen).
        for last in [0u64, 7, 16, 100] {
            for wake in last..last + 200 {
                let wait = painted_at(wake, last) - wake;
                assert!(
                    wait < 16,
                    "wake at {wake}, last paint {last}: waited {wait} ms"
                );
            }
        }
        // A wake a frame after the last paint is painted in the same tick of the event loop.
        assert_eq!(painted_at(116, 100), 116);
        // One inside the frame waits for the tick, which is the soonest the pacing allows.
        assert_eq!(painted_at(105, 100), 112);
    }

    #[test]
    fn output_wakes_a_redraw_and_coalesces() {
        let gate = WakeGate::default();
        assert!(gate.try_queue(), "the first wake-up is queued");
        for _ in 0..1000 {
            assert!(!gate.try_queue(), "a wake-up is already on its way");
        }
        gate.delivered();
        assert!(
            gate.try_queue(),
            "after delivery the next output queues a new one"
        );
    }

    #[test]
    fn overlay_names_gpu() {
        let text = overlay_text(RendererChoice::Gpu, None, GridPainter::Gpu, "x", "y");
        assert!(text.starts_with("renderer: GPU (femtovg-wgpu)\ngrid: wgpu\n"));
    }

    #[test]
    fn the_initial_window_uses_the_saved_state() {
        let state = WindowState {
            width: 1400,
            height: 900,
            x: Some(100),
            y: Some(51),
            maximised: true,
            scale: 1.5,
        };
        let init = initial_window(&state, NO_AREAS);
        assert_eq!(init.pos, Some((100, 51)));
        assert_eq!(init.logical_size, (1400, 900));
        assert!(init.maximise);
    }

    #[test]
    fn the_initial_window_is_never_below_the_minimum() {
        let areas = [Area {
            x: 0,
            y: 0,
            width: 1920,
            height: 1040,
        }];
        let state = parse_window_state(
            r#"{"width":10,"height":10,"x":0,"y":0,"maximised":false}"#,
            &areas,
        );
        assert_eq!(initial_window(&state, &areas).logical_size, (560, 400));
        // A state that did not come through the parser is held to the floor as well.
        let raw = WindowState {
            width: 10,
            height: 10,
            x: None,
            y: None,
            maximised: false,
            scale: 1.0,
        };
        assert_eq!(initial_window(&raw, NO_AREAS).logical_size, (560, 400));
    }

    #[test]
    fn a_position_off_every_monitor_is_dropped() {
        let areas = [Area {
            x: 0,
            y: 0,
            width: 1920,
            height: 1040,
        }];
        let state = WindowState {
            width: 800,
            height: 600,
            x: Some(5000),
            y: Some(5000),
            maximised: false,
            scale: 1.0,
        };
        assert_eq!(initial_window(&state, &areas).pos, None);
    }

    #[cfg(windows)]
    #[test]
    fn e2e_style_is_no_activate_tool_window_and_not_app_window() {
        let style = e2e_ex_style(WS_EX_APPWINDOW);
        assert_ne!(style & WS_EX_NOACTIVATE, 0);
        assert_ne!(style & WS_EX_TOOLWINDOW, 0);
        assert_eq!(style & WS_EX_APPWINDOW, 0);
    }

    #[test]
    fn outcomes_map_to_filter_actions() {
        assert_eq!(
            key_outcome_to_filter(KeyOutcome::Consumed),
            (FilterAction::PreventDefault, false)
        );
        assert_eq!(
            key_outcome_to_filter(KeyOutcome::Pty),
            (FilterAction::PreventDefault, true)
        );
        assert_eq!(
            key_outcome_to_filter(KeyOutcome::Slint),
            (FilterAction::Propagate, false)
        );
    }
}
