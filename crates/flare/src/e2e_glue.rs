//! The UI-thread side of the test hook: the one dispatcher for every request the pipe server hands
//! over (Tasks 8 to 11 fill its arms), and the render hooks the window's notifier calls (Appendix F).
//! Nothing here does anything unless `--e2e` is on.

use std::cell::RefCell;
use std::rc::Rc;

use crate::AppWindow;
use crate::appctl::AppCtl;
use crate::config::Args;

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

/// Starts the hook for an `--e2e` run (Task 8 starts the pipe server here). Task 1 only creates the
/// probe registry the hook will answer from.
pub fn install(app: &AppWindow, ctl: &Rc<RefCell<AppCtl>>, args: &Args) {
    let _ = (app, ctl, args);
    crate::e2e_probes::register_all(&mut crate::e2e_probes::ProbeRegistry);
}
