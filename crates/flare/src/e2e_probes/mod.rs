//! The read-only probes the test hook answers from: one file per scenario group, plus core (Task 11).
//! Each file's `r`egister adds its probes to the registry. Nothing here runs unless --e2e is on,
//! because 2e_glue::install is the only caller and it runs only under --e2e.
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use flare_win::clipboard_port::ClipboardPort;

use crate::AppWindow;
use crate::appctl::AppCtl;
use crate::config::Args;

pub mod agent_hooks;
pub mod agent_launch;
pub mod agent_misc;
pub mod chrome;
pub mod closeask;
pub mod core;
pub mod diag;
pub mod dictation;
pub mod dpi;
pub mod help;
pub mod indicators;
pub mod input_a;
pub mod input_b;
pub mod input_c;
pub mod input_d;
pub mod lifecycle;
pub mod panel;
pub mod resize;
pub mod settings_a;
pub mod settings_b;
pub mod settings_c;
pub mod settings_d;
pub mod strip;
pub mod update;
pub mod vertical;
pub mod web;

/// What a probe may look at. A probe is read-only: it never changes the controller or the window.
pub struct ProbeCtx<'a> {
    pub app: &'a AppWindow,
    pub ctl: &'a Rc<RefCell<AppCtl>>,
    pub args: &'a Args,
    /// The in-memory clipboard under `--e2e` (None in a run that has none).
    pub clip: Option<&'a Arc<dyn ClipboardPort>>,
    /// The shell in front, as the key path holds it (the other tabs' terminals are not reachable
    /// from a probe yet).
    pub front: Option<Arc<flare_term::handle::TermHandle>>,
}

/// One probe: its argument in, its answer (or a reason) out.
pub type ProbeFn = fn(&ProbeCtx, &serde_json::Value) -> Result<serde_json::Value, String>;

/// The table of named probes (dotted names, each registered once).
#[derive(Debug, Default)]
pub struct ProbeRegistry {
    map: BTreeMap<&'static str, ProbeFn>,
}

impl ProbeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `f` under `name`. A name registered twice is a bug in the probe files.
    pub fn add(&mut self, name: &'static str, f: ProbeFn) {
        let first = self.map.insert(name, f).is_none();
        debug_assert!(first, "the probe {name:?} is registered twice");
    }

    /// Runs the probe `name`; an unknown name is `Err("no such probe: <name>")`.
    pub fn call(
        &self,
        name: &str,
        ctx: &ProbeCtx,
        arg: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        match self.map.get(name) {
            Some(f) => f(ctx, arg),
            None => Err(format!("no such probe: {name}")),
        }
    }

    /// Every registered name, sorted.
    pub fn names(&self) -> Vec<&'static str> {
        self.map.keys().copied().collect()
    }
}
/// Adds every group's probes to `r`.
pub fn register_all(r: &mut ProbeRegistry) {
    agent_hooks::register(r);
    agent_launch::register(r);
    agent_misc::register(r);
    chrome::register(r);
    closeask::register(r);
    core::register(r);
    diag::register(r);
    dictation::register(r);
    dpi::register(r);
    help::register(r);
    indicators::register(r);
    input_a::register(r);
    input_b::register(r);
    input_c::register(r);
    input_d::register(r);
    lifecycle::register(r);
    panel::register(r);
    resize::register(r);
    settings_a::register(r);
    settings_b::register(r);
    settings_c::register(r);
    settings_d::register(r);
    strip::register(r);
    update::register(r);
    vertical::register(r);
    web::register(r);
}
