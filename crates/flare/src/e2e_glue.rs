//! The UI-thread side of the test hook: the one dispatcher for every request the pipe server hands
//! over (Tasks 8 to 11 fill its arms), and the render hooks the window's notifier calls (Appendix F).
//! Nothing here does anything unless `--e2e` is on.
//!
//! Task 8 owns the input arms: `Key`, `KeyPress`, `Text`, `Pointer`, `Drag`, `DropFiles`,
//! `Resize`, `Focus`, `Quit`, `BlockUi`, `Slow`, `PanicWorker`, `Window`, `ClipSet`.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use flare_e2e_proto::{Button, PointerAction, Reply, Request};
use flare_term::keys::KeyEvent;
use flare_win::clipboard_port::ClipboardPort;
use slint::platform::{PointerEventButton, WindowEvent};
use slint::winit_030::EventResult;
use slint::{ComponentHandle, LogicalPosition, SharedString};

use crate::AppWindow;
use crate::appctl::{AppCtl, CloseDecision};
use crate::config::Args;
use crate::e2e_hook::Responder;
use crate::window::KeyPath;

/// Which side of a rendered frame the caller is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderPhase {
    Before,
    After,
}

/// Called by the render path around every frame. Returns at once unless `--e2e` is on (Task 1
/// pre-wires the call; Task 10 adds the snapshot and frame bookkeeping).
pub fn on_render(phase: RenderPhase) {
    if !crate::e2e::global().on() {
        return;
    }
    let _ = phase;
}

thread_local! {
    /// The running hook server; dropped (and its pipe closed) with the UI thread.
    static HOOK: RefCell<Option<crate::e2e_hook::HookServer>> = const { RefCell::new(None) };
}

// ---- the chord grammar ---------------------------------------------------------------------

/// The W3C code of a printable character, and whether typing it needs Shift.
fn code_of_char(c: char) -> Option<(String, bool)> {
    let shifted_digit = |d: u32| format!("Digit{d}");
    Some(match c {
        'a'..='z' => (format!("Key{}", c.to_ascii_uppercase()), false),
        'A'..='Z' => (format!("Key{c}"), true),
        '0'..='9' => (format!("Digit{c}"), false),
        ',' => ("Comma".into(), false),
        '.' => ("Period".into(), false),
        '/' => ("Slash".into(), false),
        ';' => ("Semicolon".into(), false),
        '\'' => ("Quote".into(), false),
        '[' => ("BracketLeft".into(), false),
        ']' => ("BracketRight".into(), false),
        '\\' => ("Backslash".into(), false),
        '-' => ("Minus".into(), false),
        '=' => ("Equal".into(), false),
        '`' => ("Backquote".into(), false),
        ' ' => ("Space".into(), false),
        '!' => (shifted_digit(1), true),
        '@' => (shifted_digit(2), true),
        '#' => (shifted_digit(3), true),
        '$' => (shifted_digit(4), true),
        '%' => (shifted_digit(5), true),
        '^' => (shifted_digit(6), true),
        '&' => (shifted_digit(7), true),
        '*' => (shifted_digit(8), true),
        '(' => (shifted_digit(9), true),
        ')' => (shifted_digit(0), true),
        '?' => ("Slash".into(), true),
        ':' => ("Semicolon".into(), true),
        '"' => ("Quote".into(), true),
        '<' => ("Comma".into(), true),
        '>' => ("Period".into(), true),
        '_' => ("Minus".into(), true),
        '+' => ("Equal".into(), true),
        '{' => ("BracketLeft".into(), true),
        '}' => ("BracketRight".into(), true),
        '|' => ("Backslash".into(), true),
        '~' => ("Backquote".into(), true),
        _ => return None,
    })
}

/// True when `t` names a W3C code this grammar accepts.
fn is_code(t: &str) -> bool {
    const NAMED: [&str; 28] = [
        "Enter",
        "Tab",
        "Space",
        "Escape",
        "Backspace",
        "Delete",
        "Insert",
        "ArrowUp",
        "ArrowDown",
        "ArrowLeft",
        "ArrowRight",
        "PageUp",
        "PageDown",
        "Home",
        "End",
        "Comma",
        "Period",
        "Slash",
        "Semicolon",
        "Quote",
        "BracketLeft",
        "BracketRight",
        "Backslash",
        "Minus",
        "Equal",
        "Backquote",
        "NumpadEnter",
        "CapsLock",
    ];
    if NAMED.contains(&t) {
        return true;
    }
    if let Some(l) = t.strip_prefix("Key") {
        return l.len() == 1 && l.chars().all(|c| c.is_ascii_uppercase());
    }
    if let Some(d) = t.strip_prefix("Digit") {
        return d.len() == 1 && d.chars().all(|c| c.is_ascii_digit());
    }
    if let Some(n) = t.strip_prefix('F') {
        return n.parse::<u32>().is_ok_and(|n| (1..=24).contains(&n));
    }
    if let Some(n) = t.strip_prefix("Numpad") {
        return n.len() == 1 && n.chars().all(|c| c.is_ascii_digit())
            || ["Add", "Subtract", "Multiply", "Divide", "Decimal"].contains(&n);
    }
    false
}

/// The logical key name of a code (what a US layout would report), `shift` applied.
fn logical_of_code(code: &str, shift: bool) -> String {
    if let Some(l) = code.strip_prefix("Key") {
        return if shift {
            l.to_string()
        } else {
            l.to_lowercase()
        };
    }
    if let Some(d) = code.strip_prefix("Digit") {
        let d = d.chars().next().unwrap_or('0');
        return if shift {
            ")!@#$%^&*("
                .chars()
                .nth(d as usize - '0' as usize)
                .unwrap_or(d)
        } else {
            d
        }
        .to_string();
    }
    if let Some(n) = code.strip_prefix("Numpad") {
        return match n {
            "Enter" => "Enter".into(),
            "Add" => "+".into(),
            "Subtract" => "-".into(),
            "Multiply" => "*".into(),
            "Divide" => "/".into(),
            "Decimal" => ".".into(),
            d => d.into(),
        };
    }
    let pair = |plain: &str, shifted: &str| if shift { shifted } else { plain }.to_string();
    match code {
        "Space" => " ".into(),
        "Comma" => pair(",", "<"),
        "Period" => pair(".", ">"),
        "Slash" => pair("/", "?"),
        "Semicolon" => pair(";", ":"),
        "Quote" => pair("'", "\""),
        "BracketLeft" => pair("[", "{"),
        "BracketRight" => pair("]", "}"),
        "Backslash" => pair("\\", "|"),
        "Minus" => pair("-", "_"),
        "Equal" => pair("=", "+"),
        "Backquote" => pair("`", "~"),
        other => other.to_string(),
    }
}

/// True for a code that types a printable character.
fn is_printable_code(code: &str) -> bool {
    code.starts_with("Key")
        || code.starts_with("Digit")
        || code == "Space"
        || (code.starts_with("Numpad") && code != "NumpadEnter")
        || [
            "Comma",
            "Period",
            "Slash",
            "Semicolon",
            "Quote",
            "BracketLeft",
            "BracketRight",
            "Backslash",
            "Minus",
            "Equal",
            "Backquote",
        ]
        .contains(&code)
}

/// Where the optional `=<logical key>` starts: the first `=` that is not the key itself
/// (`Ctrl+=` is the equals key).
fn logical_split(chord: &str) -> Option<usize> {
    let mut prev: Option<char> = None;
    for (i, c) in chord.char_indices() {
        if c == '=' && prev.is_some_and(|p| p != '+') {
            return Some(i);
        }
        prev = Some(c);
    }
    None
}

/// One key event from a chord: modifiers joined by `+`, then a W3C code or one printable
/// character, then an optional `=<logical key>`. The fields are exactly what
/// `keymap::key_event_from_parts` builds.
pub fn event_from_chord(chord: &str, down: bool) -> Result<KeyEvent, String> {
    let (body, logical_override) = match logical_split(chord) {
        Some(i) => (&chord[..i], Some(&chord[i + 1..])),
        None => (chord, None),
    };
    if body.is_empty() {
        return Err("empty chord".into());
    }
    // A key that is itself a plus sign: "+" or "Ctrl++".
    let (mod_part, key_token) = if body == "+" {
        ("", "+")
    } else if let Some(head) = body.strip_suffix("++") {
        (head, "+")
    } else {
        match body.rfind('+') {
            Some(i) => (&body[..i], &body[i + 1..]),
            None => ("", body),
        }
    };
    if key_token.is_empty() {
        return Err(format!("chord {chord:?} ends in a plus sign"));
    }
    let (mut ctrl, mut shift, mut alt, mut meta) = (false, false, false, false);
    if !mod_part.is_empty() {
        for m in mod_part.split('+') {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" => ctrl = true,
                "shift" => shift = true,
                "alt" => alt = true,
                "meta" => meta = true,
                _ => return Err(format!("unknown modifier {m:?} in {chord:?}")),
            }
        }
    }
    let (code, implied_shift) = if is_code(key_token) {
        (key_token.to_string(), false)
    } else if key_token.chars().count() == 1 {
        // A letter in a chord names the key; Shift is a modifier of its own.
        let c = key_token.chars().next().unwrap_or(' ').to_ascii_lowercase();
        code_of_char(c).ok_or_else(|| format!("no key for the character {key_token:?}"))?
    } else {
        return Err(format!("unknown key {key_token:?} in {chord:?}"));
    };
    shift |= implied_shift;
    let key = match logical_override {
        Some(l) if !l.is_empty() => l.to_string(),
        Some(_) => return Err(format!("empty logical key in {chord:?}")),
        None => logical_of_code(&code, shift),
    };
    let text = (is_printable_code(&code) && !(ctrl && !alt)).then(|| key.clone());
    Ok(KeyEvent {
        pressed: down,
        key,
        code,
        text,
        ctrl,
        shift,
        alt,
        meta,
    })
}

/// One typed character as a key press: `\n` is Enter, a character on the keyboard keeps its
/// code, anything else (an accent, an emoji) arrives as text only.
pub fn event_from_char(c: char, down: bool) -> KeyEvent {
    if c == '\n' {
        return KeyEvent {
            pressed: down,
            ..KeyEvent::new("Enter", "Enter")
        };
    }
    match code_of_char(c) {
        Some((code, shift)) => KeyEvent {
            pressed: down,
            key: c.to_string(),
            code,
            text: Some(c.to_string()),
            ctrl: false,
            shift,
            alt: false,
            meta: false,
        },
        None => KeyEvent {
            pressed: down,
            key: c.to_string(),
            code: "Unidentified".into(),
            text: Some(c.to_string()),
            ctrl: false,
            shift: false,
            alt: false,
            meta: false,
        },
    }
}

/// What Slint should see for a key the app left to it: the key's text, or Slint's own character
/// for a named key.
pub fn slint_text_for(ev: &KeyEvent) -> Option<SharedString> {
    use slint::platform::Key;
    if let Some(t) = &ev.text {
        return Some(SharedString::from(t.as_str()));
    }
    let k = match ev.key.as_str() {
        "Enter" => Key::Return,
        "Escape" => Key::Escape,
        "Tab" => Key::Tab,
        "Backspace" => Key::Backspace,
        "Delete" => Key::Delete,
        "ArrowUp" => Key::UpArrow,
        "ArrowDown" => Key::DownArrow,
        "ArrowLeft" => Key::LeftArrow,
        "ArrowRight" => Key::RightArrow,
        "Home" => Key::Home,
        "End" => Key::End,
        "PageUp" => Key::PageUp,
        "PageDown" => Key::PageDown,
        _ => return None,
    };
    Some(SharedString::from(char::from(k)))
}

// ---- pointer events (pure) -----------------------------------------------------------------

fn button(b: Button) -> PointerEventButton {
    match b {
        Button::Left => PointerEventButton::Left,
        Button::Right => PointerEventButton::Right,
        Button::Middle => PointerEventButton::Middle,
    }
}

/// The Slint events of one pointer action at a logical position.
pub fn pointer_events(x: f32, y: f32, action: &PointerAction) -> Vec<WindowEvent> {
    let position = LogicalPosition::new(x, y);
    let moved = || WindowEvent::PointerMoved { position };
    match action {
        PointerAction::Move => vec![moved()],
        PointerAction::Down { button: b } => vec![
            moved(),
            WindowEvent::PointerPressed {
                position,
                button: button(*b),
            },
        ],
        PointerAction::Up { button: b } => vec![WindowEvent::PointerReleased {
            position,
            button: button(*b),
        }],
        PointerAction::Click { button: b, count } => (0..(*count).max(1))
            .flat_map(|_| {
                [
                    moved(),
                    WindowEvent::PointerPressed {
                        position,
                        button: button(*b),
                    },
                    WindowEvent::PointerReleased {
                        position,
                        button: button(*b),
                    },
                ]
            })
            .collect(),
        PointerAction::Wheel { dx, dy } => vec![
            moved(),
            WindowEvent::PointerScrolled {
                position,
                delta_x: *dx,
                delta_y: *dy,
            },
        ],
        PointerAction::Leave => vec![WindowEvent::PointerExited],
    }
}

/// Press at `from`, `steps` equal moves, release at `to`.
pub fn drag_events(from: (f32, f32), to: (f32, f32), steps: u32, b: Button) -> Vec<WindowEvent> {
    let mut out = pointer_events(from.0, from.1, &PointerAction::Down { button: b });
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        out.extend(pointer_events(
            from.0 + (to.0 - from.0) * t,
            from.1 + (to.1 - from.1) * t,
            &PointerAction::Move,
        ));
    }
    out.extend(pointer_events(to.0, to.1, &PointerAction::Up { button: b }));
    out
}

// ---- drops ---------------------------------------------------------------------------------

/// What a file drop calls: winit's `HoveredFile` and `DroppedFile` end in these two.
pub trait DropTarget {
    fn hovered(&mut self);
    fn dropped(&mut self, path: PathBuf, at: (f32, f32));
}

impl DropTarget for AppCtl {
    fn hovered(&mut self) {
        self.on_hovered_file();
    }
    fn dropped(&mut self, path: PathBuf, at: (f32, f32)) {
        self.on_dropped_path(path, at);
    }
}

/// Hovered, then each path dropped at `at`.
pub fn drop_files(target: &mut dyn DropTarget, paths: &[String], at: (f32, f32)) {
    target.hovered();
    for p in paths {
        target.dropped(PathBuf::from(p), at);
    }
}

// ---- the dispatcher ------------------------------------------------------------------------

/// What the dispatcher needs from the running window.
pub struct GlueCtx {
    app: slint::Weak<AppWindow>,
    ctl: std::rc::Weak<RefCell<AppCtl>>,
    kp: Rc<KeyPath>,
    clip: Option<Arc<dyn ClipboardPort>>,
}

fn err(message: impl Into<String>) -> Reply {
    Reply::Err {
        message: message.into(),
    }
}

impl GlueCtx {
    fn dispatch(&self, events: Vec<WindowEvent>) -> Result<(), String> {
        let app = self.app.upgrade().ok_or("the window is gone")?;
        for e in events {
            app.window().dispatch_event(e);
        }
        Ok(())
    }

    /// One key through the path a real key takes; a key the app leaves to Slint reaches it too.
    fn key(&self, ev: &KeyEvent) -> Result<(), String> {
        let result = crate::window::handle_key(&self.kp, ev);
        if matches!(result, EventResult::Propagate)
            && let Some(text) = slint_text_for(ev)
        {
            let e = if ev.pressed {
                WindowEvent::KeyPressed { text }
            } else {
                WindowEvent::KeyReleased { text }
            };
            self.dispatch(vec![e])?;
        }
        Ok(())
    }

    fn with_ctl<R>(&self, f: impl FnOnce(&mut AppCtl) -> R) -> Result<R, String> {
        let rc = self.ctl.upgrade().ok_or("the controller is gone")?;
        let mut c = rc
            .try_borrow_mut()
            .map_err(|_| "the controller is busy".to_string())?;
        Ok(f(&mut c))
    }

    fn quit(&self, force: bool) -> Result<(), String> {
        let decision = self.with_ctl(|c| c.on_close_requested())?;
        if decision == CloseDecision::Close || force {
            if let Some(app) = self.app.upgrade() {
                let _ = app.hide();
            }
            let _ = slint::quit_event_loop();
        }
        Ok(())
    }

    fn window_op(&self, op: &str) -> Result<(), String> {
        let app = self.app.upgrade().ok_or("the window is gone")?;
        let w = app.window();
        match op {
            "minimise" | "minimize" => w.set_minimized(true),
            "restore" => {
                w.set_minimized(false);
                w.set_maximized(false);
            }
            "maximise" | "maximize" => w.set_maximized(true),
            "fullscreen" => {
                // The same chord the user presses.
                let ev = event_from_chord("F11", true)?;
                self.key(&ev)?;
                self.key(&KeyEvent {
                    pressed: false,
                    ..ev
                })?;
            }
            other => return Err(format!("unknown window op {other:?}")),
        }
        Ok(())
    }

    fn dpi_change(&self, dpi: u32, origin: Option<(i32, i32)>) -> Result<(), String> {
        if dpi == 0 {
            return Err("dpi must be above zero".into());
        }
        let app = self.app.upgrade().ok_or("the window is gone")?;
        let hwnd = crate::window::hwnd_of(&app).ok_or("the window has no handle")?;
        apply_dpi(hwnd, app.window(), dpi, origin, DpiMode::from_env());
        Ok(())
    }
    fn clip_set(&self, text: Option<&str>, png: Option<&str>) -> Result<(), String> {
        if !crate::e2e::global().on() {
            return Err("the real clipboard is never written".into());
        }
        let port = self.clip.as_ref().ok_or("no clipboard port")?;
        if png.is_some() {
            return Err("an image on the in-memory clipboard is not supported yet".into());
        }
        let text = text.ok_or("ClipSet needs text")?;
        port.write_text(text, usize::MAX)
    }
}

/// Holds the UI thread for `ms`, after the reply went out (the client's call returns at once and
/// the app is then blocked, as a stall would).
pub fn block_ui(ms: u64, resp: Responder, sleep: impl FnOnce(Duration)) {
    resp.reply(Reply::Ok);
    sleep(Duration::from_millis(ms)); // sync-ok: e2e only, holds the UI thread on purpose
}

// ---- the DPI change (Task 9) ---------------------------------------------------------------

/// 96 dpi is 100 percent.
pub fn dpi_to_scale(dpi: u32) -> f32 {
    dpi as f32 / 96.0
}

/// The rectangle Windows would suggest in `WM_DPICHANGED` for a window of `logical` size at
/// `origin`: left, top, right, bottom in device px, the size being `round(logical * dpi / 96)`.
pub fn suggested_rect(origin: (i32, i32), logical: (f32, f32), dpi: u32) -> (i32, i32, i32, i32) {
    let s = dpi_to_scale(dpi);
    let w = (logical.0 * s).round() as i32;
    let h = (logical.1 * s).round() as i32;
    (origin.0, origin.1, origin.0 + w, origin.1 + h)
}

/// How a DPI change reaches the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DpiMode {
    /// A real `WM_DPICHANGED` through the window procedure (the default).
    Message,
    /// The fallback: the `ScaleFactorChanged` event, then the new size.
    Event,
}

impl DpiMode {
    /// `FLARE_E2E_DPI_MODE=event` picks the fallback; anything else (or absent) the message.
    pub fn from_env_value(v: Option<&str>) -> DpiMode {
        match v {
            Some(s) if s.trim().eq_ignore_ascii_case("event") => DpiMode::Event,
            _ => DpiMode::Message,
        }
    }

    pub fn from_env() -> DpiMode {
        let name = ["FLARE_E2E", "_DPI_MODE"].concat();
        DpiMode::from_env_value(crate::e2e::global().env(&name).as_deref())
    }
}

/// What the event fallback needs from a window; a fake records the calls in the test.
pub trait DpiSeam {
    fn scale_factor_changed(&mut self, scale_factor: f32);
    fn set_physical_size(&mut self, w: u32, h: u32);
}

/// A `&slint::Window` as a `DpiSeam`.
struct WindowRef<'a>(&'a slint::Window);

impl DpiSeam for WindowRef<'_> {
    fn scale_factor_changed(&mut self, scale_factor: f32) {
        self.0
            .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor });
    }

    fn set_physical_size(&mut self, w: u32, h: u32) {
        self.0.set_size(slint::PhysicalSize::new(w, h));
    }
}

/// The event fallback: the same pair winit would send (the scale, then the resize to the suggested
/// size).
pub fn apply_dpi_event(seam: &mut dyn DpiSeam, logical: (f32, f32), dpi: u32) {
    let (l, t, r, b) = suggested_rect((0, 0), logical, dpi);
    seam.scale_factor_changed(dpi_to_scale(dpi));
    seam.set_physical_size((r - l) as u32, (b - t) as u32);
}

/// Changes the window's scaling as a monitor move would. `origin` is the new top-left in device
/// px, else the window's own. Runs on the UI thread; the reply is sent after it returns, and the
/// client then waits for the `app.state.scale` probe (the glue never claims success itself).
pub fn apply_dpi(
    hwnd: isize,
    window: &slint::Window,
    dpi: u32,
    origin: Option<(i32, i32)>,
    mode: DpiMode,
) {
    let scale = window.scale_factor().max(0.01);
    let size = window.size();
    let logical = (size.width as f32 / scale, size.height as f32 / scale);
    match mode {
        DpiMode::Message => {
            use windows_sys::Win32::Foundation::RECT;
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                GetWindowRect, SendMessageW, WM_DPICHANGED,
            };
            let h = hwnd as windows_sys::Win32::Foundation::HWND;
            let at = origin.unwrap_or_else(|| {
                let mut r = RECT {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                };
                // SAFETY: `h` is this process's window and `r` is a valid out pointer.
                unsafe { GetWindowRect(h, &mut r) };
                (r.left, r.top)
            });
            let (left, top, right, bottom) = suggested_rect(at, logical, dpi);
            let rect = RECT {
                left,
                top,
                right,
                bottom,
            };
            let wparam = (dpi as usize) | ((dpi as usize) << 16);
            // SAFETY: sent from the UI thread to its own window; `rect` outlives the synchronous call.
            unsafe {
                SendMessageW(h, WM_DPICHANGED, wparam, &rect as *const RECT as isize);
            }
        }
        DpiMode::Event => apply_dpi_event(&mut WindowRef(window), logical, dpi),
    }
}
/// The one `match` over every request. Tasks 8 to 11 each own the arms named in their title.
fn handle(ctx: &GlueCtx, req: Request, resp: Responder) {
    let result: Result<(), String> = match req {
        Request::Key { chord, down } => event_from_chord(&chord, down).and_then(|e| ctx.key(&e)),
        Request::KeyPress { chord } => event_from_chord(&chord, true)
            .and_then(|e| ctx.key(&e))
            .and_then(|()| event_from_chord(&chord, false))
            .and_then(|e| ctx.key(&e)),
        Request::Text { text } => text.chars().try_for_each(|c| {
            ctx.key(&event_from_char(c, true))?;
            ctx.key(&event_from_char(c, false))
        }),
        Request::Pointer { x, y, action } => ctx.dispatch(pointer_events(x, y, &action)),
        Request::Drag {
            from,
            to,
            steps,
            button,
        } => ctx.dispatch(drag_events(from, to, steps, button)),
        Request::DropFiles { paths, x, y } => ctx.with_ctl(|c| drop_files(c, &paths, (x, y))),
        Request::Resize { w, h } => ctx
            .app
            .upgrade()
            .ok_or_else(|| "the window is gone".to_string())
            .map(|a| a.window().set_size(slint::LogicalSize::new(w, h))),
        Request::Focus { on } => {
            crate::window::handle_focus(&ctx.kp, on);
            ctx.dispatch(vec![WindowEvent::WindowActiveChanged(on)])
        }
        Request::Window { op } => ctx.window_op(&op),
        Request::ClipSet { text, png_b64 } => ctx.clip_set(text.as_deref(), png_b64.as_deref()),
        Request::Slow { name, ms } => {
            // sync-ok: e2e only, a work item that takes `ms` so the work timer reports it
            crate::diag_ctl::timed(&name, || std::thread::sleep(Duration::from_millis(ms)));
            Ok(())
        }
        Request::BlockUi { ms } => {
            // sync-ok: e2e only, holds the UI thread on purpose (diagLog)
            block_ui(ms, resp, std::thread::sleep);
            return;
        }
        Request::PanicWorker => {
            let _ = std::thread::Builder::new()
                .name("e2e-panic".into())
                .spawn(|| panic!("e2e: worker panic"));
            Ok(())
        }
        Request::Quit { force } => ctx.quit(force),
        // Tasks 9 to 11 replace these.
        Request::Hello => Err("Hello is answered by the server".into()),
        Request::DpiChange { dpi, x, y } => ctx.dpi_change(dpi, x.zip(y)),
        Request::Snapshot { .. }
        | Request::Pixel { .. }
        | Request::Frames { .. }
        | Request::Latency { .. }
        | Request::KeyAtPrompt { .. } => Err("not built yet (Task 10)".into()),
        Request::Probe { name, .. } => Err(format!("probe {name:?} is not built yet (Task 11)")),
    };
    resp.reply(match result {
        Ok(()) => Reply::Ok,
        Err(m) => err(m),
    });
}

/// Starts the hook for an `--e2e` run: parks the window, starts the pipe server (Task 7), registers
/// the UI handler and the probe registry. The caller (window.rs) runs this only inside its
/// `if args.e2e` block.
pub fn install(
    app: &AppWindow,
    ctl: &Rc<RefCell<AppCtl>>,
    args: &Args,
    kp: &Rc<KeyPath>,
    clip: Option<Arc<dyn ClipboardPort>>,
) {
    match crate::window::hwnd_of(app) {
        Some(hwnd) => flare_win::window_park::park(hwnd),
        None => log::warn!("e2e: the window has no handle, it is not parked"),
    }
    let appdata = std::env::var("APPDATA").ok();
    let profile = crate::startup::profile_dir(args.user_data_dir.as_deref(), appdata.as_deref());
    match crate::e2e_hook::start(&profile) {
        Ok(server) => {
            log::info!("e2e: the test hook listens on {}", server.pipe);
            HOOK.with(|h| *h.borrow_mut() = Some(server));
        }
        Err(e) => log::warn!("e2e: the test hook did not start: {e:?}"),
    }
    let ctx = GlueCtx {
        app: app.as_weak(),
        ctl: Rc::downgrade(ctl),
        kp: kp.clone(),
        clip,
    };
    crate::e2e_hook::register_ui_handler(Rc::new(move |req, resp| handle(&ctx, req, resp)));
    crate::e2e_probes::register_all(&mut crate::e2e_probes::ProbeRegistry);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::key_event_from_parts;
    use slint::winit_030::winit::keyboard::{Key, KeyCode, ModifiersState, PhysicalKey};

    fn ch(s: &str) -> Key {
        Key::Character(s.into())
    }

    fn named(n: slint::winit_030::winit::keyboard::NamedKey) -> Key {
        Key::Named(n)
    }

    /// (chord, physical, logical, text, modifiers) as winit would report them.
    fn table() -> Vec<(
        &'static str,
        KeyCode,
        Key,
        Option<&'static str>,
        ModifiersState,
    )> {
        use slint::winit_030::winit::keyboard::NamedKey as N;
        let none = ModifiersState::empty();
        let ctrl = ModifiersState::CONTROL;
        let shift = ModifiersState::SHIFT;
        let alt = ModifiersState::ALT;
        vec![
            ("Ctrl+T", KeyCode::KeyT, ch("t"), Some("\u{14}"), ctrl),
            ("Ctrl+t", KeyCode::KeyT, ch("t"), Some("\u{14}"), ctrl),
            (
                "Ctrl+Shift+KeyC",
                KeyCode::KeyC,
                ch("C"),
                Some("\u{3}"),
                ctrl | shift,
            ),
            ("Shift+Enter", KeyCode::Enter, named(N::Enter), None, shift),
            ("Alt+b", KeyCode::KeyB, ch("b"), Some("b"), alt),
            ("F11", KeyCode::F11, named(N::F11), None, none),
            ("Ctrl+,", KeyCode::Comma, ch(","), Some(","), ctrl),
            ("Ctrl+Tab", KeyCode::Tab, named(N::Tab), None, ctrl),
            ("ArrowUp", KeyCode::ArrowUp, named(N::ArrowUp), None, none),
            ("Ctrl+Alt+q", KeyCode::KeyQ, ch("q"), Some("q"), ctrl | alt),
            ("?", KeyCode::Slash, ch("?"), Some("?"), shift),
            ("t", KeyCode::KeyT, ch("t"), Some("t"), none),
            ("Shift+t", KeyCode::KeyT, ch("T"), Some("T"), shift),
            ("Digit1", KeyCode::Digit1, ch("1"), Some("1"), none),
            ("Shift+Digit1", KeyCode::Digit1, ch("!"), Some("!"), shift),
            ("Space", KeyCode::Space, named(N::Space), Some(" "), none),
            ("Escape", KeyCode::Escape, named(N::Escape), None, none),
            (
                "Backspace",
                KeyCode::Backspace,
                named(N::Backspace),
                None,
                none,
            ),
            ("Ctrl+Delete", KeyCode::Delete, named(N::Delete), None, ctrl),
            ("PageUp", KeyCode::PageUp, named(N::PageUp), None, none),
            ("Home", KeyCode::Home, named(N::Home), None, none),
            ("End", KeyCode::End, named(N::End), None, none),
            ("Ctrl+.", KeyCode::Period, ch("."), Some("."), ctrl),
            ("Ctrl+/", KeyCode::Slash, ch("/"), Some("/"), ctrl),
            ("Ctrl+`", KeyCode::Backquote, ch("`"), Some("`"), ctrl),
            ("Numpad0", KeyCode::Numpad0, ch("0"), Some("0"), none),
            ("F5", KeyCode::F5, named(N::F5), None, none),
        ]
    }

    #[test]
    fn a_chord_builds_the_event_keymap_would_build() {
        let cases = table();
        assert!(cases.len() >= 25, "{} chords", cases.len());
        for (chord, code, logical, text, mods) in cases {
            for down in [true, false] {
                let want =
                    key_event_from_parts(down, &PhysicalKey::Code(code), &logical, text, mods);
                let got = event_from_chord(chord, down).unwrap_or_else(|e| panic!("{chord}: {e}"));
                assert_eq!(got, want, "chord {chord:?} down={down}");
            }
        }
    }

    #[test]
    fn a_logical_key_override_keeps_the_physical_key() {
        let got = event_from_chord("Ctrl+KeyC=\u{441}", true).expect("parses");
        assert_eq!(got.code, "KeyC");
        assert_eq!(got.key, "\u{441}");
        assert!(got.ctrl);
        assert_eq!(got.text, None, "Ctrl without Alt drops the text");
        let eq = event_from_chord("Ctrl+=", true).expect("the equals key");
        assert_eq!((eq.code.as_str(), eq.key.as_str()), ("Equal", "="));
        let plus = event_from_chord("Ctrl++", true).expect("the plus key");
        assert_eq!(plus.key, "+");
    }

    #[test]
    fn a_bad_chord_is_an_error_message() {
        for bad in ["Ctrl+", "Foo+T", "", "Ctrl+Nope", "Ctrl+KeyC="] {
            assert!(event_from_chord(bad, true).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn text_is_a_key_per_character_and_a_newline_is_enter() {
        let a = event_from_char('a', true);
        assert_eq!((a.code.as_str(), a.text.as_deref()), ("KeyA", Some("a")));
        let big = event_from_char('A', true);
        assert!(big.shift && big.key == "A");
        let enter = event_from_char('\n', true);
        assert_eq!((enter.key.as_str(), enter.text), ("Enter", None));
        let accent = event_from_char('\u{e9}', true);
        assert_eq!(accent.text.as_deref(), Some("\u{e9}"));
    }

    #[test]
    fn a_click_is_move_press_release_at_the_logical_position() {
        let evs = pointer_events(
            10.5,
            20.0,
            &PointerAction::Click {
                button: Button::Left,
                count: 2,
            },
        );
        assert_eq!(evs.len(), 6);
        assert!(matches!(
            evs[1],
            WindowEvent::PointerPressed {
                button: PointerEventButton::Left,
                ..
            }
        ));
        if let WindowEvent::PointerMoved { position } = &evs[0] {
            assert_eq!((position.x, position.y), (10.5, 20.0));
        } else {
            panic!("the first event is a move");
        }
        let drag = drag_events((0.0, 0.0), (10.0, 0.0), 5, Button::Left);
        assert_eq!(drag.len(), 2 + 5 + 1);
        assert!(matches!(
            drag.last(),
            Some(WindowEvent::PointerReleased { .. })
        ));
    }

    #[derive(Default)]
    struct Fake(Vec<String>);

    impl DropTarget for Fake {
        fn hovered(&mut self) {
            self.0.push("hover".into());
        }
        fn dropped(&mut self, path: PathBuf, at: (f32, f32)) {
            self.0.push(format!("drop {} {:?}", path.display(), at));
        }
    }

    #[test]
    fn drop_calls_hover_then_drop() {
        let mut f = Fake::default();
        drop_files(&mut f, &["C:/a.txt".into(), "C:/b.txt".into()], (5.0, 6.0));
        assert_eq!(
            f.0,
            vec![
                "hover",
                "drop C:/a.txt (5.0, 6.0)",
                "drop C:/b.txt (5.0, 6.0)"
            ]
        );
    }

    #[test]
    fn block_ui_replies_before_it_blocks() {
        let (resp, rx) = crate::e2e_hook::responder();
        let order = std::cell::RefCell::new(Vec::new());
        block_ui(40, resp, |d| {
            order.borrow_mut().push(format!("sleep {}", d.as_millis()));
            // By now the reply is already waiting.
            assert_eq!(rx.try_recv().ok(), Some(Reply::Ok));
        });
        assert_eq!(order.into_inner(), vec!["sleep 40"]);
    }

    #[test]
    fn park_is_called_first_in_install() {
        let src = include_str!("e2e_glue.rs");
        let from = src.find("pub fn install(").expect("install exists");
        let body = &src[from..];
        let open = body.find('{').expect("a body");
        let first = body[open + 1..].trim_start();
        assert!(
            first.starts_with("match crate::window::hwnd_of(app)")
                && first[..200].contains("window_park::park"),
            "park must be the first thing install does"
        );
    }

    #[test]
    fn named_keys_reach_slint_as_its_own_characters() {
        let esc = event_from_chord("Escape", true).expect("parses");
        let t = slint_text_for(&esc).expect("a text");
        assert_eq!(
            t.as_str(),
            String::from(char::from(slint::platform::Key::Escape))
        );
        assert!(slint_text_for(&event_from_chord("F11", true).expect("parses")).is_none());
    }

    #[test]
    fn a_click_reaches_the_slint_component_at_every_scale() {
        use crate::ui_harness::{Harness, SCALES};
        use crate::{StripGeom, TabStripPreview};
        use slint::ModelRc;
        for s in SCALES {
            let h = Harness::new(s, 400, 36, || {
                let c = TabStripPreview::new().expect("the strip builds");
                c.set_rows(ModelRc::default());
                c.set_geom(StripGeom {
                    more_x: 0,
                    more_w: 0,
                    plus_x: 100,
                    plus_w: 28,
                    more_count: 0,
                    dots: ModelRc::default(),
                });
                c.set_strip_x(0.0);
                c.set_new_tab_title("New tab".into());
                c
            });
            let news = Rc::new(RefCell::new(0));
            let n = news.clone();
            h.comp.on_new_tab(move || *n.borrow_mut() += 1);
            // The strip's geometry is in device pixels: the button's centre is 114 of them.
            for e in pointer_events(
                114.0 / s,
                18.0,
                &PointerAction::Click {
                    button: Button::Left,
                    count: 1,
                },
            ) {
                h.comp.window().dispatch_event(e);
            }
            assert_eq!(*news.borrow(), 1, "scale {s}");
        }
    }

    #[test]
    fn an_unconsumed_key_with_text_reaches_a_slint_text_field() {
        use crate::FramePreview;
        use crate::ui_harness::Harness;
        let h = Harness::new(1.0, 1000, 604, || {
            FramePreview::new().expect("the preview builds")
        });
        h.advance(0);
        h.comp.invoke_focus_field();
        h.advance(0);
        let send = |ev: KeyEvent| {
            if let Some(text) = slint_text_for(&ev) {
                let e = if ev.pressed {
                    WindowEvent::KeyPressed { text }
                } else {
                    WindowEvent::KeyReleased { text }
                };
                h.comp.window().dispatch_event(e);
            }
        };
        for c in "colour".chars() {
            send(event_from_char(c, true));
            send(event_from_char(c, false));
        }
        for down in [true, false] {
            send(event_from_chord("Escape", down).expect("parses"));
        }
        assert_eq!(
            h.comp.get_escape_unhandled_count(),
            0,
            "a field with text claims Escape"
        );
        assert_eq!(h.comp.get_last_key(), "find:Escape");
    }

    #[test]
    fn dpi_96_to_288_is_the_eight_scales() {
        let want = [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 3.0];
        for (dpi, w) in [96, 120, 144, 168, 192, 216, 240, 288]
            .into_iter()
            .zip(want)
        {
            assert!((dpi_to_scale(dpi) - w).abs() < 1e-6, "dpi {dpi}");
        }
    }

    #[test]
    fn the_suggested_rect_keeps_the_logical_size_and_the_origin() {
        for dpi in [96u32, 120, 144, 168, 192, 216, 240, 288] {
            let s = dpi_to_scale(dpi);
            let (l, t, r, b) = suggested_rect((-1920, 40), (1000.0, 640.0), dpi);
            assert_eq!((l, t), (-1920, 40), "origin at {dpi}");
            assert_eq!(r - l, (1000.0 * s).round() as i32, "width at {dpi}");
            assert_eq!(b - t, (640.0 * s).round() as i32, "height at {dpi}");
        }
    }

    #[derive(Default)]
    struct FakeWindow {
        calls: Vec<String>,
    }

    impl DpiSeam for FakeWindow {
        fn scale_factor_changed(&mut self, scale_factor: f32) {
            self.calls.push(format!("scale {scale_factor}"));
        }

        fn set_physical_size(&mut self, w: u32, h: u32) {
            self.calls.push(format!("size {w}x{h}"));
        }
    }

    #[test]
    fn the_event_fallback_sends_the_same_pair_winit_would() {
        let mut w = FakeWindow::default();
        apply_dpi_event(&mut w, (800.0, 600.0), 144);
        assert_eq!(w.calls, vec!["scale 1.5", "size 1200x900"]);
    }

    #[test]
    fn the_default_mode_is_the_message() {
        assert_eq!(DpiMode::from_env_value(None), DpiMode::Message);
        assert_eq!(DpiMode::from_env_value(Some("")), DpiMode::Message);
        assert_eq!(DpiMode::from_env_value(Some("nonsense")), DpiMode::Message);
        assert_eq!(DpiMode::from_env_value(Some("event")), DpiMode::Event);
        assert_eq!(DpiMode::from_env_value(Some("EVENT")), DpiMode::Event);
    }
}
