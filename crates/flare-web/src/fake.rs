//! A web backend with no browser, for the panel logic's tests: it keeps the views it was asked for,
//! records every call and lets a test play the browser's part by sending events.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use flare_surfaces::web::PhysRect;

use crate::backend::{ControllerProbe, KeyFilter, Nav, Sink, WebBackend, WebEvent};

/// One call the window made.
#[derive(Clone, Debug, PartialEq)]
pub enum Call {
    Open {
        tab: String,
        url: String,
        bounds: PhysRect,
        visible: bool,
    },
    Navigate(String, String),
    Go(String, Nav),
    Bounds(String, PhysRect),
    Visible(String, bool),
    Capture(String, u64),
    Focus(String),
    Close(String),
    /// `close_all` (the window is ending).
    CloseAll,
    /// The answer to a download: its id and whether it may be saved.
    Download(u64, bool),
    Script(String, u64, String),
}

/// A view as the fake holds it.
#[derive(Clone, Debug, PartialEq)]
pub struct FakeView {
    pub url: String,
    pub bounds: PhysRect,
    pub visible: bool,
}

/// What a test sees of the fake from outside (the window owns the fake itself).
#[derive(Default)]
pub struct FakeState {
    pub calls: Vec<Call>,
    pub views: BTreeMap<String, FakeView>,
    /// When set, `open` answers `Failed` instead of making a view.
    pub fail: Option<(String, bool)>,
    /// When set, a still answers with this PNG (else None: it could not be taken).
    pub still: Option<Vec<u8>>,
    /// When set, a still is never answered (a page that is hung).
    pub still_silent: bool,
    pub filter: Option<KeyFilter>,
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
}

impl WebBackend for FakeWeb {
    fn open(&mut self, tab: &str, url: &str, bounds: PhysRect, visible: bool) {
        let mut s = self.state.borrow_mut();
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
            tab.into(),
            FakeView {
                url: url.into(),
                bounds,
                visible,
            },
        );
        drop(s);
        (self.sink)(WebEvent::Ready { tab: tab.into() });
    }

    fn navigate(&mut self, tab: &str, url: &str) {
        let mut s = self.state.borrow_mut();
        s.calls.push(Call::Navigate(tab.into(), url.into()));
        if let Some(v) = s.views.get_mut(tab) {
            v.url = url.into();
        }
    }

    fn go(&mut self, tab: &str, nav: Nav) {
        self.state
            .borrow_mut()
            .calls
            .push(Call::Go(tab.into(), nav));
    }

    fn set_bounds(&mut self, tab: &str, bounds: PhysRect) {
        let mut s = self.state.borrow_mut();
        s.calls.push(Call::Bounds(tab.into(), bounds));
        if let Some(v) = s.views.get_mut(tab) {
            v.bounds = bounds;
        }
    }

    fn set_visible(&mut self, tab: &str, visible: bool) {
        let mut s = self.state.borrow_mut();
        s.calls.push(Call::Visible(tab.into(), visible));
        if let Some(v) = s.views.get_mut(tab) {
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

    fn close(&mut self, tab: &str) {
        let mut s = self.state.borrow_mut();
        s.calls.push(Call::Close(tab.into()));
        s.views.remove(tab);
    }

    fn close_all(&mut self) {
        let mut s = self.state.borrow_mut();
        let tabs: Vec<String> = s.views.keys().cloned().collect();
        for t in tabs {
            s.calls.push(Call::Close(t));
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

    fn run_script(&mut self, tab: &str, id: u64, js: &str) {
        self.state
            .borrow_mut()
            .calls
            .push(Call::Script(tab.into(), id, js.into()));
    }

    fn probe(&self, tab: &str) -> Option<ControllerProbe> {
        let s = self.state.borrow();
        s.views.get(tab).map(|v| ControllerProbe {
            bounds: v.bounds,
            visible: v.visible,
            url: v.url.clone(),
            ..ControllerProbe::default()
        })
    }
}
