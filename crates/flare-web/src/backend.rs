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
            WebEvent::Gone { .. } => "Gone",
            WebEvent::Script { .. } => "Script",
            WebEvent::Cdp { .. } => "Cdp",
            WebEvent::ModMessage { .. } => "ModMessage",
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
}

/// One web view per tab with an open web panel.
pub trait WebBackend {
    /// Makes the tab's view (asynchronously, the first time also the browser environment) and
    /// opens `url` in it at `bounds`. A view the tab already has is only moved and sent to `url`.
    fn open(&mut self, tab: &str, url: &str, bounds: PhysRect, visible: bool);
    /// Makes the tab's view for a mini-app (Surfaces stage 5): a view of the apps' own private profile,
    /// with the app's folder served on its host name, a navigation that may not leave the app's page, no
    /// window of its own for the page and, unless `app.allow_network`, no request that is not the app's
    /// own. A view the tab already has (of anything else) is closed first.
    fn open_app(&mut self, tab: &str, app: AppSpec, bounds: PhysRect, visible: bool);
    fn navigate(&mut self, tab: &str, url: &str);
    fn go(&mut self, tab: &str, nav: Nav);
    fn set_bounds(&mut self, tab: &str, bounds: PhysRect);
    fn set_visible(&mut self, tab: &str, visible: bool);
    /// Takes a still of the page; answered by `WebEvent::Still` with the same token.
    fn capture_still(&mut self, tab: &str, token: u64);
    /// Gives the page the keyboard.
    fn focus(&mut self, tab: &str);
    /// Closes and frees the tab's view. The browser environment goes with the last one.
    fn close(&mut self, tab: &str);
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
    fn post_message(&mut self, tab: &str, json: &str);
    /// The e2e only: sets the browser's own zoom of the tab's page (what Ctrl+wheel in the panel does).
    fn set_zoom(&mut self, tab: &str, factor: f64);
    /// The e2e only: runs a script in the page; answered by `WebEvent::Script`.
    fn run_script(&mut self, tab: &str, id: u64, js: &str);
    /// The e2e only: the controller's own numbers.
    fn probe(&self, tab: &str) -> Option<ControllerProbe>;
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
