//! The dictation widget window: the second window that shows for dictate-from-any-app (M8 Task 26).
//!
//! PT's widget (`overlay.ts`, `overlay.css`, `dictateAnywhere.ts` ensure/show/heightFor) is a frameless, transparent,
//! never-focusable window above every window. Flare's is a second Slint window (`ui/dictation_widget.slint`) with the
//! same rules (N18, N19, N20, N23, N32):
//!
//! - Created on first use, never before; shown with `SW_SHOWNOACTIVATE` at the work area's bottom centre, 16 css px up,
//!   on the monitor under the pointer, 480 css px wide at THAT monitor's scale; hidden with `SW_HIDE` (never closed, so
//!   the event loop never sees its last window go).
//! - Never takes focus: the winit attributes ask for an inactive, undecorated, taskbar-less, always-on-top window parked
//!   off every monitor, and the first thing done with the handle (which only exists once the event loop has made the real window, so it is fetched lazily) is `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW |
//!   WS_EX_TOPMOST` (and no `WS_EX_APPWINDOW`). Clicks pass through it (`WS_EX_TRANSPARENT | WS_EX_LAYERED`, set here and not through winit's `set_cursor_hittest`,
//!   whose style rewrite drops `WS_EX_NOACTIVATE` and `WS_EX_TOOLWINDOW`) except while it is picking.
//! - Under `--e2e` it is parked like the main window (M10 D4): it never leaves -32000,-32000, but the bounds it WOULD
//!   have are computed and readable through [`snapshot`].
//! - The faces wear the inks the main window hands over at each outside start (N32), not a theme of their own.
//!
//! **[seam]** The plan names `Pick(id)`; the click reaches the controller through `dictation_ctl::widget_pick`.
//! "Visible on every virtual desktop" (PT `setVisibleOnAllWorkspaces`) is a no-op in Electron on Windows too, and
//! Windows offers no documented way to pin a window to all desktops, so it is not done here (listed in the report).

// Task 27's probe reads `snapshot`; the card's words live in the .slint file and are held here for the test that keeps
// them the same (ui_tests/dictation_widget.rs).
#![allow(dead_code)]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use flare_dictation::route::OverlayRow;
use flare_dictation::service::{OverlayState, Tone, WIDGET_WIDTH};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::winit_030::WinitWindowAccessor;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{DictationWidget, Motion, WidgetItem};

// ---------------------------------------------------------------------------------------------------------------------
// The words and numbers of PT's widget
// ---------------------------------------------------------------------------------------------------------------------

/// The card's words (PT `overlay.ts`).
pub mod texts {
    pub const LISTENING: &str = "Listening";
    pub const TRANSCRIBING: &str = "Transcribing";
    pub const PICK_TITLE: &str = "Send to";
    /// Shown under the live words' place while listening with no words yet and the engine still starting.
    pub const STARTING: &str = "Starting the speech engine";
    /// Shown when listening with no words and the engine ready.
    pub const SPEAK_NOW: &str = "Speak now";
    pub const LISTENING_HINT: &str = "Let go to finish. Esc cancels.";
    pub const PICK_HINT: &str = "Press a number or click. Esc keeps it unsent.";
}

/// The widget is 480 css px wide (N18).
pub const WIDTH: u32 = WIDGET_WIDTH;
/// It sits this many css px above the work area's foot (N18).
pub const LIFT: u32 = 16;

/// The level bar's scale: `max(0.04, min(1, level * 3))` (N19).
pub fn bar_scale(level: f32) -> f32 {
    if !level.is_finite() {
        return 0.04;
    }
    (level * 3.0).clamp(0.04, 1.0)
}

/// One line of the picker, as the card draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// A group's header (its name is in `label`), not a session.
    pub header: bool,
    pub id: String,
    pub n: u8,
    pub label: String,
    pub folder: String,
    /// "working", "asking" or "orchestrator", or empty.
    pub chip: String,
    /// The row's title: why it cannot be picked. Empty for a row that can.
    pub reason: String,
    pub disabled: bool,
}

/// The rows of a picker with a header before each new group (PT: `if (r.group && r.group !== group)`).
pub fn items_of(rows: &[OverlayRow]) -> Vec<Item> {
    let mut out = Vec::with_capacity(rows.len() + 2);
    let mut group: Option<&str> = None;
    for r in rows {
        if let Some(g) = r.group.as_deref()
            && group != Some(g)
        {
            out.push(Item {
                header: true,
                id: String::new(),
                n: 0,
                label: g.to_string(),
                folder: String::new(),
                chip: String::new(),
                reason: String::new(),
                disabled: false,
            });
        }
        group = r.group.as_deref();
        let chip = if r.lead {
            "orchestrator"
        } else if r.disabled.is_some() {
            "asking"
        } else if r.state.as_deref() == Some("working") {
            "working"
        } else {
            ""
        };
        out.push(Item {
            header: false,
            id: r.id.clone(),
            n: r.n,
            label: r.label.clone(),
            folder: r.folder.clone(),
            chip: chip.to_string(),
            reason: r.disabled.clone().unwrap_or_default(),
            disabled: r.disabled.is_some(),
        });
    }
    out
}

/// A face, as the card is set to draw it.
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    /// 1 listening, 2 transcribing, 3 pick, 4 sending, 5 sent, 6 note.
    pub phase: i32,
    pub title: String,
    pub starting: bool,
    /// The live words (listening) or the heard words (pick).
    pub text: String,
    pub bar: f32,
    pub warn: bool,
    pub items: Vec<Item>,
    /// The window's height in css px (N18).
    pub height: u32,
}

/// The face of a service state.
pub fn face_of(state: &OverlayState) -> Face {
    let mut f = Face {
        phase: 0,
        title: String::new(),
        starting: false,
        text: String::new(),
        bar: 0.04,
        warn: false,
        items: Vec::new(),
        height: state.height(),
    };
    match state {
        OverlayState::Listening {
            text,
            level,
            starting,
        } => {
            f.phase = 1;
            f.title = texts::LISTENING.into();
            f.text = text.clone();
            f.bar = bar_scale(*level);
            f.starting = *starting;
        }
        OverlayState::Transcribing => {
            f.phase = 2;
            f.title = texts::TRANSCRIBING.into();
        }
        OverlayState::Pick { text, rows } => {
            f.phase = 3;
            f.title = texts::PICK_TITLE.into();
            f.text = text.clone();
            f.items = items_of(rows);
        }
        OverlayState::Sending { to } => {
            f.phase = 4;
            f.title = format!("Sending to {to}");
        }
        OverlayState::Sent { to } => {
            f.phase = 5;
            f.title = format!("Sent to {to}");
        }
        OverlayState::Note { message, tone } => {
            f.phase = 6;
            f.title = message.clone();
            f.warn = *tone == Tone::Warn;
        }
    }
    f
}

/// The words the listening card shows: the live words, else "Starting the speech engine" or "Speak now" (N19).
pub fn live_line(text: &str, starting: bool) -> &str {
    if !text.is_empty() {
        text
    } else if starting {
        texts::STARTING
    } else {
        texts::SPEAK_NOW
    }
}

/// Whether showing `next` hands the card the inks of the theme in front (N32): at each outside start, which is the
/// first listening face after anything else, and whenever the card was hidden.
pub fn wants_inks(was_shown: bool, prev: Option<&Face>, next: &Face) -> bool {
    !was_shown || (next.phase == 1 && prev.is_none_or(|p| p.phase != 1))
}

// ---------------------------------------------------------------------------------------------------------------------
// Where it sits
// ---------------------------------------------------------------------------------------------------------------------

/// A rectangle in device pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bounds {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// The widget's rectangle on a monitor: `work` is the monitor's work area, `scale` its scaling, `css_h` the face's
/// height. Centred, `LIFT` css px above the work area's foot (N18).
pub fn bounds_for(work: Bounds, scale: f64, css_h: u32) -> Bounds {
    let w = (f64::from(WIDTH) * scale).round() as i32;
    let h = (f64::from(css_h) * scale).round() as i32;
    let lift = (f64::from(LIFT) * scale).round() as i32;
    let x = work.x + (f64::from(work.w - w) / 2.0).round() as i32;
    let y = work.y + work.h - h - lift;
    Bounds { x, y, w, h }
}

/// The monitor under the pointer: its work area and scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Display {
    pub work: Bounds,
    pub scale: f64,
}

// ---------------------------------------------------------------------------------------------------------------------
// The inks
// ---------------------------------------------------------------------------------------------------------------------

/// The theme in front, as the card wears it (PT `look`: `--p-tabs`, `--p-text`, `--p-dim`, `--p-accent-hi`,
/// `--p-divider`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Inks {
    pub ground: slint::Color,
    pub ink: slint::Color,
    pub dim: slint::Color,
    pub accent: slint::Color,
    pub edge: slint::Color,
}

/// The inks of the main window's theme now.
pub fn inks_of(app: &crate::AppWindow) -> Inks {
    let t = app.global::<crate::Theme>();
    Inks {
        ground: t.get_tabs(),
        ink: t.get_text(),
        dim: t.get_dim(),
        accent: t.get_accent_hi(),
        edge: t.get_divider(),
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// The window
// ---------------------------------------------------------------------------------------------------------------------

/// True while the widget's window is being made, so the one attributes hook knows whose attributes it is given.
static CREATING: AtomicBool = AtomicBool::new(false);
/// Whether the window surface can carry per-pixel alpha (the glass plan): else the card sits on a flat ground.
static ALPHA_SURFACE: AtomicBool = AtomicBool::new(false);

/// `backend.rs` tells the widget whether the surface the renderer makes has alpha.
pub fn set_alpha_surface(on: bool) {
    ALPHA_SURFACE.store(on, Ordering::Relaxed);
}

/// Whether the window being created is the widget (the attributes hook asks).
pub fn creating() -> bool {
    CREATING.load(Ordering::Relaxed)
}

/// The widget's winit attributes: undecorated, transparent, inactive, off the taskbar, always on top, born parked.
pub fn attributes(
    a: slint::winit_030::winit::window::WindowAttributes,
) -> slint::winit_030::winit::window::WindowAttributes {
    use slint::winit_030::winit::platform::windows::WindowAttributesExtWindows;
    use slint::winit_030::winit::window::WindowLevel;
    let (x, y) = flare_win::window_park::parked_origin();
    a.with_decorations(false)
        .with_transparent(true)
        .with_active(false)
        .with_skip_taskbar(true)
        .with_resizable(false)
        .with_undecorated_shadow(false)
        .with_window_level(WindowLevel::AlwaysOnTop)
        .with_position(slint::winit_030::winit::dpi::PhysicalPosition::new(x, y))
}

/// The Win32 half: styles, places and shows a window without ever activating it.
pub mod win {
    use super::{Bounds, Display};
    use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
    };
    use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetCursorPos, GetWindowLongPtrW, HWND_TOPMOST, IsWindowVisible, SW_HIDE,
        SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOOWNERZORDER, SetWindowLongPtrW, SetWindowPos,
        ShowWindow, WS_EX_APPWINDOW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
        WS_EX_TOPMOST, WS_EX_TRANSPARENT,
    };

    fn raw(hwnd: isize) -> HWND {
        hwnd as HWND
    }

    /// The style bits the widget needs: it can never be activated, is off the taskbar and above every window.
    pub fn widget_ex_style(current: u32) -> u32 {
        (current | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST) & !WS_EX_APPWINDOW
    }

    /// Gives the window the widget's style. Call it on a window just made, before it is shown anywhere real.
    pub fn prepare(hwnd: isize) {
        // SAFETY: `hwnd` is a window of this process on the UI thread; reading and writing its extended style
        // touches no memory of ours, and a stale handle just fails.
        unsafe {
            let cur = GetWindowLongPtrW(raw(hwnd), GWL_EXSTYLE) as u32;
            SetWindowLongPtrW(raw(hwnd), GWL_EXSTYLE, widget_ex_style(cur) as isize);
        }
    }

    /// Lets clicks fall through the window (pass) or catches them. The same bits winit's set_cursor_hittest sets, but
    /// set here so winit never rewrites the whole extended style (which would drop the never-activate bits).
    pub fn set_click_through(hwnd: isize, pass: bool) {
        const BITS: u32 = WS_EX_TRANSPARENT | WS_EX_LAYERED;
        // SAFETY: as prepare.
        unsafe {
            let cur = GetWindowLongPtrW(raw(hwnd), GWL_EXSTYLE) as u32;
            let next = if pass { cur | BITS } else { cur & !BITS };
            SetWindowLongPtrW(raw(hwnd), GWL_EXSTYLE, next as isize);
        }
    }

    /// Whether the window has every bit of the widget's style.
    pub fn is_prepared(hwnd: isize) -> bool {
        let cur = ex_style(hwnd);
        widget_ex_style(cur) == cur
    }

    /// Moves and sizes the window, above every window, without activating it.
    pub fn place(hwnd: isize, b: Bounds) {
        // SAFETY: a plain window-manager call on a window of this process; no activation (SWP_NOACTIVATE).
        unsafe {
            SetWindowPos(
                raw(hwnd),
                HWND_TOPMOST,
                b.x,
                b.y,
                b.w,
                b.h,
                SWP_NOACTIVATE | SWP_NOOWNERZORDER,
            );
        }
    }

    /// Shows the window without activating it and puts it on top again.
    pub fn show(hwnd: isize) {
        // SAFETY: as `place`; SW_SHOWNOACTIVATE shows the window in its place and leaves the foreground alone.
        unsafe {
            ShowWindow(raw(hwnd), SW_SHOWNOACTIVATE);
            SetWindowPos(
                raw(hwnd),
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE
                    | SWP_NOOWNERZORDER
                    | windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOMOVE
                    | windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOSIZE,
            );
        }
    }

    pub fn hide(hwnd: isize) {
        // SAFETY: as `place`.
        unsafe {
            ShowWindow(raw(hwnd), SW_HIDE);
        }
    }

    pub fn ex_style(hwnd: isize) -> u32 {
        // SAFETY: reads a window's extended style; a stale handle answers 0.
        unsafe { GetWindowLongPtrW(raw(hwnd), GWL_EXSTYLE) as u32 }
    }

    pub fn is_visible(hwnd: isize) -> bool {
        // SAFETY: reads a window's visibility; a stale handle answers false.
        unsafe { IsWindowVisible(raw(hwnd)) != 0 }
    }

    /// The monitor under the pointer: its work area in device pixels and its scale. None when Windows will not say.
    pub fn display_under_cursor() -> Option<Display> {
        let mut pt = POINT { x: 0, y: 0 };
        // SAFETY: `pt` is a valid POINT for the call to fill.
        if unsafe { GetCursorPos(&mut pt) } == 0 {
            return None;
        }
        // SAFETY: MonitorFromPoint takes the point by value and answers a monitor handle (never null with NEAREST).
        let hmon = unsafe { MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST) };
        if hmon.is_null() {
            return None;
        }
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            rcMonitor: RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            rcWork: RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            dwFlags: 0,
        };
        // SAFETY: `info` has cbSize set as the call requires; `hmon` was just returned by MonitorFromPoint.
        if unsafe { GetMonitorInfoW(hmon, &mut info) } == 0 {
            return None;
        }
        let (mut dx, mut dy) = (0u32, 0u32);
        // SAFETY: `hmon` is valid and both out-pointers are valid u32s.
        let hr = unsafe { GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) };
        let scale = if hr == 0 && dx > 0 {
            f64::from(dx) / 96.0
        } else {
            1.0
        };
        let r = info.rcWork;
        Some(Display {
            work: Bounds {
                x: r.left,
                y: r.top,
                w: r.right - r.left,
                h: r.bottom - r.top,
            },
            scale,
        })
    }
}

/// What the probe reads (Task 27): the widget as it is, without drawing anything.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub exists: bool,
    /// The widget is meant to be showing (under `--e2e` it is shown parked, off every monitor).
    pub visible: bool,
    /// Whether the window can be activated: false always.
    pub focusable: bool,
    pub topmost: bool,
    /// 0 when nothing shows, else the face (1 listening, 2 transcribing, 3 pick, 4 sending, 5 sent, 6 note).
    pub phase: i32,
    /// The card's head line.
    pub title: String,
    /// The live words or the heard words.
    pub text: String,
    /// The picker's lines, headers included.
    pub items: Vec<Item>,
    /// Where it would be on a real run, in device pixels.
    pub bounds: Bounds,
    /// Where it is now.
    pub parked: bool,
    /// The face's height in css px.
    pub height: u32,
}

struct Widget {
    comp: DictationWidget,
    hwnd: Option<isize>,
    shown: bool,
    face: Option<Face>,
    bounds: Bounds,
    e2e: bool,
    /// Whether clicks should pass through (wanted) and whether the window has been told so (applied).
    through: bool,
    applied_through: Option<bool>,
    /// The monitor chosen at the last show: a height change keeps the card there (Appendix E).
    display: Option<Display>,
    model: Rc<VecModel<WidgetItem>>,
    rise: slint::Timer,
    settle: slint::Timer,
    /// Polls for the real window until the event loop has made it.
    attach: slint::Timer,
}

thread_local! {
    static WIDGET: RefCell<Option<Widget>> = const { RefCell::new(None) };
}

/// Where the widget gets its window handle and its monitor from. Production reads winit and Windows; the tests put
/// their own in (a window that appears only after a few polls, a pointer that moves between monitors).
#[cfg(test)]
type HandleSource = Rc<dyn Fn(&slint::Window) -> Option<isize>>;

#[derive(Default)]
struct Sources {
    #[cfg(test)]
    handle: Option<HandleSource>,
    #[cfg(test)]
    display: Option<Rc<dyn Fn() -> Option<Display>>>,
}

thread_local! {
    static SOURCES: RefCell<Sources> = RefCell::new(Sources::default());
}

#[cfg(test)]
pub fn test_sources(
    handle: impl Fn(&slint::Window) -> Option<isize> + 'static,
    display: impl Fn() -> Option<Display> + 'static,
) {
    SOURCES.with(|s| {
        *s.borrow_mut() = Sources {
            handle: Some(Rc::new(handle)),
            display: Some(Rc::new(display)),
        }
    });
}

fn source_handle(w: &slint::Window) -> Option<isize> {
    #[cfg(test)]
    if let Some(f) = SOURCES.with(|s| s.borrow().handle.clone()) {
        return f(w);
    }
    hwnd_of(w)
}

fn source_display() -> Option<Display> {
    #[cfg(test)]
    if let Some(f) = SOURCES.with(|s| s.borrow().display.clone()) {
        return f();
    }
    win::display_under_cursor()
}

fn hwnd_of(w: &slint::Window) -> Option<isize> {
    w.with_winit_window(|w| match w.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    })
    .flatten()
}

fn to_item(i: &Item) -> WidgetItem {
    WidgetItem {
        header: i.header,
        id: SharedString::from(i.id.as_str()),
        n: i32::from(i.n),
        label: SharedString::from(i.label.as_str()),
        folder: SharedString::from(i.folder.as_str()),
        chip: SharedString::from(i.chip.as_str()),
        reason: SharedString::from(i.reason.as_str()),
        disabled: i.disabled,
    }
}

/// Sets every property of the card to a face (the whole card, not a level update).
pub fn set_face(comp: &DictationWidget, model: &VecModel<WidgetItem>, f: &Face) {
    comp.set_phase(f.phase);
    comp.set_head(SharedString::from(f.title.as_str()));
    comp.set_starting(f.starting);
    comp.set_words(SharedString::from(f.text.as_str()));
    comp.set_bar(f.bar);
    comp.set_warn(f.warn);
    model.set_vec(f.items.iter().map(to_item).collect::<Vec<_>>());
    comp.set_face_h(f.height as f32);
}

impl Widget {
    fn make(e2e: bool) -> Option<Widget> {
        CREATING.store(true, Ordering::Relaxed);
        let made = DictationWidget::new();
        let comp = match made {
            Ok(c) => c,
            Err(e) => {
                CREATING.store(false, Ordering::Relaxed);
                log::warn!("dictation widget: cannot make the window: {e}");
                return None;
            }
        };
        comp.global::<Motion>()
            .set_reduced(!crate::e2e::animations_enabled());
        comp.set_flat(!ALPHA_SURFACE.load(Ordering::Relaxed));
        let model = Rc::new(VecModel::<WidgetItem>::default());
        comp.set_items(ModelRc::from(model.clone()));
        // untimed: a row click only posts one event-loop call and returns at once
        comp.on_pick(|id| crate::dictation_ctl::widget_pick(id.to_string()));
        let shown = comp.show();
        CREATING.store(false, Ordering::Relaxed);
        if let Err(e) = shown {
            log::warn!("dictation widget: cannot show the window: {e}");
            return None;
        }
        Some(Widget {
            comp,
            hwnd: None,
            shown: false,
            face: None,
            bounds: Bounds::default(),
            e2e,
            through: true,
            applied_through: None,
            display: None,
            model,
            rise: slint::Timer::default(),
            settle: slint::Timer::default(),
            attach: slint::Timer::default(),
        })
    }

    /// Takes the window's handle once the event loop has made the window (show() only registers it). Styles it, tells
    /// it whether clicks pass, and hides it or shows it as the card is meant to be. True once it has a handle.
    fn attach(&mut self) -> bool {
        if self.hwnd.is_none() {
            self.hwnd = source_handle(self.comp.window());
        }
        let Some(h) = self.hwnd else { return false };
        self.attach.stop();
        win::prepare(h);
        self.sync_through();
        if self.shown {
            self.move_to_bounds();
            self.show_now();
        } else {
            win::hide(h);
        }
        true
    }

    /// Keeps trying to attach on a short timer, for a while.
    fn start_attach_timer(&mut self) {
        if self.attach.running() {
            return;
        }
        let tries = std::cell::Cell::new(0u32);
        self.attach.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(10),
            move || {
                tries.set(tries.get() + 1);
                WIDGET.with(|cell| {
                    let mut slot = cell.borrow_mut();
                    let Some(w) = slot.as_mut() else { return };
                    if w.attach() {
                        return;
                    }
                    if tries.get() > 400 {
                        w.attach.stop();
                        log::warn!("dictation widget: the window has no handle");
                    }
                });
            },
        );
    }

    /// Tells the window whether clicks pass, when it has a handle and has not been told yet.
    fn sync_through(&mut self) {
        let Some(h) = self.hwnd else { return };
        if self.applied_through != Some(self.through) {
            win::set_click_through(h, self.through);
            self.applied_through = Some(self.through);
        }
    }

    /// Puts the never-activate style back if anything took it off (cheap, run on every update).
    fn keep_style(&self) {
        if let Some(h) = self.hwnd
            && !win::is_prepared(h)
        {
            win::prepare(h);
        }
    }
    fn apply_inks(&self, i: Inks) {
        self.comp.set_ground(i.ground);
        self.comp.set_ink(i.ink);
        self.comp.set_dim(i.dim);
        self.comp.set_accent(i.accent);
        self.comp.set_edge(i.edge);
    }

    /// Sets the card to a face. A level or words update on the listening card changes those properties only.
    fn draw(&mut self, f: &Face) {
        let same_listening = self.shown
            && self
                .face
                .as_ref()
                .is_some_and(|p| p.phase == 1 && f.phase == 1);
        if same_listening {
            let p = self.face.as_ref();
            if p.is_none_or(|p| p.text != f.text) {
                self.comp.set_words(SharedString::from(f.text.as_str()));
            }
            if p.is_none_or(|p| p.starting != f.starting) {
                self.comp.set_starting(f.starting);
            }
            self.comp.set_bar(f.bar);
            self.face = Some(f.clone());
            return;
        }
        set_face(&self.comp, &self.model, f);
        self.face = Some(f.clone());
    }

    /// Computes where the widget goes on the monitor chosen at show and moves it there (parked under --e2e).
    fn place(&mut self, height: u32) {
        let d = self.display.unwrap_or(Display {
            work: Bounds {
                x: 0,
                y: 0,
                w: 1920,
                h: 1040,
            },
            scale: 1.0,
        });
        self.bounds = bounds_for(d.work, d.scale, height);
        self.move_to_bounds();
        if self.shown {
            self.start_settle();
        }
    }
    fn move_to_bounds(&self) {
        let Some(h) = self.hwnd else { return };
        let mut b = self.bounds;
        if self.e2e {
            let (px, py) = flare_win::window_park::parked_origin();
            b.x = px;
            b.y = py;
        }
        win::place(h, b);
    }

    fn click_through(&mut self, pass: bool) {
        self.through = pass;
        self.sync_through();
    }
    fn show_now(&mut self) {
        let Some(h) = self.hwnd else { return };
        win::place(h, self.bounds_applied());
        win::show(h);
        self.start_settle();
    }

    /// A DPI change on the way onto another monitor can make winit move the window to its own suggestion: put it back a
    /// moment later, where it was meant to go.
    fn start_settle(&self) {
        let Some(hw) = self.hwnd else { return };
        let target = self.bounds_applied();
        self.settle.start(
            slint::TimerMode::SingleShot,
            std::time::Duration::from_millis(60),
            move || {
                win::place(hw, target);
            },
        );
    }
    fn bounds_applied(&self) -> Bounds {
        if self.e2e {
            let (px, py) = flare_win::window_park::parked_origin();
            Bounds {
                x: px,
                y: py,
                ..self.bounds
            }
        } else {
            self.bounds
        }
    }
}

/// Shows `face` (or hides the widget for `None`). `inks` is asked only at an outside start (N32). UI thread only.
pub fn set(face: Option<&OverlayState>, inks: &dyn Fn() -> Option<Inks>, e2e: bool) {
    let Some(state) = face else {
        hide();
        return;
    };
    let f = face_of(state);
    WIDGET.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = Widget::make(e2e);
        }
        let Some(w) = slot.as_mut() else { return };
        let was_shown = w.shown;
        if wants_inks(was_shown, w.face.as_ref(), &f)
            && let Some(i) = inks()
        {
            w.apply_inks(i);
        }
        let height_changed = w.face.as_ref().is_none_or(|p| p.height != f.height);
        if w.hwnd.is_none() {
            w.attach();
        } else {
            w.keep_style();
        }
        w.draw(&f);
        w.click_through(f.phase != 3);
        if !was_shown {
            w.display = source_display();
        }
        if !was_shown || height_changed {
            w.place(f.height);
        }
        if !was_shown {
            w.shown = true;
            w.comp.set_risen(false);
            w.show_now();
            let weak = w.comp.as_weak();
            w.rise.start(
                slint::TimerMode::SingleShot,
                std::time::Duration::from_millis(16),
                move || {
                    if let Some(c) = weak.upgrade() {
                        c.set_risen(true);
                    }
                },
            );
        }
        if w.hwnd.is_none() {
            w.start_attach_timer();
        }
    });
}

/// Hides the widget (it is kept for the next show).
pub fn hide() {
    WIDGET.with(|cell| {
        let mut slot = cell.borrow_mut();
        let Some(w) = slot.as_mut() else { return };
        if !w.shown {
            return;
        }
        w.shown = false;
        w.rise.stop();
        w.settle.stop();
        w.display = None;
        w.face = None;
        w.comp.set_phase(0);
        if let Some(h) = w.hwnd {
            win::hide(h);
        }
    });
}

/// Quit: the widget goes with the app.
pub fn close() {
    WIDGET.with(|cell| {
        if let Some(w) = cell.borrow_mut().take() {
            w.attach.stop();
            if let Some(h) = w.hwnd {
                win::hide(h);
            }
            let _ = w.comp.hide();
        }
    });
}

/// The widget as it is now (Task 27's `dict.widget`).
pub fn snapshot() -> Snapshot {
    WIDGET.with(|cell| {
        let slot = cell.borrow();
        let Some(w) = slot.as_ref() else {
            return Snapshot::default();
        };
        let ex = w.hwnd.map_or(0, win::ex_style);
        let f = w.face.as_ref();
        Snapshot {
            exists: true,
            visible: w.shown && w.hwnd.is_some_and(win::is_visible),
            focusable: ex & windows_sys::Win32::UI::WindowsAndMessaging::WS_EX_NOACTIVATE == 0,
            topmost: ex & windows_sys::Win32::UI::WindowsAndMessaging::WS_EX_TOPMOST != 0,
            phase: f.map_or(0, |f| f.phase),
            title: f.map(|f| f.title.clone()).unwrap_or_default(),
            text: f.map(|f| f.text.clone()).unwrap_or_default(),
            items: f.map(|f| f.items.clone()).unwrap_or_default(),
            bounds: w.bounds,
            parked: w.e2e,
            height: f.map_or(0, |f| f.height),
        }
    })
}
