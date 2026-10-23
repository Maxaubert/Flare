//! WebCtl: the web panel of each tab, and what the window shows of it (Surfaces stage 3).
//!
//! The browser itself is a [`WebBackend`] (WebView2 in production, a stand-in in the tests) made
//! only when the first web panel needs it. A tab with an open web panel has one controller; closing
//! the panel or the tab frees it, and the last one takes the browser's environment with it. Nothing
//! here waits for the browser (rule 34): a call starts the work and the answer comes back as a
//! [`WebEvent`] through the window's queue.
//!
//! The browser window is a child window above everything Slint draws (airspace). Each sync decides,
//! from the window's own numbers, whether it is live, hidden behind an overlay with a still of its
//! last frame drawn in its place, or hidden because its tab is behind (`flare_surfaces::web`). Its
//! rectangle is the slot's body as Slint lays it out, in physical pixels, after every layout change,
//! splitter move and scale change.
//!
//! Keys: when the page has the keyboard WebView2 asks, before the page sees a key, whether the app
//! wants it. The app's chords (the same table the terminal uses, less Find and the agent guard,
//! which a page has its own meaning for) are the app's; a second Escape inside 600 ms gives the keys
//! back to the terminal; everything else is the page's.
//!
//! The agent (stage 4): a call to look at the page or use it is a [`Job`] (`flare_surfaces::agent_job`),
//! a state machine that says which DevTools Protocol calls to make. The browser answers each as a
//! `WebEvent::Cdp` through the window's queue and the job goes on from there; nothing waits. While a
//! job runs, and for a moment after, the panel's head wears a ribbon. The person's own key or click in
//! the page ends the job (`interrupted`): the keys are heard in the key hook, a click by looking at the
//! real mouse buttons on a timer (no hook is installed).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::time::Instant;

use flare_ctl::{CtlError, code};
use flare_surfaces::agent_job::{
    Fail, Ids, Job, JobError, Op as AgentOp, Out as JobOut, Page, RefMap,
};
use flare_surfaces::app::{self, AppSpec};
use flare_surfaces::web::{
    self, Act, CAPTURE_TIMEOUT_MS, EscTwice, Phase, PhysRect, UrlError, WebShow, host_label,
    normalize_url, phys_rect, step, url_matches, web_show,
};
use flare_tabs::keymap::{ChordId, KeyContext, chord_of};
use flare_term::keys::KeyEvent;
use flare_web::{KeyFilter, KeyPress, Nav, Sink, WebBackend, WebEvent};
use serde_json::Value;
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

use crate::AppWindow;
use crate::appctl::{Post, UiMsg};
use crate::panel_ctl::PanelCtl;

/// Where a person gets the WebView2 Runtime when it is missing (Microsoft's evergreen bootstrapper page).
pub const RUNTIME_URL: &str = "https://go.microsoft.com/fwlink/p/?LinkId=2124703";

/// The timer that ends a capture the page never answers (`timer_registry` name).
const CAPTURE_TIMER: &str = "web.capture";
/// The timer that runs while an agent call is on a page or its ribbon is still up.
const AGENT_TIMER: &str = "web.agent";
/// How often that timer looks (the person's mouse, a wait, a deadline, the ribbon). It runs only while
/// a call is on a page or the ribbon is up, and is about one frame, so a quick click is seen.
const AGENT_TICK_MS: u64 = 16;
/// How long the ribbon stays after the agent's call ends.
pub const RIBBON_HOLD_MS: u64 = 1_500;

/// Makes the browser (the first time a web panel needs it).
pub type WebFactory = Box<dyn FnMut(Sink) -> Box<dyn WebBackend>>;

/// A message for the web controller.
pub enum WebMsg {
    Event(WebEvent),
    /// A still, decoded on a worker.
    Decoded {
        tab: String,
        token: u64,
        image: Option<SharedPixelBuffer<Rgba8Pixel>>,
    },
    /// The person pressed a key in the page (or, for the e2e, stands in for a click): an agent call on
    /// the page in front ends.
    UserInput,
    /// The agent timer: waits, deadlines, the person's mouse and the ribbon.
    AgentTick,
}

/// A `WebMsg` that can sit in a `UiMsg` (which is `Debug`): it prints its kind only, never an
/// address, a title or a picture.
pub struct WebMsgBox(pub WebMsg);

impl std::fmt::Debug for WebMsgBox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            WebMsg::Event(e) => write!(f, "WebMsg::{}", e.kind()),
            WebMsg::Decoded { .. } => f.write_str("WebMsg::Decoded"),
            WebMsg::UserInput => f.write_str("WebMsg::UserInput"),
            WebMsg::AgentTick => f.write_str("WebMsg::AgentTick"),
        }
    }
}

/// What a finished navigation says to the channel call that waited for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settled {
    pub url: String,
    pub title: String,
    pub loaded: bool,
    pub status: String,
}

/// What the controller asks the window to do after a message.
pub enum WebOut {
    /// A waiting channel call is answered.
    Settle {
        reply: Sender<Result<Value, CtlError>>,
        tab: String,
        result: Result<Settled, CtlError>,
    },
    /// The page of a tab moved: the panel points at the new address.
    Address { tab: String, url: String },
    /// The page's title changed (the panel is named by it for the channel).
    Title { tab: String, title: String },
    /// The app took a key from the page.
    Key(KeyPress),
    /// The page took (true) or gave up (false) the keyboard.
    PageFocus(bool),
    /// The tab's browser is gone for good (the panel should say so).
    Changed,
    /// A page started a download: nothing is saved until the person has answered.
    Download {
        tab: String,
        id: u64,
        file: String,
        host: String,
    },
    /// A mod's page sent a message over its bridge (stage 8).
    ModMessage {
        tab: String,
        /// The mod whose page sent it (named by the host of the view that received it).
        mod_id: String,
        json: String,
    },
    /// A mod's page began loading (`ready` false) or finished loading well (true): it can be spoken to only
    /// while it is ready.
    ModPage { tab: String, ready: bool },
    /// A mod's page, or the browser it ran in, is gone.
    ModLost { tab: String },
}

struct Pending {
    reply: Sender<Result<Value, CtlError>>,
    deadline: Instant,
    /// The address the call sent the page to ("" for back, forward and reload): the first load that
    /// counts as its own is the one for this address.
    url: String,
    /// The navigation this call waits for: the browser's number for the latest one that began after
    /// the call was made (a redirect rebinds it). A load that finishes before any has begun, or one
    /// that is not this one, is not the answer.
    nav: Option<u64>,
    /// A diagram: once the page has loaded, the page is asked whether the drawing worked, and the call
    /// is answered with that.
    diagram: bool,
}

/// A diagram page that has loaded: its tab, the call waiting and what the load said.
type DiagramAsk = (String, Sender<Result<Value, CtlError>>, Settled);

/// The question put to a diagram page after it loaded (`app::diagram_result` reads the answer).
struct Probe {
    id: u64,
    reply: Sender<Result<Value, CtlError>>,
    settled: Settled,
}

/// An agent call in flight on a tab's page.
struct AgentCall {
    job: Job,
    reply: Sender<Result<Value, CtlError>>,
    /// Who is using the page, for the ribbon ("Claude").
    label: String,
    /// Which document the page was showing when the call began (`TabWeb::doc`).
    doc: u64,
}

fn ctl_error(e: JobError) -> CtlError {
    let c = match e.fail {
        Fail::BadParams => code::BAD_PARAMS,
        Fail::NotFound => code::NOT_FOUND,
        Fail::Unavailable => code::UNAVAILABLE,
        Fail::Timeout => code::TIMEOUT,
        Fail::Interrupted => code::INTERRUPTED,
        Fail::Internal => code::INTERNAL,
    };
    CtlError::new(c, e.message)
}

/// Who is using the page, for the ribbon: the tab's agent by name, else the plain word.
pub fn agent_label(k: flare_agent::kinds::DetectedAgent) -> String {
    use flare_agent::kinds::DetectedAgent as D;
    match k {
        D::Claude => "Claude",
        D::Codex => "Codex",
        D::Other => "An agent",
    }
    .to_string()
}

/// The words of the ribbon.
pub fn ribbon_words(label: &str) -> String {
    format!("{label} is using this page")
}

/// Is a point inside a rectangle (client pixels).
pub fn point_in(r: &PhysRect, x: i32, y: i32) -> bool {
    x >= r.x && x < r.right() && y >= r.y && y < r.bottom()
}

/// Is Flare's window minimized (nothing paints).
fn window_minimized(hwnd: isize) -> bool {
    // SAFETY: a plain read of the state of Flare's own window.
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsIconic(hwnd as *mut _) != 0 }
}

/// Forgets the mouse presses so far: `real_button_down_over` also sees a press that came and went since
/// the last look, and a call starting must not count one from before it.
fn flush_button_edges() {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON,
    };
    for k in [VK_LBUTTON, VK_RBUTTON, VK_MBUTTON] {
        // SAFETY: a plain read of a key state; it also clears the "pressed since" bit.
        unsafe {
            GetAsyncKeyState(i32::from(k));
        }
    }
    for vk in TYPING_KEYS {
        // SAFETY: as above, for the keyboard.
        unsafe {
            GetAsyncKeyState(i32::from(vk));
        }
    }
}

/// The keys that count as the person typing: every key but the mouse buttons and the modifiers alone
/// (a held Shift or Ctrl is not typing; the key pressed with it is).
const TYPING_KEYS: std::ops::RangeInclusive<u8> = 0x08..=0xFE;

fn is_typing_key(vk: u8) -> bool {
    TYPING_KEYS.contains(&vk)
        && !matches!(vk, 0x10..=0x12 | 0x5B | 0x5C | 0xA0..=0xA5 | 0x14 | 0x90 | 0x91)
}

/// Was a real key pressed (down now, or since the last look)? WebView2 hands the host only chords and
/// non-character keys, so plain typing in the page never reaches the key filter: the keyboard is read here.
/// The agent's own keys are browser events and press nothing.
fn real_key_pressed() -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    // SAFETY: a plain read of a key state.
    TYPING_KEYS
        .filter(|vk| is_typing_key(*vk))
        .any(|vk| (unsafe { GetAsyncKeyState(i32::from(vk)) } as u16) & 0x8001 != 0)
}

/// Is a real mouse button down, or was one pressed since the last look, with the pointer over `r`
/// (client pixels of `hwnd`)? The agent's own clicks are browser events and move neither the pointer
/// nor a button, so this is the person. The "pressed since" bit catches a click that began and ended
/// between two looks.
fn real_button_down_over(hwnd: isize, r: &PhysRect) -> bool {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GA_ROOT, GetAncestor, GetCursorPos, WindowFromPoint,
    };
    // SAFETY: plain reads of the pointer, the button state and the window under the pointer; `p` is a
    // valid out value and `hwnd` is Flare's own window.
    unsafe {
        let down = [VK_LBUTTON, VK_RBUTTON, VK_MBUTTON]
            .iter()
            .any(|k| (GetAsyncKeyState(i32::from(*k)) as u16) & 0x8001 != 0);
        if !down {
            return false;
        }
        let mut p = POINT { x: 0, y: 0 };
        if GetCursorPos(&mut p) == 0 {
            return false;
        }
        // The pointer must be over Flare's own window, not another window that covers the page.
        if GetAncestor(WindowFromPoint(p), GA_ROOT) != hwnd as *mut _ {
            return false;
        }
        if ScreenToClient(hwnd as *mut _, &mut p) == 0 {
            return false;
        }
        point_in(r, p.x, p.y)
    }
}

#[derive(Default)]
struct TabWeb {
    url: String,
    title: String,
    loading: bool,
    can_back: bool,
    can_forward: bool,
    ready: bool,
    /// Why the browser cannot be shown, and whether the runtime is simply missing.
    failed: Option<(String, bool)>,
    /// A navigation the address field or a caller refused or the page was sent away from.
    notice: String,
    pending: Option<Pending>,
    /// The last finished navigation, for a call that arrives after it.
    last: Option<Settled>,
    /// The agent call on this page, if one is running (one at a time).
    agent: Option<AgentCall>,
    /// Raised each time the page starts a navigation: a look that began on an earlier document does not
    /// leave its element names behind.
    doc: u64,
    /// The element names of the agent's last look; they last until the next look or page load.
    refs: RefMap,
    /// The ribbon's words and the moment it goes, after a call that has ended.
    hold: Option<(String, u64)>,
    /// The host of the mini-app this view shows (None for a plain web page): a view is closed and made
    /// again when the panel's app is not the one it was made for.
    app_host: Option<String>,
    /// A diagram's "did it draw" question, while it is out.
    probe: Option<Probe>,
    /// The browser's number of the last navigation the host allowed (one it cancelled never has its own).
    started_nav: Option<u64>,
}

/// The state the key hook reads (it runs inside a WebView2 callback and may not borrow the window).
struct KeyState {
    ctx: Cell<KeyContext>,
    esc: RefCell<EscTwice>,
    clock: Instant,
    /// An agent call is running on a page: a key the person presses ends it.
    job_active: Cell<bool>,
    /// The agent's key press is in flight: the browser may report its events to the key hook, and they
    /// are neither the person's nor the app's chords.
    agent_keys: Cell<bool>,
    /// How many times the key hook was asked about a key while the agent's own press was in flight (it
    /// says whether the browser reports DevTools key events to the hook; the e2e reads it).
    agent_heard: Cell<u32>,
}

pub struct WebCtl {
    post: Post,
    factory: Option<WebFactory>,
    backend: Option<Box<dyn WebBackend>>,
    tabs: HashMap<String, TabWeb>,
    /// The tab whose browser window is shown (live, behind an overlay or being captured), if any.
    shown_tab: Option<String>,
    phase: Phase,
    token: u64,
    bounds: Option<(String, PhysRect)>,
    keys: Rc<KeyState>,
    has_focus: bool,
    scripts: HashMap<u64, Result<String, String>>,
    script_seq: u64,
    /// The last id given to a DevTools call (unique across the window).
    call_ids: u64,
    /// The still of the shown tab while an overlay hides its window.
    still: Option<Image>,
    /// The capture the still answers.
    still_token: u64,
    /// The capture that was ended without a picture (the timeout, or a page that could not take one):
    /// a picture that still arrives for it, while the window is hidden for the same overlay, is drawn.
    ended_capture: Option<u64>,
    /// Flare's window, for handing the Windows keyboard focus back (set by every sync).
    hwnd: Option<isize>,
    /// The window is ending: the browser is closed and never made again.
    closed: bool,
}

/// The keys WebView2 reports in Win32 virtual-key codes, as the chord table knows them.
pub fn key_event_of(k: &KeyPress) -> Option<KeyEvent> {
    let (key, code) = match k.vk {
        0x41..=0x5A => {
            let c = char::from_u32(k.vk)?;
            (c.to_ascii_lowercase().to_string(), format!("Key{c}"))
        }
        0x30..=0x39 => {
            let c = char::from_u32(k.vk)?;
            (c.to_string(), format!("Digit{c}"))
        }
        0x70..=0x7B => {
            let n = k.vk - 0x6F;
            (format!("F{n}"), format!("F{n}"))
        }
        0x09 => ("Tab".to_string(), "Tab".to_string()),
        0x21 => ("PageUp".to_string(), "PageUp".to_string()),
        0x22 => ("PageDown".to_string(), "PageDown".to_string()),
        0x20 => (" ".to_string(), "Space".to_string()),
        0xBC => (",".to_string(), "Comma".to_string()),
        0x1B => ("Escape".to_string(), "Escape".to_string()),
        _ => return None,
    };
    Some(KeyEvent {
        pressed: k.down,
        key,
        code,
        text: None,
        ctrl: k.ctrl,
        shift: k.shift,
        alt: k.alt,
        meta: false,
    })
}

/// Does the app take this key from the page? The chord table, less the two chords whose key a page
/// has its own use for: Ctrl+F (the page's find) and Ctrl+D / Ctrl+C of the agent guard.
pub fn app_takes(k: &KeyPress, ctx: &KeyContext) -> bool {
    let Some(ev) = key_event_of(k) else {
        return false;
    };
    match chord_of(&ev, ctx) {
        Some(ChordId::AgentGuard) => false,
        Some(ChordId::Find) => ev.shift,
        Some(_) => true,
        None => false,
    }
}

/// What the window does when it loses the keyboard while a web panel may hold it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusLoss {
    /// The window is not the one in use: dictation, the walk, tips and menus let go now.
    Now,
    /// The window is still the foreground window: that is the keyboard moving into the page, or a
    /// switch to another app that Windows has not finished. Ask again one turn later.
    Check,
    /// The page has the keyboard: the window is still the one in use.
    Skip,
}

/// Decides on where the keyboard went, not on which window is in front at the instant of the
/// kill-focus (at that instant a real switch to another app can still look like the window staying).
pub fn focus_loss(
    has_backend: bool,
    page_has_focus: bool,
    window_is_foreground: bool,
) -> FocusLoss {
    if !has_backend {
        FocusLoss::Now
    } else if page_has_focus {
        FocusLoss::Skip
    } else if window_is_foreground {
        FocusLoss::Check
    } else {
        FocusLoss::Now
    }
}

fn runtime_missing_note() -> String {
    format!(
        "Flare shows web pages with the Microsoft Edge WebView2 Runtime, and this PC does not have it. Install it from {RUNTIME_URL}, then open the page again."
    )
}

impl WebCtl {
    pub fn new(post: Post) -> Self {
        Self {
            post,
            factory: None,
            backend: None,
            tabs: HashMap::new(),
            shown_tab: None,
            phase: Phase::Hidden,
            token: 0,
            bounds: None,
            keys: Rc::new(KeyState {
                ctx: Cell::new(KeyContext::default()),
                esc: RefCell::new(EscTwice::default()),
                clock: Instant::now(),
                job_active: Cell::new(false),
                agent_keys: Cell::new(false),
                agent_heard: Cell::new(0),
            }),
            has_focus: false,
            scripts: HashMap::new(),
            script_seq: 0,
            call_ids: 0,
            still: None,
            still_token: 0,
            ended_capture: None,
            hwnd: None,
            closed: false,
        }
    }

    /// How the browser is made (startup; tests hand a stand-in).
    pub fn set_factory(&mut self, f: WebFactory) {
        self.factory = Some(f);
        self.backend = None;
    }

    fn sink(&self) -> Sink {
        let post = std::sync::Arc::clone(&self.post);
        std::sync::Arc::new(move |e| post(UiMsg::Web(WebMsgBox(WebMsg::Event(e)))))
    }

    fn key_filter(&self) -> KeyFilter {
        let keys = Rc::clone(&self.keys);
        let post = std::sync::Arc::clone(&self.post);
        Rc::new(move |k: &KeyPress| {
            if !k.down {
                return false;
            }
            // The agent's own key press is the page's whole business.
            if keys.agent_keys.get() {
                keys.agent_heard.set(keys.agent_heard.get() + 1);
                return false;
            }
            // A key of the person's own, while an agent call runs on the page: they have taken over.
            if keys.job_active.get() {
                post(UiMsg::Web(WebMsgBox(WebMsg::UserInput)));
            }
            if k.vk == 0x1B && !k.ctrl && !k.shift && !k.alt {
                // A held Escape is one press: its autorepeats are neither the second Escape nor a reset.
                if k.repeat {
                    return false;
                }
                let now = u64::try_from(keys.clock.elapsed().as_millis()).unwrap_or(u64::MAX);
                return keys.esc.borrow_mut().escape(now);
            }
            keys.esc.borrow_mut().other_key();
            app_takes(k, &keys.ctx.get())
        })
    }

    /// The browser, made now if it is not (None: this build cannot make one).
    fn backend(&mut self) -> Option<&mut Box<dyn WebBackend>> {
        if self.closed {
            return None;
        }
        if self.backend.is_none() {
            let sink = self.sink();
            let filter = self.key_filter();
            let f = self.factory.as_mut()?;
            let mut b = f(sink);
            b.set_key_filter(filter);
            self.backend = Some(b);
        }
        self.backend.as_mut()
    }

    /// How many controllers exist (0 while no web panel is open).
    pub fn controllers(&self) -> usize {
        self.backend.as_ref().map_or(0, |b| b.controllers())
    }

    /// True once the browser object exists (it is made by the first web panel).
    pub fn has_backend(&self) -> bool {
        self.backend.is_some()
    }

    pub fn has_focus(&self) -> bool {
        self.has_focus
    }

    pub fn tab_url(&self, tab: &str) -> Option<&str> {
        self.tabs.get(tab).map(|t| t.url.as_str())
    }

    /// Gives the page of the tab's open web panel the keys.
    pub fn focus_page(&mut self, tab: &str) -> bool {
        if !self.tabs.get(tab).is_some_and(|t| t.ready) {
            return false;
        }
        match self.backend() {
            Some(b) => {
                b.focus(tab);
                true
            }
            None => false,
        }
    }

    /// The window gets the keyboard back from the page (a click on the terminal, the chord, Escape).
    pub fn release_focus(&mut self, hwnd: Option<isize>) {
        if !self.has_focus {
            return;
        }
        self.has_focus = false;
        if let Some(h) = hwnd {
            // SAFETY: SetFocus on Flare's own live window, from its own thread.
            unsafe {
                windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus(h as *mut _);
            }
        }
    }

    /// The window is ending (rule 28): every call still waiting is answered, every view is closed and
    /// the browser's environment is let go now, while the window and the COM apartment are still up,
    /// so no browser callback lands during the teardown. Nothing opens again afterwards.
    pub fn close_all(&mut self) {
        self.closed = true;
        crate::timer_registry::clear(CAPTURE_TIMER);
        crate::timer_registry::clear(AGENT_TIMER);
        for (_, t) in self.tabs.drain() {
            if let Some(p) = t.probe {
                let _ = p
                    .reply
                    .send(Err(CtlError::new(code::UNAVAILABLE, "Flare is closing")));
            }
            if let Some(p) = t.pending {
                let _ = p
                    .reply
                    .send(Err(CtlError::new(code::UNAVAILABLE, "Flare is closing")));
            }
            if let Some(a) = t.agent {
                let _ = a
                    .reply
                    .send(Err(CtlError::new(code::UNAVAILABLE, "Flare is closing")));
            }
        }
        self.keys.job_active.set(false);
        self.keys.agent_keys.set(false);
        if let Some(mut b) = self.backend.take() {
            b.close_all();
        }
        self.shown_tab = None;
        self.phase = Phase::Hidden;
        self.still = None;
        self.bounds = None;
        self.has_focus = false;
    }

    /// The person's answer to a download question.
    pub fn answer_download(&mut self, id: u64, save: bool) {
        if let Some(b) = self.backend.as_mut() {
            b.answer_download(id, save);
        }
    }

    /// The tab's page cannot be shown: its window is taken off the screen (a page that crashed would
    /// draw its own sad face over the note) and the keyboard goes back to Flare.
    fn fail_view(&mut self, tab: &str) {
        if self.has_focus && self.shown_tab.as_deref() == Some(tab) {
            let hwnd = self.hwnd;
            self.release_focus(hwnd);
        }
        self.hide(tab);
    }

    // ---- the window's properties ---------------------------------------------------------------

    /// Follows the window: opens, moves, shows, hides and closes the tab's browser, and pushes what
    /// the web panel shows into the window's properties. `front` is the front shell tab.
    pub fn sync(
        &mut self,
        app: &AppWindow,
        panels: &PanelCtl,
        front: Option<&str>,
        key_ctx: KeyContext,
    ) {
        self.keys.ctx.set(key_ctx);
        self.hwnd = crate::window::hwnd_of(app);
        // A tab whose web panel is closed (or gone) has no browser.
        let stale: Vec<String> = self
            .tabs
            .keys()
            .filter(|t| panels.web_target(t).is_none())
            .cloned()
            .collect();
        for t in stale {
            self.close_tab(&t);
        }
        // A tab whose panel now shows a different mini-app (or a page where an app was, or the other way
        // round) has a view made for the wrong thing: its page is replaced in the same browser.
        let replaced: Vec<(String, String)> = self
            .tabs
            .iter()
            .filter(|(t, w)| w.app_host.as_deref() != panels.app_spec(t).map(|a| a.host.as_str()))
            .filter_map(|(t, _)| panels.web_target(t).map(|u| (t.clone(), u)))
            .collect();
        for (t, url) in replaced {
            self.replace_tab(&t, &url, panels.app_spec(&t).cloned());
        }
        let front_web = front.filter(|t| panels.web_target(t).is_some());
        if front_web.is_some() && app.get_panel_kind() != 3 {
            // The bar is part of the head of a web panel: the page's rectangle is known only once it is.
            app.set_panel_kind(3);
        }
        let shown = app.get_panel_shown();
        let overlay = app.get_overlay_open();
        let scale = app.get_scale();
        let rect = phys_rect(
            app.get_panel_web_x(),
            app.get_panel_web_y(),
            app.get_panel_web_w(),
            app.get_panel_web_h(),
            scale,
        );

        // The tab in front gets its browser the first time it has a place to be.
        if let (Some(tab), true) = (front_web, shown && !rect.is_empty())
            && !self.tabs.contains_key(tab)
            && let Some(url) = panels.web_target(tab)
        {
            self.open_tab(tab, &url, rect, false, panels.app_spec(tab).cloned());
        }

        // Which tab's window is on the screen.
        let want_tab = front_web.filter(|t| self.tabs.contains_key(*t));
        if self.shown_tab.as_deref() != want_tab {
            if let Some(old) = self.shown_tab.take() {
                // The page that had the keyboard goes out of sight: the window takes it back.
                if self.has_focus {
                    self.release_focus(self.hwnd);
                }
                self.hide(&old);
            }
            self.phase = Phase::Hidden;
            self.drop_still(app);
            self.shown_tab = want_tab.map(str::to_string);
        }
        // A page that failed has its window off the screen: the note is what shows.
        let want = match &self.shown_tab {
            Some(t) if !self.tabs.get(t).is_some_and(|w| w.failed.is_some()) => {
                web_show(true, shown, overlay)
            }
            _ => WebShow::Hidden,
        };
        if let Some(tab) = self.shown_tab.clone() {
            // The window follows the slot: only a change is sent.
            if !rect.is_empty() && self.bounds.as_ref() != Some(&(tab.clone(), rect)) {
                self.bounds = Some((tab.clone(), rect));
                if let Some(b) = self.backend() {
                    b.set_bounds(&tab, rect);
                }
            }
            self.token += 1;
            let (phase, acts) = step(self.phase, want, self.token);
            self.phase = phase;
            for a in acts {
                self.act(app, &tab, a);
            }
        } else {
            self.bounds = None;
        }
        self.push(app, panels, front_web);
    }

    fn open_tab(
        &mut self,
        tab: &str,
        url: &str,
        rect: PhysRect,
        visible: bool,
        app: Option<AppSpec>,
    ) {
        if self.closed {
            return;
        }
        self.tabs.insert(
            tab.to_string(),
            TabWeb {
                url: url.to_string(),
                app_host: app.as_ref().map(|a| a.host.clone()),
                ..TabWeb::default()
            },
        );
        match self.backend() {
            Some(b) => match app {
                Some(a) => b.open_app(tab, a, rect, visible),
                None => b.open(tab, url, rect, visible),
            },
            None => {
                if let Some(t) = self.tabs.get_mut(tab) {
                    t.failed = Some(("this build has no web view".to_string(), false));
                }
            }
        }
    }

    fn act(&mut self, app: &AppWindow, tab: &str, a: Act) {
        match a {
            Act::Show => {
                if let Some(b) = self.backend() {
                    b.set_visible(tab, true);
                }
            }
            Act::Hide => {
                if let Some(b) = self.backend() {
                    b.set_visible(tab, false);
                }
            }
            Act::Capture(token) => {
                // A page that is busy or hung never answers: the timeout stands in for the answer (a
                // still of nothing), so its window cannot stay on top of the overlay.
                let (post, name) = (std::sync::Arc::clone(&self.post), tab.to_string());
                crate::timer_registry::set(CAPTURE_TIMER, CAPTURE_TIMEOUT_MS, false, move || {
                    post(UiMsg::Web(WebMsgBox(WebMsg::Event(WebEvent::Still {
                        tab: name.clone(),
                        token,
                        png: None,
                    }))));
                });
                if let Some(b) = self.backend() {
                    b.capture_still(tab, token);
                }
            }
            Act::DropStill => self.drop_still(app),
        }
    }

    fn hide(&mut self, tab: &str) {
        if let Some(b) = self.backend.as_mut() {
            b.set_visible(tab, false);
        }
    }

    fn drop_still(&mut self, app: &AppWindow) {
        if self.still.take().is_some() {
            app.set_panel_web_still(Image::default());
        }
        app.set_panel_web_has_still(false);
        self.ended_capture = None;
    }

    /// Pushes what the web panel shows. Only the front tab's web panel owns the window's `panel-*`
    /// properties; a front tab with any other panel leaves them to `PanelCtl`.
    fn push(&mut self, app: &AppWindow, panels: &PanelCtl, front_web: Option<&str>) {
        let Some(tab) = front_web else {
            if app.get_panel_kind() == 3 {
                app.set_panel_kind(0);
                app.set_panel_title("".into());
                app.set_panel_note("".into());
            }
            app.set_panel_web_app(false);
            app.set_panel_web_url("".into());
            app.set_panel_web_loading(false);
            app.set_panel_web_can_back(false);
            app.set_panel_web_can_forward(false);
            app.set_panel_web_missing_runtime(false);
            app.set_panel_web_ribbon("".into());
            return;
        };
        let target = panels.web_target(tab).unwrap_or_default();
        let t = self.tabs.get(tab);
        let url = t.map_or(target.as_str(), |t| {
            if t.url.is_empty() {
                target.as_str()
            } else {
                t.url.as_str()
            }
        });
        // A mini-app is named by the agent's title (its page's own is not the panel's name).
        let title = if panels.is_app(tab) {
            panels.snap(tab).map(|s| s.title).unwrap_or_default()
        } else {
            t.map(|t| t.title.as_str())
                .filter(|s| !s.is_empty())
                .map_or_else(|| host_label(url), str::to_string)
        };
        app.set_panel_kind(3);
        app.set_panel_web_app(panels.is_app(tab));
        app.set_panel_title(title.into());
        app.set_panel_sub(t.map_or("", |t| t.notice.as_str()).into());
        let (note, missing) = match t.and_then(|t| t.failed.as_ref()) {
            Some((_, true)) => (runtime_missing_note(), true),
            Some((why, false)) => (format!("The page could not be shown: {why}"), false),
            None => (String::new(), false),
        };
        app.set_panel_note(note.into());
        app.set_panel_web_missing_runtime(missing);
        app.set_panel_web_url(url.into());
        app.set_panel_web_loading(t.is_some_and(|t| t.loading));
        app.set_panel_web_can_back(t.is_some_and(|t| t.can_back));
        app.set_panel_web_can_forward(t.is_some_and(|t| t.can_forward));
        app.set_panel_web_ribbon(self.ribbon_text(tab).into());
    }

    /// Forgets what is known of the tab's page and answers every call that waited on it. True when the
    /// tab had a page (its browser view is the caller's to close).
    fn forget_tab(&mut self, tab: &str) -> bool {
        let Some(t) = self.tabs.remove(tab) else {
            return false;
        };
        if let Some(p) = t.probe {
            let _ = p.reply.send(Err(CtlError::new(
                code::UNAVAILABLE,
                "the panel was closed before the diagram was drawn",
            )));
        }
        if let Some(p) = t.pending {
            let _ = p.reply.send(Err(CtlError::new(
                code::UNAVAILABLE,
                "the page was closed before it finished loading",
            )));
        }
        if let Some(a) = t.agent {
            let _ = a.reply.send(Err(CtlError::new(
                code::UNAVAILABLE,
                "the page was closed while the call was running",
            )));
        }
        self.refresh_key_flags();
        true
    }

    /// The tab's panel now shows something of another kind (a mini-app where a page was, another
    /// mini-app, a page where an app was): its page is replaced in the same browser. The browser view is
    /// not closed here (the host swaps it itself and keeps its environment, so a mini-app updated again
    /// and again does not restart the browser), but everything known of the old page goes and the
    /// window's phase starts again for the new one.
    pub fn replace_tab(&mut self, tab: &str, url: &str, app: Option<AppSpec>) {
        if self.shown_tab.as_deref() == Some(tab) && self.has_focus {
            let hwnd = self.hwnd;
            self.release_focus(hwnd);
            self.hide(tab);
        }
        self.forget_tab(tab);
        if self.shown_tab.as_deref() == Some(tab) {
            self.shown_tab = None;
            self.phase = Phase::Hidden;
            self.still = None;
            crate::timer_registry::clear(CAPTURE_TIMER);
        }
        let rect = self.bounds.as_ref().map_or(
            PhysRect {
                x: 0,
                y: 0,
                w: 800,
                h: 600,
            },
            |(_, r)| *r,
        );
        self.open_tab(tab, url, rect, false, app);
    }

    /// Frees the tab's browser.
    pub fn close_tab(&mut self, tab: &str) {
        // The page that has the keyboard is closed under it: Windows would leave the focus nowhere
        // (every key then goes to the active window as a system key and the terminal types nothing).
        // The window takes it back first, and the window comes off the screen before it is closed.
        if self.shown_tab.as_deref() == Some(tab) && self.has_focus {
            let hwnd = self.hwnd;
            self.release_focus(hwnd);
            self.hide(tab);
        }
        if self.forget_tab(tab)
            && let Some(b) = self.backend.as_mut()
        {
            b.close(tab);
        }
        if self.shown_tab.as_deref() == Some(tab) {
            self.shown_tab = None;
            self.phase = Phase::Hidden;
            self.still = None;
            crate::timer_registry::clear(CAPTURE_TIMER);
        }
        if self.tabs.is_empty() {
            // No web panel is open: the browser goes (it is made again by the next one).
            self.has_focus = false;
            self.bounds = None;
            self.backend = None;
        }
    }

    // ---- what the user and the channel ask ---------------------------------------------------------

    /// A page whose renderer died (not one that could never start) is let go, so the next sync makes it
    /// again at the panel's address. True when there was one: the caller refreshes. Without this the dead
    /// page stayed, hidden behind its note, still driven by Reload and the address bar.
    pub fn recover(&mut self, tab: &str) -> bool {
        let lost = self
            .tabs
            .get(tab)
            .and_then(|t| t.failed.as_ref())
            .is_some_and(|(_, missing)| !missing);
        if lost {
            self.close_tab(tab);
        }
        lost
    }

    /// The address field was submitted. The error is the words to show.
    pub fn submit(&mut self, tab: &str, text: &str) -> Result<String, UrlError> {
        let url = normalize_url(text)?;
        self.go_to(tab, &url);
        Ok(url)
    }

    /// Sends the tab's page to `url` (already normalised).
    pub fn go_to(&mut self, tab: &str, url: &str) {
        if let Some(t) = self.tabs.get_mut(tab) {
            t.notice.clear();
            t.url = url.to_string();
        }
        if self.tabs.contains_key(tab)
            && let Some(b) = self.backend()
        {
            b.navigate(tab, url);
        }
    }

    /// A line for the header: why the last thing asked of the page did not happen.
    pub fn notice(&mut self, tab: &str, text: &str) {
        if let Some(t) = self.tabs.get_mut(tab) {
            t.notice = text.to_string();
        }
    }

    /// The window moved or changed size or monitor: every live page is told where its parent is now.
    pub fn parent_moved(&mut self) {
        if !self.tabs.is_empty()
            && let Some(b) = self.backend.as_mut()
        {
            b.parent_moved();
        }
    }

    pub fn nav(&mut self, tab: &str, nav: Nav) {
        if let Some(b) = self.backend.as_mut() {
            b.go(tab, nav);
        }
    }

    pub fn can_go(&self, tab: &str, nav: Nav) -> bool {
        self.tabs.get(tab).is_some_and(|t| match nav {
            Nav::Back => t.can_back,
            Nav::Forward => t.can_forward,
            Nav::Reload | Nav::Stop => true,
        })
    }

    /// A channel call waits for the tab's next finished navigation. A call that was waiting is told
    /// it was replaced.
    pub fn wait_for_load(
        &mut self,
        tab: &str,
        reply: Sender<Result<Value, CtlError>>,
        deadline: Instant,
        url: &str,
    ) {
        self.wait_for_load_then(tab, reply, deadline, url, false);
    }

    /// [`WebCtl::wait_for_load`], and for a `diagram` the answer is the page's own word on whether the
    /// drawing worked (its error text when it did not), asked once the page has loaded.
    pub fn wait_for_load_then(
        &mut self,
        tab: &str,
        reply: Sender<Result<Value, CtlError>>,
        deadline: Instant,
        url: &str,
        diagram: bool,
    ) {
        let Some(t) = self.tabs.get_mut(tab) else {
            let _ = reply.send(Err(CtlError::new(
                code::UNAVAILABLE,
                "there is no web page in this tab",
            )));
            return;
        };
        let pending = Pending {
            reply,
            deadline,
            url: url.to_string(),
            nav: None,
            diagram,
        };
        if let Some(old) = t.pending.replace(pending) {
            let _ = old.reply.send(Err(CtlError::new(
                code::UNAVAILABLE,
                "another call sent the page somewhere else",
            )));
        }
        t.last = None;
    }

    /// The tab has a browser that is up (or on its way) and has not failed.
    pub fn has_tab(&self, tab: &str) -> bool {
        self.tabs.contains_key(tab)
    }

    /// Makes the browser of a tab that is behind, hidden, because an agent call needs its page (only the
    /// front tab gets one otherwise). It takes the place of the last page shown, and is put right when its
    /// tab comes to the front.
    pub fn ensure_hidden(&mut self, tab: &str, url: &str, app: Option<AppSpec>) {
        if self.closed || self.tabs.contains_key(tab) {
            return;
        }
        let rect = self.bounds.as_ref().map_or(
            PhysRect {
                x: 0,
                y: 0,
                w: 800,
                h: 600,
            },
            |(_, r)| *r,
        );
        self.open_tab(tab, url, rect, false, app);
    }

    pub fn failure(&self, tab: &str) -> Option<(String, bool)> {
        self.tabs.get(tab).and_then(|t| t.failed.clone())
    }

    /// What the tab's page is, for a call that has nothing to wait for.
    pub fn snapshot(&self, tab: &str) -> Settled {
        self.tabs.get(tab).map_or(
            Settled {
                url: String::new(),
                title: String::new(),
                loaded: false,
                status: String::new(),
            },
            |t| Settled {
                url: t.url.clone(),
                title: t.title.clone(),
                loaded: !t.loading,
                status: String::new(),
            },
        )
    }

    // ---- the agent (stage 4) ---------------------------------------------------------------------------

    fn now_ms(&self) -> u64 {
        u64::try_from(self.keys.clock.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// The ribbon's words for the tab's page: while an agent call runs on it and for a moment after.
    pub fn ribbon_text(&self, tab: &str) -> String {
        let Some(t) = self.tabs.get(tab) else {
            return String::new();
        };
        if let Some(a) = &t.agent {
            return ribbon_words(&a.label);
        }
        match &t.hold {
            Some((label, until)) if self.now_ms() < *until => ribbon_words(label),
            _ => String::new(),
        }
    }

    /// True while an agent call runs on any page.
    pub fn agent_busy(&self) -> bool {
        self.tabs.values().any(|t| t.agent.is_some())
    }

    /// The operation of the agent call on the tab's page, if one runs (for the e2e).
    pub fn agent_op(&self, tab: &str) -> Option<&'static str> {
        self.tabs.get(tab)?.agent.as_ref().map(|a| a.job.op())
    }

    /// How many element names the agent's last look left on the tab's page (for the e2e).
    pub fn agent_refs(&self, tab: &str) -> usize {
        self.tabs.get(tab).map_or(0, |t| t.refs.len())
    }

    /// How many keys the hook heard while the agent's own press was in flight (for the e2e).
    pub fn agent_keys_heard(&self) -> u32 {
        self.keys.agent_heard.get()
    }

    /// How many agent calls run and how many ribbons are held: when this is the same before and after a
    /// message, the window's properties need no new sync (a DevTools answer or a timer turn alone).
    pub fn agent_shape(&self) -> (usize, usize) {
        (
            self.tabs.values().filter(|t| t.agent.is_some()).count(),
            self.tabs.values().filter(|t| t.hold.is_some()).count(),
        )
    }

    fn refresh_key_flags(&self) {
        self.keys.job_active.set(self.agent_busy());
        self.keys.agent_keys.set(
            self.tabs
                .values()
                .any(|t| t.agent.as_ref().is_some_and(|a| a.job.op() == "press")),
        );
    }

    fn ensure_agent_timer(&self) {
        if crate::timer_registry::running(AGENT_TIMER) {
            return;
        }
        let post = std::sync::Arc::clone(&self.post);
        crate::timer_registry::set(AGENT_TIMER, AGENT_TICK_MS, true, move || {
            post(UiMsg::Web(WebMsgBox(WebMsg::AgentTick)));
        });
    }

    /// An agent call on the tab's page (`browser.look`, `click`, ...). The answer goes to `reply` when the
    /// work is done, the person takes over, the deadline passes or the page goes. `label` names the agent
    /// for the ribbon.
    pub fn start_agent(
        &mut self,
        tab: &str,
        op: AgentOp,
        label: &str,
        reply: Sender<Result<Value, CtlError>>,
    ) {
        let say = |m: &str| {
            let _ = reply.send(Err(CtlError::new(code::UNAVAILABLE, m.to_string())));
        };
        let now = self.now_ms();
        let Some(t) = self.tabs.get_mut(tab) else {
            // The tab has a web panel (the caller checked) but its page is not made: its tab is behind.
            say("the page is not ready yet; try again in a moment");
            return;
        };
        if let Some((why, missing)) = &t.failed {
            say(&if *missing {
                format!("the WebView2 Runtime is not installed ({RUNTIME_URL})")
            } else {
                format!("the web view could not be made: {why}")
            });
            return;
        }
        if !t.ready {
            say("the page is not ready yet; try again in a moment");
            return;
        }
        if t.agent.is_some() {
            say("another browser call is still running on this page; wait for it");
            return;
        }
        // A page that is not on screen (its tab is behind, a Flare menu covers it) paints nothing, so no
        // picture of it can be taken; its list can.
        let op = match op {
            AgentOp::Look { .. } => AgentOp::Look {
                // Only a page that is painting has a picture: not one collapsed, covered by a menu or
                // in a minimized window (each would wait out the whole picture budget).
                picture: self.shown_tab.as_deref() == Some(tab)
                    && matches!(self.phase, Phase::Live | Phase::Capturing(_))
                    && !self.hwnd.is_some_and(window_minimized),
            },
            other => other,
        };
        let (job, out) = Job::start(op, &t.refs, now, &mut Ids(&mut self.call_ids));
        let Some(job) = job else {
            // Refused before it began (a bad ref, a bad point): nothing was sent to the page.
            if let JobOut::Done(r) = out {
                let _ = reply.send(r.map_err(ctl_error));
            }
            return;
        };
        t.agent = Some(AgentCall {
            job,
            reply,
            label: label.to_string(),
            doc: t.doc,
        });
        t.hold = None;
        flush_button_edges();
        self.refresh_key_flags();
        self.ensure_agent_timer();
        self.drive(tab, out);
    }

    /// Carries out what a job asked.
    fn drive(&mut self, tab: &str, out: JobOut) {
        match out {
            JobOut::Idle => {}
            JobOut::Calls(calls) => {
                for c in calls {
                    let params = c.params.to_string();
                    if let Some(b) = self.backend.as_mut() {
                        b.cdp(tab, c.id, &c.method, &params);
                    }
                }
            }
            JobOut::Done(r) => self.finish_agent(tab, r),
        }
    }

    /// The call is over: its answer is sent, a look's element names are kept, the ribbon holds a moment.
    fn finish_agent(&mut self, tab: &str, result: Result<Value, JobError>) {
        let now = self.now_ms();
        let Some(t) = self.tabs.get_mut(tab) else {
            return;
        };
        let Some(mut a) = t.agent.take() else {
            return;
        };
        // The names of a look are kept only when the page is still the document the look read (a
        // navigation that began meanwhile has cleared them already, and they belong to the old page).
        if let Some(refs) = a.job.take_refs()
            && a.doc == t.doc
        {
            t.refs = refs;
        }
        // A press the call was cut short in is let go of, so the page does not think a button is held.
        if result.is_err() {
            for c in a.job.release_calls(&mut Ids(&mut self.call_ids)) {
                if let Some(b) = self.backend.as_mut() {
                    b.cdp(tab, c.id, &c.method, &c.params.to_string());
                }
            }
        }
        let interrupted = matches!(&result, Err(e) if e.fail == Fail::Interrupted);
        if interrupted {
            // The page is the person's now: what the agent last saw may no longer be there.
            t.refs = RefMap::default();
            t.hold = None;
        } else {
            t.hold = Some((a.label.clone(), now + RIBBON_HOLD_MS));
        }
        let _ = a.reply.send(result.map_err(ctl_error));
        self.refresh_key_flags();
    }

    /// The person pressed a key or a button in the page in front: an agent call there ends, `interrupted`.
    pub fn user_input(&mut self) {
        let Some(tab) = self.shown_tab.clone() else {
            return;
        };
        if self.tabs.get(&tab).is_some_and(|t| t.agent.is_some()) {
            self.finish_agent(&tab, Err(Job::interrupted()));
        } else if let Some(t) = self.tabs.get_mut(&tab) {
            t.hold = None;
        }
    }

    /// One turn of the agent timer. False when nothing is left for it to watch.
    fn agent_tick(&mut self) -> bool {
        let now = self.now_ms();
        // The person's mouse: a button down over the page in front while the agent is working there.
        if let (Some(hwnd), Some((tab, rect))) = (self.hwnd, self.bounds.clone())
            && self.tabs.get(&tab).is_some_and(|t| t.agent.is_some())
            && self.shown_tab.as_deref() == Some(tab.as_str())
            && self.phase == Phase::Live
            && real_button_down_over(hwnd, &rect)
        {
            self.user_input();
        }
        // The person's keyboard: a key typed while the page has the keys and the agent is working in it.
        if self.has_focus
            && let Some(tab) = self.shown_tab.clone()
            && self.tabs.get(&tab).is_some_and(|t| t.agent.is_some())
            && self.phase == Phase::Live
            && real_key_pressed()
        {
            self.user_input();
        }
        let busy: Vec<String> = self
            .tabs
            .iter()
            .filter(|(_, t)| t.agent.is_some())
            .map(|(k, _)| k.clone())
            .collect();
        for tab in busy {
            let out = self.tabs.get_mut(&tab).and_then(|t| {
                t.agent
                    .as_mut()
                    .map(|a| a.job.on_tick(now, &mut Ids(&mut self.call_ids)))
            });
            if let Some(out) = out {
                self.drive(&tab, out);
            }
        }
        for t in self.tabs.values_mut() {
            if t.hold.as_ref().is_some_and(|(_, until)| now >= *until) {
                t.hold = None;
            }
        }
        self.agent_busy() || self.tabs.values().any(|t| t.hold.is_some())
    }

    /// A DevTools call answered.
    fn on_cdp(&mut self, tab: &str, id: u64, result: Result<String, String>) {
        let now = self.now_ms();
        let Some(t) = self.tabs.get_mut(tab) else {
            return;
        };
        let (url, title) = (t.url.clone(), t.title.clone());
        let Some(a) = t.agent.as_mut() else {
            return;
        };
        let page = Page {
            url: &url,
            title: &title,
        };
        let out = a
            .job
            .on_reply(id, result, page, now, &mut Ids(&mut self.call_ids));
        self.drive(tab, out);
    }

    // ---- the e2e's seams -------------------------------------------------------------------------------

    /// Sets the browser's zoom of the tab's page (the e2e's seam).
    pub fn set_zoom(&mut self, tab: &str, factor: f64) {
        if let Some(b) = self.backend() {
            b.set_zoom(tab, factor);
        }
    }

    /// Starts a script in the tab's page; the answer is read with [`WebCtl::script_result`].
    pub fn start_script(&mut self, tab: &str, js: &str) -> u64 {
        self.script_seq += 1;
        let id = self.script_seq;
        self.scripts.remove(&id);
        match self.backend() {
            Some(b) => b.run_script(tab, id, js),
            None => {
                self.scripts.insert(id, Err("no web view".to_string()));
            }
        }
        id
    }

    /// The answer of the latest script started, None while it runs.
    pub fn script_latest(&self) -> Option<Result<String, String>> {
        self.scripts.get(&self.script_seq).cloned()
    }

    pub fn probe(&self, tab: &str) -> Option<flare_web::ControllerProbe> {
        self.backend.as_ref().and_then(|b| b.probe(tab))
    }

    pub fn phase_name(&self) -> &'static str {
        match self.phase {
            Phase::Hidden => "hidden",
            Phase::Live => "live",
            Phase::Capturing(_) => "capturing",
            Phase::Still => "still",
        }
    }

    /// The tabs that have a browser, sorted.
    pub fn tab_ids(&self) -> Vec<String> {
        let mut v: Vec<String> = self.tabs.keys().cloned().collect();
        v.sort();
        v
    }

    pub fn shown_tab(&self) -> Option<&str> {
        self.shown_tab.as_deref()
    }

    pub fn has_still(&self) -> bool {
        self.still.is_some()
    }

    // ---- the browser's messages --------------------------------------------------------------------------

    pub fn on_msg(&mut self, app: &AppWindow, msg: WebMsg) -> Vec<WebOut> {
        match msg {
            WebMsg::Event(e) => self.on_event(app, e),
            WebMsg::Decoded { tab, token, image } => {
                if self.shown_tab.as_deref() != Some(tab.as_str()) {
                    return vec![];
                }
                // The window was already hidden by the capture's answer; the picture now fills its place.
                if self.phase == Phase::Still
                    && token == self.still_token
                    && let Some(buf) = image
                {
                    let img = Image::from_rgba8(buf);
                    app.set_panel_web_still(img.clone());
                    app.set_panel_web_has_still(true);
                    self.still = Some(img);
                }
                vec![]
            }
            WebMsg::UserInput => {
                self.user_input();
                vec![]
            }
            WebMsg::AgentTick => {
                if !self.agent_tick() {
                    crate::timer_registry::clear(AGENT_TIMER);
                }
                vec![]
            }
        }
    }

    /// The tab's view shows a mod's page (stage 8).
    fn is_mod_tab(&self, tab: &str) -> bool {
        self.tabs
            .get(tab)
            .and_then(|t| t.app_host.as_deref())
            .is_some_and(|h| flare_surfaces::modview::mod_of_host(h).is_some())
    }

    /// Speaks to a mod's page: `json` is delivered to its bridge. Nothing happens for a tab with no such page.
    pub fn post_message(&mut self, tab: &str, json: &str) {
        if self.is_mod_tab(tab)
            && let Some(b) = self.backend.as_mut()
        {
            b.post_message(tab, json);
        }
    }

    /// The mod page's own state, told to the window after the event is handled (the tab is read before the
    /// event can drop it).
    fn mod_note(&self, e: &WebEvent) -> Option<WebOut> {
        match e {
            WebEvent::Loading { tab, .. } if self.is_mod_tab(tab) => Some(WebOut::ModPage {
                tab: tab.clone(),
                ready: false,
            }),
            // Only the load of a navigation the host allowed counts: a navigation it cancelled (a link out of the
            // mod's own host) still ends with a failed load, and the page that is open is untouched by it.
            WebEvent::Loaded { tab, ok, nav, .. }
                if self.is_mod_tab(tab)
                    && self.tabs.get(tab).and_then(|t| t.started_nav) == Some(*nav) =>
            {
                Some(WebOut::ModPage {
                    tab: tab.clone(),
                    ready: *ok,
                })
            }
            WebEvent::Failed { tab, .. } | WebEvent::Gone { tab, .. } if self.is_mod_tab(tab) => {
                Some(WebOut::ModLost { tab: tab.clone() })
            }
            _ => None,
        }
    }

    fn on_event(&mut self, app: &AppWindow, e: WebEvent) -> Vec<WebOut> {
        let mut out = Vec::new();
        let mod_note = self.mod_note(&e);
        // Set when a diagram page has loaded: the question to put to it once the borrow of the tab ends.
        let mut diagram_ask: Option<DiagramAsk> = None;
        match e {
            WebEvent::Pump => {
                if let Some(b) = self.backend.as_mut() {
                    b.pump();
                }
            }
            WebEvent::Runtime(r) => {
                if let Some(b) = self.backend.as_mut() {
                    b.runtime_checked(r);
                }
            }
            WebEvent::Ready { tab } => {
                if let Some(t) = self.tabs.get_mut(&tab) {
                    t.ready = true;
                    t.failed = None;
                }
                out.push(WebOut::Changed);
            }
            WebEvent::Failed {
                tab,
                why,
                missing_runtime,
            } => {
                self.fail_view(&tab);
                self.end_agent(&tab, "the web view could not be made");
                self.end_probe(&tab, "the web view could not be made");
                if let Some(t) = self.tabs.get_mut(&tab) {
                    t.failed = Some((why.clone(), missing_runtime));
                    if let Some(p) = t.pending.take() {
                        let msg = if missing_runtime {
                            format!("the WebView2 Runtime is not installed ({RUNTIME_URL})")
                        } else {
                            format!("the web view could not be made: {why}")
                        };
                        out.push(WebOut::Settle {
                            reply: p.reply,
                            tab: tab.clone(),
                            result: Err(CtlError::new(code::UNAVAILABLE, msg)),
                        });
                    }
                }
                out.push(WebOut::Changed);
            }
            WebEvent::Loading { tab, url, nav } => {
                if let Some(t) = self.tabs.get_mut(&tab) {
                    t.started_nav = Some(nav);
                    t.loading = true;
                    // The page moved on while a diagram's question was out.
                    if let Some(p) = t.probe.take() {
                        let _ = p.reply.send(Err(CtlError::new(
                            code::UNAVAILABLE,
                            "the page changed before the diagram was drawn",
                        )));
                    }
                    // A new page: the names of the last look point at nothing now.
                    t.refs = RefMap::default();
                    t.doc += 1;
                    t.notice.clear();
                    // The call's own navigation is the first for the address it sent the page to (an
                    // earlier one still on its way is not it); a redirect after that rebinds it.
                    if let Some(p) = t.pending.as_mut()
                        && (p.nav.is_some() || p.url.is_empty() || url_matches(&p.url, &url, false))
                    {
                        p.nav = Some(nav);
                    }
                    t.url = url;
                }
            }
            WebEvent::Loaded {
                tab,
                url,
                ok,
                status,
                nav,
            } => {
                if let Some(t) = self.tabs.get_mut(&tab) {
                    // Only the latest navigation ends the loading; an older one finishing late does not.
                    let latest = t
                        .pending
                        .as_ref()
                        .is_none_or(|p| p.nav.is_none_or(|n| n == nav));
                    if latest {
                        t.loading = false;
                    }
                    if !url.is_empty() {
                        t.url = url.clone();
                    }
                    let settled = Settled {
                        url: t.url.clone(),
                        title: t.title.clone(),
                        loaded: ok,
                        status,
                    };
                    t.last = Some(settled.clone());
                    if t.pending.as_ref().is_some_and(|p| p.nav == Some(nav)) {
                        let p = t.pending.take().expect("checked just above");
                        // A call whose caller has given up (past its deadline) is let go.
                        if Instant::now() <= p.deadline {
                            if p.diagram && ok {
                                // Loaded is not drawn: the page is asked, and its answer is the call's.
                                diagram_ask = Some((tab.clone(), p.reply, settled));
                            } else if p.diagram {
                                out.push(WebOut::Settle {
                                    reply: p.reply,
                                    tab: tab.clone(),
                                    result: Err(CtlError::new(
                                        code::UNAVAILABLE,
                                        "the diagram page could not be loaded",
                                    )),
                                });
                            } else {
                                out.push(WebOut::Settle {
                                    reply: p.reply,
                                    tab: tab.clone(),
                                    result: Ok(settled),
                                });
                            }
                        }
                    }
                    if ok && !t.url.is_empty() {
                        out.push(WebOut::Address {
                            tab,
                            url: t.url.clone(),
                        });
                    }
                }
            }
            WebEvent::Url { tab, url } => {
                if let Some(t) = self.tabs.get_mut(&tab)
                    && !url.is_empty()
                {
                    t.url = url.clone();
                    out.push(WebOut::Address { tab, url });
                }
            }
            WebEvent::SameDocument { tab, url } => {
                if let Some(t) = self.tabs.get_mut(&tab)
                    && !url.is_empty()
                {
                    t.url.clone_from(&url);
                    // No navigation is raised for it: a call that sent the page to exactly this address
                    // (a `#fragment` of the page it is on) is answered here, and so is a back, forward or reload
                    // (no address) that stepped through the history inside one document.
                    let wanted = t.pending.as_ref().is_some_and(|p| {
                        p.nav.is_none() && (p.url.is_empty() || url_matches(&p.url, &url, true))
                    });
                    if wanted {
                        let p = t.pending.take().expect("checked just above");
                        let settled = Settled {
                            url: url.clone(),
                            title: t.title.clone(),
                            loaded: true,
                            status: String::new(),
                        };
                        t.last = Some(settled.clone());
                        t.loading = false;
                        if Instant::now() <= p.deadline {
                            out.push(WebOut::Settle {
                                reply: p.reply,
                                tab: tab.clone(),
                                result: Ok(settled),
                            });
                        }
                    }
                    out.push(WebOut::Address { tab, url });
                }
            }
            WebEvent::Download {
                tab,
                id,
                file,
                host,
            } => {
                // A download of a page that is not here any more is not asked about.
                if self.tabs.contains_key(&tab) {
                    out.push(WebOut::Download {
                        tab,
                        id,
                        file,
                        host,
                    });
                } else if let Some(b) = self.backend.as_mut() {
                    b.answer_download(id, false);
                }
            }
            WebEvent::Title { tab, title } => {
                if let Some(t) = self.tabs.get_mut(&tab) {
                    t.title.clone_from(&title);
                    out.push(WebOut::Title { tab, title });
                }
            }
            WebEvent::History {
                tab,
                can_back,
                can_forward,
            } => {
                if let Some(t) = self.tabs.get_mut(&tab) {
                    t.can_back = can_back;
                    t.can_forward = can_forward;
                }
            }
            WebEvent::Refused { tab, url } => {
                if let Some(t) = self.tabs.get_mut(&tab) {
                    let shown: String = url.chars().take(60).collect();
                    t.notice = format!("Not opened here: {shown}");
                    t.loading = false;
                    // A call waiting for a page that was sent somewhere it may not go is answered.
                    if let Some(p) = t.pending.take() {
                        out.push(WebOut::Settle {
                            reply: p.reply,
                            tab: tab.clone(),
                            result: Err(CtlError::new(
                                code::UNAVAILABLE,
                                "the page tried to go to an address that cannot open here",
                            )),
                        });
                    }
                }
            }
            WebEvent::Still { tab, token, png } => {
                if self.shown_tab.as_deref() == Some(tab.as_str())
                    && let Some((phase, acts)) = web::captured(self.phase, token)
                {
                    crate::timer_registry::clear(CAPTURE_TIMER);
                    self.phase = phase;
                    self.still_token = token;
                    for a in acts {
                        self.act(app, &tab, a);
                    }
                    match png {
                        Some(bytes) => self.decode_still(tab, token, bytes),
                        None => self.ended_capture = Some(token),
                    }
                } else if self.shown_tab.as_deref() == Some(tab.as_str())
                    && self.phase == Phase::Still
                    && self.ended_capture == Some(token)
                    && let Some(bytes) = png
                {
                    // Slow, not lost: the picture of the capture the timeout ended, for the overlay still open.
                    self.ended_capture = None;
                    self.still_token = token;
                    self.decode_still(tab, token, bytes);
                }
            }
            WebEvent::Key { tab: _, key } => out.push(WebOut::Key(key)),
            WebEvent::Focus { tab, has } => {
                self.has_focus = has;
                out.push(WebOut::PageFocus(has));
                // The person's click on a page that had no focus gives it the keyboard: that is a
                // takeover too, whatever the mouse poll saw (the agent's own events never move the
                // window's focus).
                if has && self.tabs.get(&tab).is_some_and(|t| t.agent.is_some()) {
                    self.finish_agent(&tab, Err(Job::interrupted()));
                }
            }
            WebEvent::Gone { tab, why } => {
                self.fail_view(&tab);
                self.end_agent(&tab, "the page's process ended");
                self.end_probe(&tab, "the page's process ended");
                if let Some(t) = self.tabs.get_mut(&tab) {
                    t.failed = Some((why, false));
                    t.loading = false;
                }
                out.push(WebOut::Changed);
            }
            WebEvent::Script { id, result } => {
                self.scripts.insert(id, result);
            }
            WebEvent::Cdp { tab, id, result } => {
                if let Some(o) = self.diagram_answer(&tab, id, &result) {
                    out.push(o);
                } else {
                    self.on_cdp(&tab, id, result);
                }
            }
            WebEvent::ModMessage { tab, host, json } => {
                // Only a mod's page has a bridge; a message from any other view is dropped, and so is one from
                // a page the tab no longer shows (it was queued before the tab moved to another mod).
                let live = self
                    .tabs
                    .get(&tab)
                    .and_then(|t| t.app_host.as_deref())
                    .is_some_and(|h| h == host);
                if live && let Some(mod_id) = flare_surfaces::modview::mod_of_host(&host) {
                    out.push(WebOut::ModMessage {
                        tab,
                        mod_id: mod_id.to_string(),
                        json,
                    });
                }
            }
        }
        out.extend(mod_note);
        if let Some((tab, reply, settled)) = diagram_ask {
            self.ask_diagram(&tab, reply, settled);
        }
        out
    }

    /// Asks a diagram page whether it drew (its `window.__flareDiagram` promise). The answer is read
    /// by [`WebCtl::diagram_answer`].
    fn ask_diagram(&mut self, tab: &str, reply: Sender<Result<Value, CtlError>>, settled: Settled) {
        self.call_ids += 1;
        let id = self.call_ids;
        let params = serde_json::json!({
            "expression": "window.__flareDiagram",
            "awaitPromise": true,
            "returnByValue": true,
        })
        .to_string();
        let Some(t) = self.tabs.get_mut(tab) else {
            let _ = reply.send(Err(CtlError::new(
                code::UNAVAILABLE,
                "the panel was closed before the diagram was drawn",
            )));
            return;
        };
        t.probe = Some(Probe { id, reply, settled });
        match self.backend.as_mut() {
            Some(b) => b.cdp(tab, id, "Runtime.evaluate", &params),
            None => self.end_probe(tab, "the web view is gone"),
        }
    }

    /// The page's word on a diagram: the call that waited for it is answered. None when this answer is
    /// not the diagram question's.
    fn diagram_answer(
        &mut self,
        tab: &str,
        id: u64,
        result: &Result<String, String>,
    ) -> Option<WebOut> {
        let t = self.tabs.get_mut(tab)?;
        if t.probe.as_ref().is_none_or(|p| p.id != id) {
            return None;
        }
        let p = t.probe.take()?;
        let answer = match result {
            Ok(json) => app::diagram_result(json),
            Err(e) => Err(format!("the page did not answer: {e}")),
        };
        Some(WebOut::Settle {
            reply: p.reply,
            tab: tab.to_string(),
            result: match answer {
                Ok(()) => Ok(p.settled),
                Err(msg) => Err(CtlError::new(
                    code::BAD_PARAMS,
                    format!("the diagram did not draw: {msg}"),
                )),
            },
        })
    }

    /// The tab's page is gone under a diagram's question: it is answered `unavailable`.
    fn end_probe(&mut self, tab: &str, why: &str) {
        if let Some(t) = self.tabs.get_mut(tab)
            && let Some(p) = t.probe.take()
        {
            let _ = p
                .reply
                .send(Err(CtlError::new(code::UNAVAILABLE, why.to_string())));
        }
    }

    /// The tab's page is gone under an agent call: it is answered `unavailable`.
    fn end_agent(&mut self, tab: &str, why: &str) {
        if let Some(t) = self.tabs.get_mut(tab)
            && let Some(a) = t.agent.take()
        {
            let _ = a
                .reply
                .send(Err(CtlError::new(code::UNAVAILABLE, why.to_string())));
        }
        self.refresh_key_flags();
    }

    /// Decodes a still on a worker thread; the answer is a `WebMsg::Decoded`.
    fn decode_still(&self, tab: String, token: u64, bytes: Vec<u8>) {
        let post = std::sync::Arc::clone(&self.post);
        let job = move || {
            let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
                .ok()
                .map(|i| {
                    let rgba = i.to_rgba8();
                    SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                        rgba.as_raw(),
                        rgba.width(),
                        rgba.height(),
                    )
                });
            post(UiMsg::Web(WebMsgBox(WebMsg::Decoded { tab, token, image })));
        };
        if let Err(e) = std::thread::Builder::new()
            .name("flare-web-still".into())
            .spawn(job)
        {
            log::warn!("could not start the still decoder: {e}");
        }
    }
}

/// The production browser: WebView2 in Flare's own window, its profile under the profile folder the
/// run was given (else `%LOCALAPPDATA%\Flare\WebView2`). Made when the first web panel needs it, so a
/// run that never opens one never loads the browser's runtime.
pub fn production_factory(
    window: slint::Weak<AppWindow>,
    user_data_dir: Option<std::path::PathBuf>,
    profile_dir: std::path::PathBuf,
) -> WebFactory {
    Box::new(move |sink| {
        let parent = window
            .upgrade()
            .and_then(|a| crate::window::hwnd_of(&a))
            .unwrap_or(0);
        let data_dir = web::webview_folder(
            user_data_dir.as_deref(),
            flare_win::paths::local_app_data()
                .map(|p| p.to_string_lossy().into_owned())
                .as_deref(),
        )
        .unwrap_or_else(|| profile_dir.join("WebView2"));
        // The e2e's seams (read only under --e2e): a pretend missing runtime, and a folder of local
        // pages served as https://flare-test.local/.
        let missing = crate::ship_ctl::e2e_var("WEBVIEW_RUNTIME").as_deref() == Some("missing");
        let runtime: flare_web::host::RuntimeCheck = if missing {
            std::sync::Arc::new(|| Err("the WebView2 Runtime is not installed (test)".to_string()))
        } else {
            std::sync::Arc::new(flare_web::host::installed_runtime)
        };
        let virtual_hosts = crate::ship_ctl::e2e_var("WEB_ROOT")
            .map(|root| {
                vec![(
                    "flare-test.local".to_string(),
                    std::path::PathBuf::from(root),
                )]
            })
            .unwrap_or_default();
        Box::new(flare_web::host::WebHost::new(flare_web::host::HostConfig {
            parent,
            data_dir,
            sink,
            runtime,
            virtual_hosts,
            // The e2e's seam: a mini-app without the peer-connection script, to test the walls under it.
            without_webrtc_script: crate::ship_ctl::e2e_var("APP_NO_WEBRTC_SCRIPT").is_some(),
        }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(vk: u32, ctrl: bool, shift: bool, alt: bool) -> KeyPress {
        KeyPress {
            vk,
            ctrl,
            shift,
            alt,
            down: true,
            repeat: false,
        }
    }

    #[test]
    fn virtual_keys_become_the_keys_the_chord_table_knows() {
        let k = key_event_of(&press(0x50, true, true, false)).unwrap();
        assert_eq!((k.key.as_str(), k.code.as_str()), ("p", "KeyP"));
        assert!(k.ctrl && k.shift && !k.alt && k.pressed);
        let k = key_event_of(&press(0x31, true, false, false)).unwrap();
        assert_eq!((k.key.as_str(), k.code.as_str()), ("1", "Digit1"));
        let k = key_event_of(&press(0x70, false, false, false)).unwrap();
        assert_eq!(k.key, "F1");
        let k = key_event_of(&press(0x7A, false, false, false)).unwrap();
        assert_eq!(k.key, "F11");
        let k = key_event_of(&press(0xBC, true, false, false)).unwrap();
        assert_eq!((k.key.as_str(), k.code.as_str()), (",", "Comma"));
        assert_eq!(
            key_event_of(&press(0x09, true, false, false)).unwrap().key,
            "Tab"
        );
        assert_eq!(
            key_event_of(&press(0x22, true, false, false)).unwrap().key,
            "PageDown"
        );
        // A key the table has no name for is never a chord.
        assert!(key_event_of(&press(0x0D, false, false, false)).is_none());
        assert!(key_event_of(&press(0x25, false, false, false)).is_none());
    }

    fn ctx() -> KeyContext {
        KeyContext {
            help: true,
            vertical: true,
            ..KeyContext::default()
        }
    }

    #[test]
    fn the_app_takes_its_chords_from_the_page() {
        for (k, what) in [
            (press(0x50, true, true, false), "Ctrl+Shift+P: the panel"),
            (
                press(0x4F, true, true, false),
                "Ctrl+Shift+O: the panel focus",
            ),
            (press(0x54, true, false, false), "Ctrl+T: a new tab"),
            (press(0x57, true, false, false), "Ctrl+W: close the tab"),
            (press(0x09, true, false, false), "Ctrl+Tab: next tab"),
            (press(0x31, true, false, false), "Ctrl+1: jump to a tab"),
            (press(0xBC, true, false, false), "Ctrl+,: settings"),
            (press(0x4B, true, true, false), "Ctrl+Shift+K: find a tab"),
            (press(0x42, true, true, false), "Ctrl+Shift+B: the sidebar"),
            (press(0x70, false, false, false), "F1: help"),
            (press(0x7A, false, false, false), "F11: full screen"),
            (
                press(0x20, false, false, true),
                "Alt+Space: the system menu",
            ),
            (
                press(0x46, true, true, false),
                "Ctrl+Shift+F: find in the terminal",
            ),
        ] {
            assert!(app_takes(&k, &ctx()), "{what}");
        }
    }

    #[test]
    fn the_page_keeps_every_other_key() {
        for (k, what) in [
            (press(0x50, false, false, false), "a plain p"),
            (press(0x50, true, false, false), "Ctrl+P (print)"),
            (press(0x46, true, false, false), "Ctrl+F (the page's find)"),
            (press(0x43, true, false, false), "Ctrl+C (copy)"),
            (press(0x56, true, false, false), "Ctrl+V (paste)"),
            (press(0x41, true, false, false), "Ctrl+A (select all)"),
            (press(0x1B, false, false, false), "Escape"),
            (press(0x0D, false, false, false), "Enter"),
            (press(0x25, false, false, false), "an arrow"),
            (press(0x74, false, false, false), "F5 (reload)"),
        ] {
            assert!(!app_takes(&k, &ctx()), "{what}");
        }
        // The agent guard's Ctrl+D and Ctrl+C are the page's even where the guard is on.
        let guard = KeyContext {
            agent_guard: true,
            agent_here: true,
            ..ctx()
        };
        assert!(!app_takes(&press(0x44, true, false, false), &guard));
        assert!(!app_takes(&press(0x43, true, false, false), &guard));
        // Help off: F1 is the page's.
        let no_help = KeyContext {
            help: false,
            ..ctx()
        };
        assert!(!app_takes(&press(0x70, false, false, false), &no_help));
    }

    #[test]
    fn the_hook_takes_only_the_second_escape_and_resets_on_another_key() {
        let mut c = WebCtl::new(std::sync::Arc::new(|_| {}));
        let filter = c.key_filter();
        let esc = press(0x1B, false, false, false);
        assert!(!filter(&esc), "the first Escape is the page's");
        assert!(filter(&esc), "the second, soon after, is the app's");
        assert!(!filter(&esc), "and the count starts over");
        assert!(!filter(&press(0x41, false, false, false)));
        assert!(!filter(&esc));
        let _ = &mut c;
        // A key up is never taken.
        let up = KeyPress {
            down: false,
            ..press(0x50, true, true, false)
        };
        assert!(!filter(&up));
    }

    #[test]
    fn a_held_escape_is_one_press_and_its_repeats_count_for_nothing() {
        let c = WebCtl::new(std::sync::Arc::new(|_| {}));
        let filter = c.key_filter();
        let esc = press(0x1B, false, false, false);
        let held = KeyPress {
            repeat: true,
            ..esc
        };
        assert!(!filter(&esc), "the first Escape is the page's");
        assert!(!filter(&held), "an autorepeat is not the second Escape");
        assert!(!filter(&held));
        assert!(filter(&esc), "a fresh press soon after is the second");
    }

    #[test]
    fn the_blur_decision_follows_where_the_keyboard_went() {
        // No browser: the old rule, a blur is a blur.
        assert_eq!(focus_loss(false, false, true), FocusLoss::Now);
        // The page has the keyboard: the window is still in use.
        assert_eq!(focus_loss(true, true, true), FocusLoss::Skip);
        assert_eq!(focus_loss(true, true, false), FocusLoss::Skip);
        // Still the foreground window, page not yet known to have it: ask again one turn later.
        assert_eq!(focus_loss(true, false, true), FocusLoss::Check);
        // Another window is in front already.
        assert_eq!(focus_loss(true, false, false), FocusLoss::Now);
    }

    fn post_log() -> (
        crate::appctl::Post,
        std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    ) {
        let log: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
        let keep = log.clone();
        let post: crate::appctl::Post = std::sync::Arc::new(move |m| {
            keep.lock().unwrap().push(format!("{m:?}"));
        });
        (post, log)
    }

    #[test]
    fn a_key_of_the_persons_posts_a_takeover_only_while_an_agent_works() {
        let (post, log) = post_log();
        let c = WebCtl::new(post);
        let filter = c.key_filter();
        let a = press(0x41, false, false, false);
        assert!(!filter(&a));
        assert!(
            log.lock().unwrap().is_empty(),
            "no agent call: nothing to end"
        );
        c.keys.job_active.set(true);
        assert!(!filter(&a), "the key is still the page's");
        assert_eq!(log.lock().unwrap().as_slice(), ["Web(WebMsg::UserInput)"]);
        // A key up is not a press.
        let up = KeyPress { down: false, ..a };
        assert!(!filter(&up));
        assert_eq!(log.lock().unwrap().len(), 1);
    }

    #[test]
    fn the_agents_own_key_press_is_neither_the_persons_nor_a_chord() {
        let (post, log) = post_log();
        let c = WebCtl::new(post);
        let filter = c.key_filter();
        c.keys.job_active.set(true);
        c.keys.agent_keys.set(true);
        // Ctrl+Shift+P would be the app's chord; the agent's press of it is the page's.
        assert!(!filter(&press(0x50, true, true, false)));
        assert!(!filter(&press(0x1B, false, false, false)));
        assert!(!filter(&press(0x1B, false, false, false)));
        assert!(log.lock().unwrap().is_empty());
        c.keys.agent_keys.set(false);
        assert!(
            filter(&press(0x50, true, true, false)),
            "and the chord is the app's again"
        );
    }

    #[test]
    fn a_point_is_inside_a_rectangle_by_its_near_edges() {
        let r = PhysRect {
            x: 10,
            y: 20,
            w: 100,
            h: 50,
        };
        assert!(point_in(&r, 10, 20));
        assert!(point_in(&r, 109, 69));
        assert!(!point_in(&r, 110, 69));
        assert!(!point_in(&r, 109, 70));
        assert!(!point_in(&r, 9, 20));
        assert!(!point_in(&PhysRect::default(), 0, 0));
    }

    #[test]
    fn the_ribbon_names_who_is_using_the_page() {
        use flare_agent::kinds::DetectedAgent as D;
        assert_eq!(
            ribbon_words(&agent_label(D::Claude)),
            "Claude is using this page"
        );
        assert_eq!(
            ribbon_words(&agent_label(D::Codex)),
            "Codex is using this page"
        );
        assert_eq!(
            ribbon_words(&agent_label(D::Other)),
            "An agent is using this page"
        );
    }

    #[test]
    fn a_message_prints_its_kind_and_never_its_data() {
        let m = WebMsgBox(WebMsg::Event(WebEvent::Loaded {
            tab: "t".into(),
            url: "https://secret.example/token=1".into(),
            ok: true,
            status: "0".into(),
            nav: 1,
        }));
        let text = format!("{m:?}");
        assert_eq!(text, "WebMsg::Loaded");
        assert!(!text.contains("secret"));
        assert_eq!(
            format!("{:?}", WebMsgBox(WebMsg::UserInput)),
            "WebMsg::UserInput"
        );
        assert_eq!(
            format!("{:?}", WebMsgBox(WebMsg::AgentTick)),
            "WebMsg::AgentTick"
        );
        assert_eq!(
            format!(
                "{:?}",
                WebMsgBox(WebMsg::Decoded {
                    tab: "t".into(),
                    token: 1,
                    image: None
                })
            ),
            "WebMsg::Decoded"
        );
    }

    #[test]
    fn the_runtime_link_is_microsofts() {
        assert!(RUNTIME_URL.starts_with("https://go.microsoft.com/"));
        assert!(runtime_missing_note().contains(RUNTIME_URL));
    }
}

#[cfg(test)]
mod typing_tests {
    use super::is_typing_key;

    #[test]
    fn letters_digits_space_enter_and_arrows_are_typing_and_buttons_and_modifiers_alone_are_not() {
        for vk in [b'A', b'Z', b'0', b'9', 0x20, 0x0D, 0x25, 0x08, 0x70, 0xBA] {
            assert!(is_typing_key(vk), "{vk:#x}");
        }
        for vk in [
            0x01, 0x02, 0x04, 0x10, 0x11, 0x12, 0x5B, 0xA0, 0xA2, 0xA4, 0x14,
        ] {
            assert!(!is_typing_key(vk), "{vk:#x}");
        }
    }
}
