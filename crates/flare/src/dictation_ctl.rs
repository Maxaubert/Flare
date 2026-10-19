//! The dictation controller on the UI thread (M8). Scaffolding only: Task 1 pre-wires every call site
//! (Appendix D of the M8 plan) with no behaviour, so no later task edits a shared file twice. Task 22
//! fills the bodies; Task 23 reads `view()`; Task 24 reads `wired()`; Task 27 fills the `e2e_*` arms.
#![allow(dead_code)] // scaffolding: each task removes what it fills

use std::cell::RefCell;
use std::rc::Rc;

use flare_e2e_proto::RouteResult;
use flare_term::keys::KeyEvent;

use crate::AppWindow;
use crate::appctl::AppCtl;
use crate::config::Args;

/// The key path's side of dictation, held by the window controller.
#[derive(Default)]
pub struct DictationKeys;

impl DictationKeys {
    /// Offers a key to dictation first (Prism Terminal's capture phase). `up` is true for a key
    /// release. Returns whether dictation swallowed it; a swallowed key goes nowhere else.
    pub fn feed_key(&mut self, _ev: &KeyEvent, _up: bool) -> bool {
        false
    }
}

/// What the pill and the tab mark draw. Empty until Task 22.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DictView {
    pub active: bool,
}

/// Builds the controller, the service and the widget. Nothing is built while dictation is off.
pub fn install(_app: &AppWindow, _ctl: &Rc<RefCell<AppCtl>>, _args: &Args) {}

/// The window lost the keyboard focus.
pub fn on_blur() {}

/// Quit: stops the engine, the host, the hook and the link before the shells are killed.
pub fn stop_all() {}

/// Whether the Dictation page is wired (Settings' find index and the page env read it).
pub fn wired() -> bool {
    false
}

/// What the pill and the mark show now.
pub fn view() -> DictView {
    DictView::default()
}

const NOT_BUILT: &str = "dictation is not built yet (Task 22)";

/// The test hook's `DictLine`: one helper line, as if the keyboard hook had printed it.
pub fn e2e_line(_line: &str) -> Result<(), String> {
    Err(NOT_BUILT.into())
}

/// The test hook's `DictRoute`: `text` was heard with the window of process `front` in front.
pub fn e2e_route(_text: &str, _front: Option<u32>) -> Result<(), String> {
    Err(NOT_BUILT.into())
}

/// The test hook's `DictRouted`: what the window hands main as heard.
pub fn e2e_routed(_result: &RouteResult) -> Result<(), String> {
    Err(NOT_BUILT.into())
}
