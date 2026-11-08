//! What the window asks of a web view and what a web view tells the window: the seam between
//! `flare` (Slint, panels, the channel) and the WebView2 glue ([`crate::host::WebHost`]). A test
//! world uses [`crate::fake::FakeWeb`] behind the same trait, so the panel logic is tested without a
//! browser.
//!
//! Everything here runs on the UI thread (WebView2 is single-threaded COM). Nothing blocks: a call
//! that needs the browser starts it and the answer comes back as a [`WebEvent`], posted to the
//! window's message queue (rule 34).

use std::rc::Rc;
use std::sync::Arc;

use flare_surfaces::app::AppSpec;
pub use flare_surfaces::slot::{Slot, ViewKind};
use flare_surfaces::web::PhysRect;

/// Where events go: the window's `UiMsg` queue. Called on the UI thread, from inside the message
/// loop's dispatch of a WebView2 callback, so it must only queue.
pub type Sink = Arc<dyn Fn(WebEvent) + Send + Sync>;

/// A key the page is about to get, as the accelerator hook sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyPress {
    /// The Win32 virtual-key code.
    pub vk: u32,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// Down (a key up is never a chord).
    pub down: bool,
    /// An autorepeat of a key already held (it is not a new press).
    pub repeat: bool,
}

/// Decides, at once, whether the app takes this key from the page (the chord table, the second
/// Escape). The hook cannot wait: WebView2 needs the answer before it returns.
pub type KeyFilter = Rc<dyn Fn(&KeyPress) -> bool>;

/// A move through the page's history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    Back,
    Forward,
    Reload,
    Stop,
}

/// What a web view reports.
#[derive(Clone, PartialEq, Eq)]
pub enum WebEvent {
    /// Something finished in the background (the environment or a controller): ask the backend to
    /// `pump`.
    Pump,
    /// The runtime question was answered on a worker (the version, or why not): hand it to
    /// `runtime_checked`.
    Runtime(Result<String, String>),
    /// The tab's controller exists.
    Ready {
        tab: String,
    },
    /// The tab's web view cannot be made. `missing_runtime`: the Edge WebView2 Runtime is not
    /// installed (the panel shows the download link).
    Failed {
        tab: String,
        why: String,
        missing_runtime: bool,
    },
    /// A navigation began (allowed ones only). `nav` is the browser's number for it; a redirect has its own.
    Loading {
        tab: String,
        url: String,
        nav: u64,
    },
    /// A navigation ended; `nav` is the number `Loading` gave it.
    Loaded {
        tab: String,
        url: String,
        ok: bool,
        status: String,
        nav: u64,
    },
    /// The address changed (history, a script, a redirect).
    Url {
        tab: String,
        url: String,
    },
    /// The address changed inside the same document (a `#fragment`, `history.pushState`): the
    /// browser raises no navigation for it.
    SameDocument {
        tab: String,
        url: String,
    },
    /// A download is waiting for a yes: nothing is saved until `answer_download`. `file` is the name
    /// the browser proposes, `host` where it comes from.
    Download {
        tab: String,
        id: u64,
        file: String,
        host: String,
    },
    Title {
        tab: String,
        title: String,
    },
    History {
        tab: String,
        can_back: bool,
        can_forward: bool,
    },
    /// A navigation to an address that may not open here was cancelled.
    Refused {
        tab: String,
        url: String,
    },
    /// A still of the page (PNG), or None when it could not be taken. `token` is the caller's.
    Still {
        tab: String,
        token: u64,
        png: Option<Vec<u8>>,
    },
    /// The page had a key the app took (a chord, or the second Escape).
    Key {
        tab: String,
        key: KeyPress,
    },
    /// The page took or lost the keyboard.
    Focus {
        tab: String,
        has: bool,
    },
    /// The page put an element in full screen (a video's full-screen button, `requestFullscreen`), or left it
    /// (Escape, `exitFullscreen`). The host is to make the page cover the window while `on` holds.
    FullScreen {
        tab: String,
        on: bool,
    },
    /// The browser process of the tab ended or the controller was closed under us.
    Gone {
        tab: String,
        why: String,
    },
    /// A script the e2e started answered (JSON text, or an error).
    Script {
        id: u64,
        result: Result<String, String>,
    },
    /// A DevTools Protocol call answered: the result's JSON, or why not. `id` is the caller's.
    Cdp {
        tab: String,
        id: u64,
        result: Result<String, String>,
    },
    /// A mod's page sent a message over its bridge (`window.flare`). `json` is the text the page posted, from
    /// the mod's own address only; it is untrusted until `flare_mods::bridge::parse_page_message` has read it.
    ModMessage {
        tab: String,
        /// The host of the view that received it (the mod's own, which names the mod): a message that arrives
        /// after the tab moved on to another mod's page is told apart by it.
        host: String,
        json: String,
    },
    /// An event of a view that is not a tab's panel (a plugin's bar or background page): the event as a panel
    /// would say it (its `tab` is the slot's owner), with the slot it came from. A panel's events are never
    /// wrapped, so everything that knew only panels reads them as before. See [`addressed`].
    View {
        slot: Slot,
        event: Box<WebEvent>,
    },
}

/// The event of the view in `slot`: as it is for a tab's panel, wrapped in `WebEvent::View` for any other slot.
pub fn addressed(slot: &Slot, event: WebEvent) -> WebEvent {
    if slot.is_panel() {
        event
    } else {
        WebEvent::View {
            slot: slot.clone(),
            event: Box::new(event),
        }
    }
}

/// Where one view's events go: the window's sink, every event addressed by the view's slot.
#[derive(Clone)]
pub struct ViewSink {
    sink: Sink,
    slot: Slot,
}

impl ViewSink {
    pub fn new(sink: &Sink, slot: &Slot) -> Self {
        Self {
            sink: Arc::clone(sink),
            slot: slot.clone(),
        }
    }

    /// Sends the event `make` builds from the view's owner (the event's `tab`).
    pub fn send(&self, make: impl FnOnce(String) -> WebEvent) {
        (self.sink)(addressed(&self.slot, make(self.slot.owner.clone())));
    }
}

impl WebEvent {
    /// The variant's name: what a log or a `Debug` prints (never an address or a title).
    pub fn kind(&self) -> &'static str {
        match self {
            WebEvent::Pump => "Pump",
            WebEvent::Runtime(_) => "Runtime",
            WebEvent::Ready { .. } => "Ready",
            WebEvent::Failed { .. } => "Failed",
            WebEvent::Loading { .. } => "Loading",
            WebEvent::Loaded { .. } => "Loaded",
            WebEvent::Url { .. } => "Url",
            WebEvent::SameDocument { .. } => "SameDocument",
            WebEvent::Download { .. } => "Download",
            WebEvent::Title { .. } => "Title",
            WebEvent::History { .. } => "History",
            WebEvent::Refused { .. } => "Refused",
            WebEvent::Still { .. } => "Still",
            WebEvent::Key { .. } => "Key",
            WebEvent::Focus { .. } => "Focus",
            WebEvent::FullScreen { .. } => "FullScreen",
            WebEvent::Gone { .. } => "Gone",
            WebEvent::Script { .. } => "Script",
            WebEvent::Cdp { .. } => "Cdp",
            WebEvent::ModMessage { .. } => "ModMessage",
            WebEvent::View { .. } => "View",
        }
    }
}

impl std::fmt::Debug for WebEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "WebEvent::{}", self.kind())
    }
}

/// What the e2e reads back of a live controller: the facts the browser itself holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ControllerProbe {
    /// `Controller.Bounds` in the parent's client pixels.
    pub bounds: PhysRect,
    pub visible: bool,
    /// The parent's child windows that belong to a web view: class name and rectangle, in the
    /// parent's client pixels.
    pub child_windows: Vec<(String, PhysRect)>,
    pub url: String,
    pub title: String,
    pub browser_pid: u32,
    /// The browser profile the view runs in (empty for the web panel's own default profile).
    pub profile: String,
    /// The profile keeps nothing on disk (a mini-app's).
    pub private: bool,
    /// The browser offers to save passwords and fills them in, and fills in forms (the web panel only).
    pub autofill: bool,
}

/// A view's latest picture, for a backend that draws its pages off screen (macOS: CEF's windowless rendering): BGRA,
/// premultiplied, `width * height` device pixels, and a number that changes with every new picture. The window
/// shows it where the page sits (on Windows the page is a child window and there is no picture).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ViewFrame {
    pub width: u32,
    pub height: u32,
    pub bgra: Arc<Vec<u8>>,
    pub serial: u64,
}

/// The modifier keys held with a pointer or key event the window forwards.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// Cmd on a Mac.
    pub cmd: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerButton {
    Left,
    Middle,
    Right,
}

/// What the pointer did over a view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PointerAct {
    Move,
    /// It left the view.
    Leave,
    /// A button went down; the click count (2 for a double click).
    Down(PointerButton, u8),
    Up(PointerButton, u8),
    /// A wheel or trackpad scroll, in device pixels (positive `dy` scrolls the content down, as a wheel turned
    /// toward the person does).
    Wheel {
        dx: f32,
        dy: f32,
    },
}

/// A pointer event the window forwards to an off-screen view: where, in device pixels from the view's top left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pointer {
    pub x: f32,
    pub y: f32,
    pub act: PointerAct,
    pub mods: Mods,
}

/// A key the window forwards to an off-screen view that has the keyboard.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyInput {
    pub down: bool,
    pub repeat: bool,
    /// The Win32 virtual-key code (Chromium's `keyCode`, and what the key filter reads).
    pub vk: u32,
    /// The macOS key code (`kVK_*`), 0 when unknown.
    pub mac_code: u16,
    /// What the key types (empty for a key that types nothing).
    pub text: String,
    pub mods: Mods,
}

/// The input method's text, forwarded to the view that has the keyboard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImeInput {
    /// Text being composed, the caret at `cursor` (UTF-16 units).
    Preedit {
        text: String,
        cursor: u32,
    },
    /// The composed text is chosen.
    Commit(String),
    Cancel,
}

/// One web view per slot: the panel of each tab with an open web panel and, for plugins, a bar beside a tab or
/// a background page (see [`Slot`]; only panels are made by the app so far). The methods that take a tab act on
/// its panel.
pub trait WebBackend {
    /// Makes the tab's view (asynchronously, the first time also the browser environment) and
    /// opens `url` in it at `bounds`. A view the tab already has is only moved and sent to `url`.
    fn open(&mut self, tab: &str, url: &str, bounds: PhysRect, visible: bool);
    /// Makes the tab's view for a mini-app (Surfaces stage 5): a view of the apps' own private profile,
    /// with the app's folder served on its host name, a navigation that may not leave the app's page, no
    /// window of its own for the page and, unless `app.allow_network`, no request that is not the app's
    /// own. A view the tab already has (of anything else) is closed first.
    fn open_app(&mut self, tab: &str, app: AppSpec, bounds: PhysRect, visible: bool) {
        self.open_view(&Slot::panel(tab), app, bounds, visible);
    }
    /// Makes the view of `slot` for a page with walls (`open_app` is this for a tab's panel). A bar or a
    /// background slot takes only a mod's or plugin's page (`app.module`); anything else is answered `Failed`
    /// and no view is made. Every mod's and plugin's view is sealed as its kind says
    /// ([`crate::seal::plugin_seal`]). A view the slot already has (of anything else) is closed first.
    fn open_view(&mut self, slot: &Slot, app: AppSpec, bounds: PhysRect, visible: bool);
    fn navigate(&mut self, tab: &str, url: &str);
    fn go(&mut self, tab: &str, nav: Nav);
    fn set_bounds(&mut self, tab: &str, bounds: PhysRect) {
        self.set_view_bounds(&Slot::panel(tab), bounds);
    }
    fn set_visible(&mut self, tab: &str, visible: bool) {
        self.set_view_visible(&Slot::panel(tab), visible);
    }
    fn set_view_bounds(&mut self, slot: &Slot, bounds: PhysRect);
    fn set_view_visible(&mut self, slot: &Slot, visible: bool);
    /// The window the pages sit in moved (or changed monitor or size): the browser must be told, or the
    /// popups it draws (a select's list, the IME's candidates, its menus) keep the old screen position.
    fn parent_moved(&mut self) {}
    /// Takes a still of the page; answered by `WebEvent::Still` with the same token.
    fn capture_still(&mut self, tab: &str, token: u64);
    /// Gives the page the keyboard.
    fn focus(&mut self, tab: &str);
    /// Closes and frees the tab's view. The browser environment goes with the last one.
    fn close(&mut self, tab: &str) {
        self.close_view(&Slot::panel(tab));
    }
    /// Closes and frees the view of `slot`, as `close` does a tab's.
    fn close_view(&mut self, slot: &Slot);
    /// Closes every view and lets the browser environment go, now (the window is ending). Nothing is
    /// reported after it. Downloads still waiting for a yes are cancelled.
    fn close_all(&mut self);
    /// The yes or no a `WebEvent::Download` asked for. Unknown ids are ignored.
    fn answer_download(&mut self, id: u64, save: bool);
    /// Takes in what finished in the background (answers `WebEvent::Pump`).
    fn pump(&mut self);
    /// The runtime check the first open asked for has answered (`WebEvent::Runtime`).
    fn runtime_checked(&mut self, result: Result<String, String>);
    /// How many controllers exist, including ones still being made.
    fn controllers(&self) -> usize;
    /// Where the hook asks whether the app takes a key.
    fn set_key_filter(&mut self, filter: KeyFilter);
    /// Calls a DevTools Protocol method of the tab's page (no debugging port is opened: the call goes
    /// through the controller). `params` is a JSON object; the answer is a `WebEvent::Cdp` with the
    /// caller's `id`, posted from the browser's completion and never waited for (rule 34). A tab with
    /// no view answers at once with an error.
    fn cdp(&mut self, tab: &str, id: u64, method: &str, params: &str);
    /// Posts a message to a mod's page (its bridge's `message` event). Does nothing for a view that is not a
    /// mod's or has no page yet.
    fn post_message(&mut self, tab: &str, json: &str) {
        self.post_to_view(&Slot::panel(tab), json);
    }
    /// Posts a message to the mod's or plugin's page in `slot`, as `post_message` does to a tab's.
    fn post_to_view(&mut self, slot: &Slot, json: &str);
    /// The e2e only: sets the browser's own zoom of the tab's page (what Ctrl+wheel in the panel does).
    fn set_zoom(&mut self, tab: &str, factor: f64);
    /// The e2e only: runs a script in the page; answered by `WebEvent::Script`.
    fn run_script(&mut self, tab: &str, id: u64, js: &str);
    /// The e2e only: the controller's own numbers.
    fn probe(&self, tab: &str) -> Option<ControllerProbe> {
        self.probe_view(&Slot::panel(tab))
    }
    fn probe_view(&self, slot: &Slot) -> Option<ControllerProbe>;
    /// The view's latest picture when the backend draws off screen (macOS); None on Windows, where the page is a
    /// window of its own.
    fn frame(&self, _slot: &Slot) -> Option<ViewFrame> {
        None
    }
    /// The pointer over an off-screen view (macOS). Windows' page window takes the pointer itself.
    fn pointer(&mut self, _slot: &Slot, _p: Pointer) {}
    /// A key for an off-screen view that has the keyboard (macOS). The key filter decides first, as the
    /// accelerator hook does on Windows.
    fn key(&mut self, _slot: &Slot, _k: KeyInput) {}
    /// The input method's text for an off-screen view (macOS).
    fn ime(&mut self, _slot: &Slot, _i: ImeInput) {}
    /// The window's device scale (macOS: an off-screen view is laid out in CSS pixels at it).
    fn set_scale(&mut self, _scale: f64) {}
    /// The window takes the keyboard back from the tab's page (macOS; on Windows focusing the window does it).
    fn blur(&mut self, _tab: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{Call, FakeWeb};
    use std::sync::Mutex;

    fn collecting() -> (Sink, Arc<Mutex<Vec<WebEvent>>>) {
        let seen: Arc<Mutex<Vec<WebEvent>>> = Arc::default();
        let keep = Arc::clone(&seen);
        (Arc::new(move |e| keep.lock().unwrap().push(e)), seen)
    }

    #[test]
    fn an_event_prints_its_kind_and_never_its_data() {
        let e = WebEvent::Loaded {
            tab: "t1".into(),
            url: "https://private.example/?token=abc".into(),
            ok: true,
            status: "0".into(),
            nav: 3,
        };
        assert_eq!(format!("{e:?}"), "WebEvent::Loaded");
        let t = WebEvent::Title {
            tab: "t1".into(),
            title: "My bank".into(),
        };
        assert_eq!(format!("{t:?}"), "WebEvent::Title");
        let s = WebEvent::Still {
            tab: "t1".into(),
            token: 1,
            png: Some(vec![0; 10]),
        };
        assert_eq!(format!("{s:?}"), "WebEvent::Still");
        let r = WebEvent::Runtime(Err("no".into()));
        assert_eq!(format!("{r:?}"), "WebEvent::Runtime");
    }

    #[test]
    fn the_fake_keeps_one_view_per_tab_and_frees_it_on_close() {
        let (sink, seen) = collecting();
        let mut f = FakeWeb::new(sink);
        let r = PhysRect {
            x: 1,
            y: 2,
            w: 30,
            h: 40,
        };
        f.open("a", "https://a.example/", r, false);
        f.open("b", "https://b.example/", r, true);
        assert_eq!(f.controllers(), 2);
        f.set_bounds("a", PhysRect { w: 99, ..r });
        f.set_visible("a", true);
        let p = f.probe("a").unwrap();
        assert_eq!((p.bounds.w, p.visible), (99, true));
        f.close("a");
        assert_eq!(f.controllers(), 1);
        assert!(f.probe("a").is_none());
        assert_eq!(
            seen.lock()
                .unwrap()
                .iter()
                .map(WebEvent::kind)
                .collect::<Vec<_>>(),
            ["Ready", "Ready"]
        );
        assert!(f.calls().contains(&Call::Close("a".into())));
    }

    #[test]
    fn an_app_view_and_a_page_view_replace_each_other() {
        use flare_surfaces::app::{AppSpec, PROFILE_NAME, app_host, app_url};
        let (sink, _seen) = collecting();
        let mut f = FakeWeb::new(sink);
        let r = PhysRect {
            x: 0,
            y: 0,
            w: 10,
            h: 10,
        };
        let app = |n| AppSpec {
            host: app_host(n),
            folder: "C:\\apps".into(),
            url: app_url(&app_host(n)),
            allow_network: false,
            module: None,
        };
        f.open("a", "https://a.example/", r, true);
        assert!(!f.probe("a").unwrap().private);
        f.open_app("a", app(1), r, true);
        let p = f.probe("a").unwrap();
        assert!(
            p.private && p.profile == PROFILE_NAME,
            "its own private profile"
        );
        assert_eq!(f.controllers(), 1);
        // The same app again only moves; another app swaps the view; a page swaps it back.
        f.open_app("a", app(1), r, true);
        f.open_app("a", app(2), r, true);
        f.open("a", "https://a.example/", r, true);
        assert!(!f.probe("a").unwrap().private);
        let closes = f
            .calls()
            .iter()
            .filter(|c| matches!(c, Call::Close(_)))
            .count();
        assert_eq!(closes, 3, "{:?}", f.calls());
        assert_eq!(f.controllers(), 1);
    }

    #[test]
    fn a_mods_view_has_a_profile_of_its_own_and_is_the_only_one_a_message_reaches() {
        use flare_surfaces::modview::mod_spec;
        let (sink, _seen) = collecting();
        let mut f = FakeWeb::new(sink);
        let r = PhysRect {
            x: 0,
            y: 0,
            w: 10,
            h: 10,
        };
        f.open("page", "https://a.example/", r, true);
        f.open_app(
            "mod",
            mod_spec(
                "notes",
                std::path::Path::new("C:\\mods\\notes"),
                "index.html",
            ),
            r,
            true,
        );
        let p = f.probe("mod").unwrap();
        assert_eq!(p.profile, "flare-mod-notes");
        assert!(!p.private, "a mod keeps its own storage between runs");
        f.post_message("page", "{}");
        f.post_message("mod", r#"{"t":"tool"}"#);
        f.post_message("nowhere", "{}");
        let posts: Vec<_> = f
            .calls()
            .into_iter()
            .filter(|c| matches!(c, Call::Post(..)))
            .collect();
        assert_eq!(
            posts,
            [Call::Post("mod".into(), r#"{"t":"tool"}"#.into())],
            "only the mod's view gets a message"
        );
    }

    fn notes() -> AppSpec {
        flare_surfaces::modview::mod_spec(
            "notes",
            std::path::Path::new("C:\\mods\\notes"),
            "index.html",
        )
    }

    /// Every kind of view a plugin can have is sealed by the one seal of its kind, and nothing else is.
    #[test]
    fn every_kind_of_plugin_view_is_sealed_as_its_kind_says() {
        use crate::seal::{DownloadWall, plugin_seal};
        let (sink, _seen) = collecting();
        let mut f = FakeWeb::new(sink);
        let r = PhysRect::default();
        let slots = [
            Slot::panel("t1"),
            Slot::bar("t1"),
            Slot::background("notes"),
        ];
        for s in &slots {
            f.open_view(s, notes(), r, false);
        }
        assert_eq!(f.controllers(), 3, "three slots, three views");
        for s in &slots {
            let seal = f.view(s).and_then(|v| v.seal).expect("sealed");
            assert_eq!(seal, plugin_seal(s.kind));
            assert!(!seal.script_dialogs && !seal.new_windows && !seal.permissions);
            assert!(!seal.external_uris && seal.frames_own_only && !seal.dev_tools);
        }
        let bar = f.view(&Slot::bar("t1")).unwrap().seal.unwrap();
        let bg = f.view(&Slot::background("notes")).unwrap().seal.unwrap();
        for s in [bar, bg] {
            assert!(!s.context_menu && !s.browser_keys);
            assert_eq!(s.downloads, DownloadWall::Cancel);
        }
        let panel = f.view(&Slot::panel("t1")).unwrap().seal.unwrap();
        assert!(panel.context_menu && panel.browser_keys);
        // The web panel and a mini-app are not plugin views and get no plugin seal.
        f.open("t2", "https://a.example/", r, true);
        assert_eq!(f.view(&Slot::panel("t2")).unwrap().seal, None);
    }

    #[test]
    fn a_bar_or_a_background_slot_takes_only_a_plugins_page() {
        use flare_surfaces::app::{app_host, app_url};
        let (sink, seen) = collecting();
        let mut f = FakeWeb::new(sink);
        let mini = AppSpec {
            host: app_host(1),
            folder: "C:\\apps".into(),
            url: app_url(&app_host(1)),
            allow_network: false,
            module: None,
        };
        f.open_view(&Slot::bar("t1"), mini.clone(), PhysRect::default(), true);
        f.open_view(&Slot::background("x"), mini, PhysRect::default(), true);
        assert_eq!(f.controllers(), 0);
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert!(seen.iter().all(|e| matches!(
            e,
            WebEvent::View { event, .. } if matches!(**event, WebEvent::Failed { .. })
        )));
    }

    /// A panel's events and calls are what they always were; a bar's and a background page's name their slot,
    /// so nothing that knows only panels mistakes them for a tab's.
    #[test]
    fn a_panels_events_name_its_tab_and_other_slots_events_come_wrapped() {
        let (sink, seen) = collecting();
        let mut f = FakeWeb::new(sink);
        let r = PhysRect::default();
        f.open_app("t1", notes(), r, true);
        f.open_view(&Slot::bar("t1"), notes(), r, true);
        f.open_view(&Slot::background("notes"), notes(), r, false);
        {
            let seen = seen.lock().unwrap();
            assert_eq!(seen[0], WebEvent::Ready { tab: "t1".into() });
            assert_eq!(
                seen[1],
                WebEvent::View {
                    slot: Slot::bar("t1"),
                    event: Box::new(WebEvent::Ready { tab: "t1".into() }),
                }
            );
            assert_eq!(
                seen[2],
                WebEvent::View {
                    slot: Slot::background("notes"),
                    event: Box::new(WebEvent::Ready {
                        tab: "notes".into()
                    }),
                }
            );
            assert_eq!(seen[2].kind(), "View");
        }
        assert!(
            f.calls()
                .iter()
                .any(|c| matches!(c, Call::OpenApp { tab, .. } if tab == "t1"))
        );
        // A message reaches the slot it names, and only it.
        f.post_message("t1", "p");
        f.post_to_view(&Slot::bar("t1"), "b");
        f.post_to_view(&Slot::background("notes"), "g");
        f.post_to_view(&Slot::background("other"), "x");
        let posts: Vec<_> = f
            .calls()
            .into_iter()
            .filter(|c| matches!(c, Call::Post(..) | Call::PostView(..)))
            .collect();
        assert_eq!(
            posts,
            [
                Call::Post("t1".into(), "p".into()),
                Call::PostView(Slot::bar("t1"), "b".into()),
                Call::PostView(Slot::background("notes"), "g".into()),
            ]
        );
        // Closing the tab's panel leaves its bar and the background page; each closes on its own.
        f.close("t1");
        assert_eq!(f.controllers(), 2);
        assert!(f.probe("t1").is_none() && f.probe_view(&Slot::bar("t1")).is_some());
        f.close_view(&Slot::bar("t1"));
        f.close_all();
        assert_eq!(f.controllers(), 0);
        let closes: Vec<_> = f
            .calls()
            .into_iter()
            .filter(|c| matches!(c, Call::Close(_) | Call::CloseView(_)))
            .collect();
        assert_eq!(
            closes,
            [
                Call::Close("t1".into()),
                Call::CloseView(Slot::bar("t1")),
                Call::CloseView(Slot::background("notes")),
            ]
        );
    }

    #[test]
    fn a_failing_fake_makes_no_view() {
        let (sink, seen) = collecting();
        let mut f = FakeWeb::new(sink);
        f.state.borrow_mut().fail = Some(("nope".into(), true));
        f.open("a", "https://a.example/", PhysRect::default(), true);
        assert_eq!(f.controllers(), 0);
        assert!(matches!(
            seen.lock().unwrap().first(),
            Some(WebEvent::Failed {
                missing_runtime: true,
                ..
            })
        ));
    }

    #[test]
    fn a_still_answers_with_the_callers_token() {
        let (sink, seen) = collecting();
        let mut f = FakeWeb::new(sink);
        f.capture_still("a", 42);
        assert!(matches!(
            seen.lock().unwrap().first(),
            Some(WebEvent::Still {
                token: 42,
                png: None,
                ..
            })
        ));
    }
}
