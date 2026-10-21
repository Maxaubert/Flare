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

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::time::Instant;

use flare_ctl::{CtlError, code};
use flare_surfaces::web::{
    self, Act, EscTwice, Phase, PhysRect, UrlError, WebShow, host_label, normalize_url, phys_rect,
    step, web_show,
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
}

/// A `WebMsg` that can sit in a `UiMsg` (which is `Debug`): it prints its kind only, never an
/// address, a title or a picture.
pub struct WebMsgBox(pub WebMsg);

impl std::fmt::Debug for WebMsgBox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            WebMsg::Event(e) => write!(f, "WebMsg::{}", e.kind()),
            WebMsg::Decoded { .. } => f.write_str("WebMsg::Decoded"),
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
}

struct Pending {
    reply: Sender<Result<Value, CtlError>>,
    deadline: Instant,
    /// The navigation this call waits for: the browser's number for the latest one that began after
    /// the call was made (a redirect rebinds it). A load that finishes before any has begun, or one
    /// that is not this one, is not the answer.
    nav: Option<u64>,
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
}

/// The state the key hook reads (it runs inside a WebView2 callback and may not borrow the window).
struct KeyState {
    ctx: Cell<KeyContext>,
    esc: RefCell<EscTwice>,
    clock: Instant,
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
    /// The still of the shown tab while an overlay hides its window.
    still: Option<Image>,
    /// The capture the still answers.
    still_token: u64,
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
            }),
            has_focus: false,
            scripts: HashMap::new(),
            script_seq: 0,
            still: None,
            still_token: 0,
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
        Rc::new(move |k: &KeyPress| {
            if !k.down {
                return false;
            }
            if k.vk == 0x1B && !k.ctrl && !k.shift && !k.alt {
                let now = u64::try_from(keys.clock.elapsed().as_millis()).unwrap_or(u64::MAX);
                return keys.esc.borrow_mut().escape(now);
            }
            keys.esc.borrow_mut().other_key();
            app_takes(k, &keys.ctx.get())
        })
    }

    /// The browser, made now if it is not (None: this build cannot make one).
    fn backend(&mut self) -> Option<&mut Box<dyn WebBackend>> {
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
        // A tab whose web panel is closed (or gone) has no browser.
        let stale: Vec<String> = self
            .tabs
            .keys()
            .filter(|t| panels.web_target(t).is_none())
            .cloned()
            .collect();
        for t in stale {
            // The page that had the keyboard goes away: the window takes it back.
            if self.has_focus && self.shown_tab.as_deref() == Some(t.as_str()) {
                self.release_focus(crate::window::hwnd_of(app));
            }
            self.close_tab(&t);
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
            self.open_tab(tab, &url, rect, false);
        }

        // Which tab's window is on the screen.
        let want_tab = front_web.filter(|t| self.tabs.contains_key(*t));
        if self.shown_tab.as_deref() != want_tab {
            if let Some(old) = self.shown_tab.take() {
                self.hide(&old);
            }
            self.phase = Phase::Hidden;
            self.drop_still(app);
            self.shown_tab = want_tab.map(str::to_string);
        }
        let want = match &self.shown_tab {
            Some(_) => web_show(true, shown, overlay),
            None => WebShow::Hidden,
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

    fn open_tab(&mut self, tab: &str, url: &str, rect: PhysRect, visible: bool) {
        self.tabs.insert(
            tab.to_string(),
            TabWeb {
                url: url.to_string(),
                ..TabWeb::default()
            },
        );
        match self.backend() {
            Some(b) => b.open(tab, url, rect, visible),
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
            app.set_panel_web_url("".into());
            app.set_panel_web_loading(false);
            app.set_panel_web_can_back(false);
            app.set_panel_web_can_forward(false);
            app.set_panel_web_missing_runtime(false);
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
        let title = t
            .map(|t| t.title.as_str())
            .filter(|s| !s.is_empty())
            .map_or_else(|| host_label(url), str::to_string);
        app.set_panel_kind(3);
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
    }

    /// Frees the tab's browser.
    pub fn close_tab(&mut self, tab: &str) {
        if let Some(t) = self.tabs.remove(tab) {
            if let Some(p) = t.pending {
                let _ = p.reply.send(Err(CtlError::new(
                    code::UNAVAILABLE,
                    "the page was closed before it finished loading",
                )));
            }
            if let Some(b) = self.backend.as_mut() {
                b.close(tab);
            }
        }
        if self.shown_tab.as_deref() == Some(tab) {
            self.shown_tab = None;
            self.phase = Phase::Hidden;
            self.still = None;
        }
        if self.tabs.is_empty() {
            // No web panel is open: the browser goes (it is made again by the next one).
            self.has_focus = false;
            self.bounds = None;
            self.backend = None;
        }
    }

    // ---- what the user and the channel ask ---------------------------------------------------------

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
            nav: None,
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

    // ---- the e2e's seams -------------------------------------------------------------------------------

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
        }
    }

    fn on_event(&mut self, app: &AppWindow, e: WebEvent) -> Vec<WebOut> {
        let mut out = Vec::new();
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
                    t.loading = true;
                    t.notice.clear();
                    t.url = url;
                    if let Some(p) = t.pending.as_mut() {
                        p.nav = Some(nav);
                    }
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
                            out.push(WebOut::Settle {
                                reply: p.reply,
                                tab: tab.clone(),
                                result: Ok(settled),
                            });
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
                    self.phase = phase;
                    self.still_token = token;
                    for a in acts {
                        self.act(app, &tab, a);
                    }
                    if let Some(bytes) = png {
                        self.decode_still(tab, token, bytes);
                    }
                }
            }
            WebEvent::Key { tab: _, key } => out.push(WebOut::Key(key)),
            WebEvent::Focus { tab: _, has } => {
                self.has_focus = has;
                out.push(WebOut::PageFocus(has));
            }
            WebEvent::Gone { tab, why } => {
                if let Some(t) = self.tabs.get_mut(&tab) {
                    t.failed = Some((why, false));
                    t.loading = false;
                }
                out.push(WebOut::Changed);
            }
            WebEvent::Script { id, result } => {
                self.scripts.insert(id, result);
            }
        }
        out
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
