//! A web backend with no browser, for the panel logic's tests: it keeps the views it was asked for (by
//! slot), records every call and lets a test play the browser's part by sending events. A tab's panel is
//! recorded as it always was (`Call::OpenApp`, `Call::Close` with the tab); a bar or a background page has calls
//! of its own that name the slot.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use flare_surfaces::app::AppSpec;
use flare_surfaces::web::PhysRect;

use crate::backend::{ControllerProbe, KeyFilter, Nav, Sink, Slot, ViewSink, WebBackend, WebEvent};
use crate::seal::{Seal, plugin_seal};

/// One call the window made.
#[derive(Clone, Debug, PartialEq)]
pub enum Call {
    Open {
        tab: String,
        url: String,
        bounds: PhysRect,
        visible: bool,
    },
    /// A mini-app view was asked for.
    OpenApp {
        tab: String,
        app: AppSpec,
        bounds: PhysRect,
        visible: bool,
    },
    Navigate(String, String),
    Go(String, Nav),
    Bounds(String, PhysRect),
    Visible(String, bool),
    /// The parent window moved (the browser was told).
    ParentMoved,
    Capture(String, u64),
    Focus(String),
    Close(String),
    /// `close_all` (the window is ending).
    CloseAll,
    /// The answer to a download: its id and whether it may be saved.
    Download(u64, bool),
    Script(String, u64, String),
    Zoom(String, f64),
    /// A DevTools Protocol call: tab, id, method and params.
    Cdp(String, u64, String, String),
    /// A message posted to a mod's page: tab and JSON text.
    Post(String, String),
    /// A view was asked for in a slot that is not a tab's panel (a bar, a background page).
    OpenView {
        slot: Slot,
        app: AppSpec,
        bounds: PhysRect,
        visible: bool,
    },
    ViewBounds(Slot, PhysRect),
    ViewVisible(Slot, bool),
    CloseView(Slot),
    /// A message posted to the page in a slot that is not a tab's panel.
    PostView(Slot, String),
}

/// How a test plays the page's DevTools: method and params in, the answer out.
pub type CdpResponder = Box<dyn FnMut(&str, &str) -> Result<String, String>>;

/// A view as the fake holds it.
#[derive(Clone, Debug, PartialEq)]
pub struct FakeView {
    pub url: String,
    pub app: Option<AppSpec>,
    pub bounds: PhysRect,
    pub visible: bool,
    /// The walls of a mod's or plugin's view, as the host would apply them (None for anything else).
    pub seal: Option<Seal>,
}

/// What a test sees of the fake from outside (the window owns the fake itself).
#[derive(Default)]
pub struct FakeState {
    pub calls: Vec<Call>,
    pub views: BTreeMap<Slot, FakeView>,
    /// When set, `open` answers `Failed` instead of making a view.
    pub fail: Option<(String, bool)>,
    /// When set, a still answers with this PNG (else None: it could not be taken).
    pub still: Option<Vec<u8>>,
    /// When set, a still is never answered (a page that is hung).
    pub still_silent: bool,
    pub filter: Option<KeyFilter>,
    /// Answers a DevTools call at once with this (else the call stays unanswered, like a hung page).
    pub cdp: Option<CdpResponder>,
}

#[derive(Clone)]
pub struct FakeWeb {
    pub state: Rc<RefCell<FakeState>>,
    sink: Sink,
}

impl FakeWeb {
    pub fn new(sink: Sink) -> Self {
        Self {
            state: Rc::new(RefCell::new(FakeState::default())),
            sink,
        }
    }

    /// Plays the browser: tells the window something.
    pub fn say(&self, e: WebEvent) {
        (self.sink)(e);
    }

    pub fn calls(&self) -> Vec<Call> {
        self.state.borrow().calls.clone()
    }

    /// The view of `slot`, as the fake holds it.
    pub fn view(&self, slot: &Slot) -> Option<FakeView> {
        self.state.borrow().views.get(slot).cloned()
    }
}

/// The call that closes the view of `slot`: a tab's panel is recorded by its tab, as it always was.
fn close_call(slot: &Slot) -> Call {
    if slot.is_panel() {
        Call::Close(slot.owner.clone())
    } else {
        Call::CloseView(slot.clone())
    }
}

impl WebBackend for FakeWeb {
    fn open(&mut self, tab: &str, url: &str, bounds: PhysRect, visible: bool) {
        let slot = Slot::panel(tab);
        let mut s = self.state.borrow_mut();
        // A view of the other kind is closed first, as the real host does.
        if s.views.get(&slot).is_some_and(|v| v.app.is_some()) {
            s.calls.push(Call::Close(tab.into()));
            s.views.remove(&slot);
        }
        s.calls.push(Call::Open {
            tab: tab.into(),
            url: url.into(),
            bounds,
            visible,
        });
        if let Some((why, missing_runtime)) = s.fail.clone() {
            drop(s);
            (self.sink)(WebEvent::Failed {
                tab: tab.into(),
                why,
                missing_runtime,
            });
            return;
        }
        s.views.insert(
            slot,
            FakeView {
                url: url.into(),
                app: None,
                bounds,
                visible,
                seal: None,
            },
        );
        drop(s);
        (self.sink)(WebEvent::Ready { tab: tab.into() });
    }

    fn open_view(&mut self, slot: &Slot, app: AppSpec, bounds: PhysRect, visible: bool) {
        let events = ViewSink::new(&self.sink, slot);
        let mut s = self.state.borrow_mut();
        // A view of anything else (another app, a page) is closed first, as the real host does.
        if s.views
            .get(slot)
            .is_some_and(|v| v.app.as_ref().map(|a| &a.host) != Some(&app.host))
        {
            s.calls.push(close_call(slot));
            s.views.remove(slot);
        }
        s.calls.push(if slot.is_panel() {
            Call::OpenApp {
                tab: slot.owner.clone(),
                app: app.clone(),
                bounds,
                visible,
            }
        } else {
            Call::OpenView {
                slot: slot.clone(),
                app: app.clone(),
                bounds,
                visible,
            }
        });
        let refused = (!slot.is_panel() && app.module.is_none())
            .then(|| ("only a plugin's page opens here".to_string(), false));
        if let Some((why, missing_runtime)) = refused.or_else(|| s.fail.clone()) {
            drop(s);
            events.send(|tab| WebEvent::Failed {
                tab,
                why,
                missing_runtime,
            });
            return;
        }
        let seal = app.module.is_some().then(|| plugin_seal(slot.kind));
        s.views.insert(
            slot.clone(),
            FakeView {
                url: app.url.clone(),
                app: Some(app),
                bounds,
                visible,
                seal,
            },
        );
        drop(s);
        events.send(|tab| WebEvent::Ready { tab });
    }

    fn navigate(&mut self, tab: &str, url: &str) {
        let mut s = self.state.borrow_mut();
        s.calls.push(Call::Navigate(tab.into(), url.into()));
        if let Some(v) = s.views.get_mut(&Slot::panel(tab)) {
            v.url = url.into();
        }
    }

    fn go(&mut self, tab: &str, nav: Nav) {
        self.state
            .borrow_mut()
            .calls
            .push(Call::Go(tab.into(), nav));
    }

    fn set_view_bounds(&mut self, slot: &Slot, bounds: PhysRect) {
        let mut s = self.state.borrow_mut();
        s.calls.push(if slot.is_panel() {
            Call::Bounds(slot.owner.clone(), bounds)
        } else {
            Call::ViewBounds(slot.clone(), bounds)
        });
        if let Some(v) = s.views.get_mut(slot) {
            v.bounds = bounds;
        }
    }

    fn parent_moved(&mut self) {
        self.state.borrow_mut().calls.push(Call::ParentMoved);
    }

    fn set_view_visible(&mut self, slot: &Slot, visible: bool) {
        let mut s = self.state.borrow_mut();
        s.calls.push(if slot.is_panel() {
            Call::Visible(slot.owner.clone(), visible)
        } else {
            Call::ViewVisible(slot.clone(), visible)
        });
        if let Some(v) = s.views.get_mut(slot) {
            v.visible = visible;
        }
    }

    fn capture_still(&mut self, tab: &str, token: u64) {
        let png = {
            let mut s = self.state.borrow_mut();
            s.calls.push(Call::Capture(tab.into(), token));
            if s.still_silent {
                return;
            }
            s.still.clone()
        };
        (self.sink)(WebEvent::Still {
            tab: tab.into(),
            token,
            png,
        });
    }

    fn focus(&mut self, tab: &str) {
        self.state.borrow_mut().calls.push(Call::Focus(tab.into()));
    }

    fn close_view(&mut self, slot: &Slot) {
        let mut s = self.state.borrow_mut();
        s.calls.push(close_call(slot));
        s.views.remove(slot);
    }

    fn close_all(&mut self) {
        let mut s = self.state.borrow_mut();
        let slots: Vec<Slot> = s.views.keys().cloned().collect();
        for slot in slots {
            s.calls.push(close_call(&slot));
        }
        s.calls.push(Call::CloseAll);
        s.views.clear();
    }

    fn answer_download(&mut self, id: u64, save: bool) {
        self.state.borrow_mut().calls.push(Call::Download(id, save));
    }

    fn pump(&mut self) {}

    fn runtime_checked(&mut self, _result: Result<String, String>) {}

    fn controllers(&self) -> usize {
        self.state.borrow().views.len()
    }

    fn set_key_filter(&mut self, filter: KeyFilter) {
        self.state.borrow_mut().filter = Some(filter);
    }

    fn cdp(&mut self, tab: &str, id: u64, method: &str, params: &str) {
        let answer = {
            let mut s = self.state.borrow_mut();
            s.calls
                .push(Call::Cdp(tab.into(), id, method.into(), params.into()));
            if !s.views.contains_key(&Slot::panel(tab)) {
                Some(Err("no web view".to_string()))
            } else {
                s.cdp.as_mut().map(|f| f(method, params))
            }
        };
        if let Some(result) = answer {
            (self.sink)(WebEvent::Cdp {
                tab: tab.into(),
                id,
                result,
            });
        }
    }

    fn post_to_view(&mut self, slot: &Slot, json: &str) {
        let mut s = self.state.borrow_mut();
        // Only a mod's view has a bridge.
        if s.views
            .get(slot)
            .is_some_and(|v| v.app.as_ref().is_some_and(|a| a.module.is_some()))
        {
            s.calls.push(if slot.is_panel() {
                Call::Post(slot.owner.clone(), json.into())
            } else {
                Call::PostView(slot.clone(), json.into())
            });
        }
    }

    fn set_zoom(&mut self, tab: &str, factor: f64) {
        self.state
            .borrow_mut()
            .calls
            .push(Call::Zoom(tab.into(), factor));
    }

    fn run_script(&mut self, tab: &str, id: u64, js: &str) {
        self.state
            .borrow_mut()
            .calls
            .push(Call::Script(tab.into(), id, js.into()));
    }

    fn probe_view(&self, slot: &Slot) -> Option<ControllerProbe> {
        let s = self.state.borrow();
        s.views.get(slot).map(|v| ControllerProbe {
            bounds: v.bounds,
            visible: v.visible,
            url: v.url.clone(),
            profile: v.app.as_ref().map_or(String::new(), |a| match &a.module {
                // A mod keeps its own profile, apart from the mini-apps' private one.
                Some(m) => flare_surfaces::modview::profile_name(&m.id),
                None => flare_surfaces::app::PROFILE_NAME.to_string(),
            }),
            private: v.app.as_ref().is_some_and(|a| a.module.is_none()),
            autofill: v.app.is_none(),
            ..ControllerProbe::default()
        })
    }
}
