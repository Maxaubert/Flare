//! The web panel's browser off Windows. macOS: phase 1 (the WKWebView spike) and phase 5 (the
//! backend) of the port plan. Until then every view fails to open with a sentence, scripts and
//! DevTools calls are answered with an error, and nothing else happens.

use std::path::PathBuf;
use std::sync::Arc;

use flare_surfaces::app::AppSpec;
use flare_surfaces::web::PhysRect;

use crate::backend::{ControllerProbe, KeyFilter, Nav, Sink, Slot, ViewSink, WebBackend, WebEvent};

const NOT_YET: &str = "the web panel is not built on this platform yet";

/// Is a browser runtime there? The version string, or why not (the same shape as on Windows).
pub type RuntimeCheck = Arc<dyn Fn() -> Result<String, String> + Send + Sync>;

/// No browser runtime until phase 5.
pub fn installed_runtime() -> Result<String, String> {
    Err(NOT_YET.to_string())
}

/// What the host is told at the start (the same fields as on Windows).
pub struct HostConfig {
    pub parent: isize,
    pub data_dir: PathBuf,
    pub sink: Sink,
    pub runtime: RuntimeCheck,
    pub virtual_hosts: Vec<(String, PathBuf)>,
    pub without_webrtc_script: bool,
    pub limits: Option<Arc<flare_mods::limit::Limits>>,
}

/// A host with no browser: every view fails to open.
pub struct WebHost {
    sink: Sink,
}

impl WebHost {
    pub fn new(cfg: HostConfig) -> Self {
        Self { sink: cfg.sink }
    }

    fn refuse(&self, slot: &Slot) {
        ViewSink::new(&self.sink, slot).send(|tab| WebEvent::Failed {
            tab,
            why: NOT_YET.to_string(),
            missing_runtime: false,
        });
    }
}

impl WebBackend for WebHost {
    fn open(&mut self, tab: &str, _url: &str, _bounds: PhysRect, _visible: bool) {
        self.refuse(&Slot::panel(tab));
    }

    fn open_view(&mut self, slot: &Slot, _app: AppSpec, _bounds: PhysRect, _visible: bool) {
        self.refuse(slot);
    }

    fn navigate(&mut self, _tab: &str, _url: &str) {}

    fn go(&mut self, _tab: &str, _nav: Nav) {}

    fn set_view_bounds(&mut self, _slot: &Slot, _bounds: PhysRect) {}

    fn set_view_visible(&mut self, _slot: &Slot, _visible: bool) {}

    fn capture_still(&mut self, tab: &str, token: u64) {
        // No view can exist yet, so there is never a still: answer at once, as Windows does.
        (self.sink)(WebEvent::Still {
            tab: tab.to_string(),
            token,
            png: None,
        });
    }

    fn focus(&mut self, _tab: &str) {}

    fn close_view(&mut self, _slot: &Slot) {}

    fn close_popup(&mut self, _tab: &str) {}

    fn close_all(&mut self) {}

    fn answer_download(&mut self, _id: u64, _save: bool) {}

    fn pump(&mut self) {}

    fn runtime_checked(&mut self, _result: Result<String, String>) {}

    fn controllers(&self) -> usize {
        0
    }

    fn set_key_filter(&mut self, _filter: KeyFilter) {}

    fn cdp(&mut self, tab: &str, id: u64, _method: &str, _params: &str) {
        (self.sink)(WebEvent::Cdp {
            tab: tab.to_string(),
            id,
            result: Err(NOT_YET.to_string()),
        });
    }

    fn post_to_view(&mut self, _slot: &Slot, _json: &str) {}

    fn set_zoom(&mut self, _tab: &str, _factor: f64) {}

    fn run_script(&mut self, _tab: &str, id: u64, _js: &str) {
        (self.sink)(WebEvent::Script {
            id,
            result: Err(NOT_YET.to_string()),
        });
    }

    fn probe_view(&self, _slot: &Slot) -> Option<ControllerProbe> {
        None
    }
}
