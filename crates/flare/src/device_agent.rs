//! The device panel's agent half (issue #248, PR 5): the `device_*` calls of the agent in a tab, on that tab's own
//! panel only (rule 1322), and its touch indicators (rule 1321).
//!
//! A call reaches the controller as an [`AgentCall`] from `appctl/device_glue.rs` (the call's thread checked its
//! params; the glue checked that the tab is a local shell tab). Input calls (tap, swipe, type, press) run as a
//! [`Job`] on the controller's 16 ms agent tick: the finger dot glides to the point first (the agent cursor's timing),
//! then the input goes, then the job waits for the sim host's next "changed" count or 300 ms (a second for the first
//! key of a session). A look asks the source for a PNG at point size and the UI tree, and answers once both came; the
//! tree's refs are kept for the tab's next tap. A boot shows the device in the tab's panel and answers once it is
//! booted. A shutdown is refused for a device Flare did not boot.
//!
//! The person's press, key or scroll in the panel ends a running input call with `interrupted` (rule 1320), lets go of
//! what it held down and hides the indicators at once. A cancel (rule 229) ends it at once too. Every answer goes back
//! on the call's own reply channel; nothing here blocks.

use std::collections::{HashMap, HashSet, VecDeque};

use base64::Engine as _;
use flare_ctl::{CancelFlag, CtlError, code};
use flare_device::indicator::{IndicatorFrame, Indicators};
use flare_device::job::{DeviceOp, Fail, Input, Job, JobError, Out, TAP_MS};
use flare_device::tree::{RefMap, UiNode, filter_tree};
use serde_json::{Value, json};
use slint::{ModelRc, VecModel};

use super::*;
use crate::DeviceTrailPoint;

/// Where an agent call's answer goes (the control channel's worker waits on it).
pub type Reply = std::sync::mpsc::Sender<Result<Value, CtlError>>;

/// The timer of the agent's jobs and indicators.
pub const AGENT_TICK: &str = "device.agent";
/// How often it ticks while a job runs or an indicator moves (one frame at 60 Hz).
pub const AGENT_TICK_MS: u64 = 16;

/// What a tab whose session runs on another machine is told (rule 1323: Flare never boots a device no window of its
/// own shows).
pub const REMOTE_WORDS: &str = "the device panel cannot show in a remote session yet";

/// One `device.*` call, its params already checked on the call's thread.
#[derive(Clone, Debug)]
pub enum AgentReq {
    List,
    /// A device by target or name, or with neither the tab's last device, else the newest iPhone.
    Boot {
        target: Option<Target>,
        name: Option<String>,
    },
    Shutdown,
    Look {
        tree: bool,
    },
    Ui,
    Act(DeviceOp),
    Launch {
        app: String,
    },
    /// An `.app` folder, its symlinks resolved and its place checked (the tab's folder or DerivedData).
    Install {
        path: PathBuf,
    },
    State,
}

/// An agent call as the controller gets it.
pub struct AgentCall {
    pub tab: String,
    pub req: AgentReq,
    pub reply: Reply,
    pub cancel: CancelFlag,
    /// The agent's name for the dot's tag and the badge ("Claude", "Codex", "Agent").
    pub label: String,
}

/// A running input call.
pub(super) struct AgentJob {
    job: Job,
    target: Target,
    reply: Reply,
    cancel: CancelFlag,
    /// A tap held longer than a tap: the dot is drawn filled while the finger is down.
    long: bool,
}

/// A request the agent's call waits on.
#[derive(Clone, Debug)]
pub(super) enum AgentWait {
    /// A look's picture (true) or tree (false).
    Look(u64, bool),
    Ui {
        tab: String,
        reply: Reply,
        target: Target,
    },
    /// Launch, install: answered with `said` once done.
    Done { reply: Reply, said: Value },
    Shutdown {
        tab: String,
        reply: Reply,
        target: Target,
    },
    /// A job's pasted text (typing the keymap cannot): a failure ends the job.
    Paste { tab: String },
}

/// A look waiting for its picture and tree.
pub(super) struct LookCall {
    tab: String,
    reply: Reply,
    target: Target,
    tree: bool,
    png: Option<Vec<u8>>,
    node: Option<UiNode>,
    left: u8,
}

/// What a deferred call waits for before it starts again.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Gate {
    /// The device list.
    List,
    /// The device's attach (its frames and input connection).
    Attach(Target),
}

/// The agent's half of the controller.
pub(super) struct Agent {
    pub(super) jobs: HashMap<String, AgentJob>,
    looks: HashMap<u64, LookCall>,
    next_look: u64,
    /// Boot calls waiting for their device, with the tab each shows it in.
    boots: Vec<(Target, Reply)>,
    deferred: Vec<(AgentCall, Gate)>,
    /// Each tab's refs of its last look, for the next tap.
    refs: HashMap<String, RefMap>,
    ind: Indicators,
    /// The tab the indicators belong to (drawn only while it is the front device panel).
    ind_tab: Option<String>,
    label: String,
    held: bool,
    pub(super) ticking: bool,
    pub(super) effects: Vec<Effect>,
    /// Devices whose input session typed a key already (the first key waits longer).
    typed: HashSet<Target>,
    /// The tab's last call was ended by the person.
    took_over: HashSet<String>,
    /// What the window last showed of the indicators (to set only what changed).
    pub(super) pushed: Option<IndicatorFrame>,
    /// (ms, frames shown) of the last second or two, for `device_state`'s frames a second.
    fps: VecDeque<(u64, u64)>,
    /// Animation effects are on (Settings; the e2e seam): off places the dot at once.
    motion: std::rc::Rc<dyn Fn() -> bool>,
}

impl Default for Agent {
    fn default() -> Self {
        Self {
            jobs: HashMap::new(),
            looks: HashMap::new(),
            next_look: 0,
            boots: Vec::new(),
            deferred: Vec::new(),
            refs: HashMap::new(),
            ind: Indicators::default(),
            ind_tab: None,
            label: String::new(),
            held: false,
            ticking: false,
            effects: Vec::new(),
            typed: HashSet::new(),
            took_over: HashSet::new(),
            pushed: None,
            fps: VecDeque::new(),
            motion: std::rc::Rc::new(crate::e2e::animations_enabled),
        }
    }
}

fn ctl_err(e: JobError) -> CtlError {
    CtlError::new(e.fail.code(), e.message)
}

fn fail(f: Fail, m: impl Into<String>) -> CtlError {
    CtlError::new(f.code(), m.into())
}

fn send(reply: &Reply, r: Result<Value, CtlError>) {
    // The caller may have gone (its deadline passed): nobody reads the answer then.
    let _ = reply.send(r);
}

fn source_err(e: flare_device::source::SourceError) -> CtlError {
    fail(e.fail, e.message)
}

fn device_json(d: &DeviceInfo) -> Value {
    json!({
        "target": d.target.name(),
        "name": d.name,
        "kind": d.kind,
        "os": d.os,
        "state": d.state,
        "booted_by_flare": d.booted_by_flare,
    })
}

impl DeviceCtl {
    // ---- starting a call -----------------------------------------------------------------------------

    /// An agent call on the tab's own device panel. Answers on its reply, now or later (rule 34).
    pub fn start_agent(&mut self, call: AgentCall) {
        if call.cancel.is_set() {
            send(
                &call.reply,
                Err(fail(Fail::Interrupted, "the call was cancelled")),
            );
            return;
        }
        if !self.offered() {
            send(
                &call.reply,
                Err(fail(
                    Fail::Unavailable,
                    "the device panel is not available in this Flare",
                )),
            );
            return;
        }
        // The real source starts with a call too (an agent may boot before any panel is open).
        self.ensure_source();
        self.drain();
        if let Err(e) = self.xcode_ready() {
            send(&call.reply, Err(e));
            return;
        }
        match call.req.clone() {
            AgentReq::State => {
                let v = self.agent_state(&call.tab);
                send(&call.reply, Ok(v));
            }
            AgentReq::List => {
                if self.devices.is_none() {
                    return self.defer(call, Gate::List);
                }
                let shown = self.target_of(&call.tab).map(|t| t.name());
                let list: Vec<Value> = self.devices.iter().flatten().map(device_json).collect();
                send(&call.reply, Ok(json!({"devices": list, "panel": shown})));
            }
            AgentReq::Boot { target, name } => self.agent_boot(call, target, name),
            AgentReq::Shutdown => self.agent_shutdown(call),
            AgentReq::Look { .. } | AgentReq::Ui | AgentReq::Act(_) => self.agent_on_screen(call),
            AgentReq::Launch { app } => {
                let Some(t) = self.live_target(&call) else {
                    return;
                };
                if let Some(id) = self.source().map(|s| s.launch(&t, &app)) {
                    self.pending.insert(
                        id,
                        Pending::Agent(AgentWait::Done {
                            reply: call.reply,
                            said: json!({"launched": app}),
                        }),
                    );
                }
                self.drain();
            }
            AgentReq::Install { path } => {
                let Some(t) = self.live_target(&call) else {
                    return;
                };
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if let Some(id) = self.source().map(|s| s.install(&t, &path)) {
                    self.pending.insert(
                        id,
                        Pending::Agent(AgentWait::Done {
                            reply: call.reply,
                            said: json!({"installed": name}),
                        }),
                    );
                }
                self.drain();
            }
        }
    }

    /// The agent's device tools work here: the fake source (`--e2e`), or the real one where the Xcode pick found an
    /// Xcode (macOS on Apple silicon). The window adds "and it shows" (`sync_device_gate`).
    pub fn agent_offered(&self) -> bool {
        self.menu_offered()
    }

    /// Where "animation effects on" is read (tests pass their own).
    #[allow(dead_code)] // tests
    pub fn set_motion(&mut self, motion: std::rc::Rc<dyn Fn() -> bool>) {
        self.agent.motion = motion;
    }

    /// The effects an agent call asked of the window (a panel to show or close).
    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.agent.effects)
    }

    /// Xcode can run a simulator here, or why not.
    fn xcode_ready(&self) -> Result<(), CtlError> {
        if self.real.as_ref().is_some_and(|r| r.xcode.is_none()) {
            return Err(fail(
                Fail::Busy,
                "Flare is still looking for Xcode; try again in a moment",
            ));
        }
        match self.xcode {
            Xcode::Missing => Err(fail(
                Fail::Unavailable,
                "the device panel needs Xcode, which is not installed",
            )),
            Xcode::NotSetUp => Err(fail(
                Fail::Unavailable,
                "Xcode is not set up yet: the user must open Xcode once to finish its setup",
            )),
            Xcode::Ready => Ok(()),
        }
    }

    fn defer(&mut self, call: AgentCall, gate: Gate) {
        // Kept first: the fake answers inside `list`, and the answer opens the gate.
        let list = gate == Gate::List && !self.listing;
        self.agent.deferred.push((call, gate));
        if list {
            self.list_error = None;
            self.list();
        }
        self.drain();
    }

    /// Calls waiting for `gate` start again (or fail with `err`).
    fn open_gate(&mut self, gate: &Gate, err: Option<CtlError>) {
        let (ready, wait): (Vec<_>, Vec<_>) = std::mem::take(&mut self.agent.deferred)
            .into_iter()
            .partition(|(_, g)| g == gate);
        self.agent.deferred = wait;
        for (call, _) in ready {
            match &err {
                Some(e) => send(&call.reply, Err(e.clone())),
                None => self.start_agent(call),
            }
        }
    }

    /// The device the tab's panel shows, booted: else the call is answered with why not.
    fn live_target(&mut self, call: &AgentCall) -> Option<Target> {
        let Some(t) = self.target_of(&call.tab) else {
            send(
                &call.reply,
                Err(fail(
                    Fail::Unavailable,
                    "no device is open in this tab's panel; device_boot boots one and shows it",
                )),
            );
            return None;
        };
        match self.state_of(&t) {
            Some(DeviceState::Booted) => Some(t),
            Some(DeviceState::Booting) => {
                send(
                    &call.reply,
                    Err(fail(
                        Fail::Busy,
                        "the device is still booting; try again in a moment",
                    )),
                );
                None
            }
            Some(_) => {
                send(
                    &call.reply,
                    Err(fail(
                        Fail::Unavailable,
                        "the device in this tab's panel is shut down; device_boot boots it",
                    )),
                );
                None
            }
            None => {
                send(
                    &call.reply,
                    Err(fail(
                        Fail::Unavailable,
                        "the device in this tab's panel is not on this Mac any more",
                    )),
                );
                None
            }
        }
    }

    // ---- boot and shutdown -----------------------------------------------------------------------------

    fn agent_boot(&mut self, call: AgentCall, target: Option<Target>, name: Option<String>) {
        if self.devices.is_none() {
            return self.defer(call, Gate::List);
        }
        let devices = self.devices.clone().unwrap_or_default();
        let found = match (&target, &name) {
            (Some(t), _) => devices.iter().find(|d| &d.target == t),
            (None, Some(n)) => devices
                .iter()
                .find(|d| d.name.eq_ignore_ascii_case(n.trim())),
            (None, None) => self
                .target_of(&call.tab)
                .and_then(|t| devices.iter().find(|d| d.target == t))
                .or_else(|| devices.iter().find(|d| d.kind == "iPhone"))
                .or_else(|| devices.first()),
        };
        let Some(d) = found.cloned() else {
            let what = if devices.is_empty() {
                "Xcode has no iPhone simulator yet; the user adds one in Xcode > Settings > Components".to_string()
            } else {
                "no such device on this Mac; device_list lists them".to_string()
            };
            send(&call.reply, Err(fail(Fail::NotFound, what)));
            return;
        };
        // The device shows in the tab's own panel (rule 1322), whether or not it is booted yet.
        self.agent.effects.push(Effect::SetTarget {
            tab: call.tab.clone(),
            target: d.target.name(),
        });
        if let Some(t) = self.tabs.get_mut(&call.tab) {
            t.choosing = false;
            t.failed = None;
        }
        match d.state {
            DeviceState::Booted => send(
                &call.reply,
                Ok(
                    json!({"target": d.target.name(), "name": d.name, "os": d.os, "booted": true, "already": true}),
                ),
            ),
            DeviceState::ShuttingDown => send(
                &call.reply,
                Err(fail(
                    Fail::Busy,
                    "the device is shutting down; boot it again in a moment",
                )),
            ),
            DeviceState::Booting => self.agent.boots.push((d.target.clone(), call.reply)),
            DeviceState::Shutdown => {
                self.agent.boots.push((d.target.clone(), call.reply));
                self.boot(&d.target);
            }
        }
        self.drain();
    }

    /// The device `t` booted (or failed to): the boot calls waiting for it answer.
    fn boots_done(&mut self, t: &Target, r: Result<(), CtlError>) {
        let info = self.info_of(t).cloned();
        let (done, wait): (Vec<_>, Vec<_>) = std::mem::take(&mut self.agent.boots)
            .into_iter()
            .partition(|(b, _)| b == t);
        self.agent.boots = wait;
        for (_, reply) in done {
            send(
                &reply,
                r.clone().map(|()| {
                    json!({
                        "target": t.name(),
                        "name": info.as_ref().map(|d| d.name.clone()),
                        "os": info.as_ref().map(|d| d.os.clone()),
                        "booted": true,
                        "already": false,
                    })
                }),
            );
        }
    }

    fn agent_shutdown(&mut self, call: AgentCall) {
        let Some(t) = self.target_of(&call.tab) else {
            send(
                &call.reply,
                Err(fail(
                    Fail::Unavailable,
                    "no device is open in this tab's panel",
                )),
            );
            return;
        };
        // Rule 1322: only a device Flare booted (the person's own simulators are theirs).
        if !self.booted.has(t.udid()) {
            send(
                &call.reply,
                Err(fail(
                    Fail::Forbidden,
                    "Flare did not boot this device, so it does not shut it down; the user shuts it down",
                )),
            );
            return;
        }
        if let Some(id) = self.source().map(|s| s.shutdown(&t)) {
            self.pending.insert(
                id,
                Pending::Agent(AgentWait::Shutdown {
                    tab: call.tab,
                    reply: call.reply,
                    target: t,
                }),
            );
        }
        self.drain();
    }

    // ---- look, the tree and input ----------------------------------------------------------------------

    /// A look, a tree read or an input call: the device must be live in the tab's panel and attached (a tab behind
    /// gets its device attached, paused).
    fn agent_on_screen(&mut self, call: AgentCall) {
        let Some(t) = self.live_target(&call) else {
            return;
        };
        let input = matches!(call.req, AgentReq::Act(_));
        if input && let Some(why) = &self.broken {
            send(
                &call.reply,
                Err(fail(
                    Fail::Unavailable,
                    format!("the device shows still pictures only, so it takes no input: {why}"),
                )),
            );
            return;
        }
        let dev = self.devs.entry(t.clone()).or_default();
        if let Some(driver) = dev.driver.as_deref().filter(|d| *d != call.tab) {
            if input {
                let name = self.names.get(driver).map_or("another tab", String::as_str);
                send(
                    &call.reply,
                    Err(fail(
                        Fail::Unavailable,
                        format!(
                            "this device is driven from the tab \"{name}\"; this tab only views it"
                        ),
                    )),
                );
                return;
            }
        } else if dev.driver.is_none() {
            dev.driver = Some(call.tab.clone());
        }
        if !dev.attached {
            if !dev.attaching {
                dev.attaching = true;
                if let Some(id) = self.source().map(|s| s.attach(&t)) {
                    self.pending.insert(id, Pending::Attach(t.clone()));
                }
            }
            return self.defer(call, Gate::Attach(t));
        }
        if let Some(InputState::Broken { why }) = &dev.input
            && input
        {
            let why = why.clone();
            send(
                &call.reply,
                Err(fail(
                    Fail::Unavailable,
                    format!("the device takes no input: {why}"),
                )),
            );
            return;
        }
        match call.req.clone() {
            AgentReq::Look { tree } => self.agent_look(call, t, tree),
            AgentReq::Ui => {
                if let Some(id) = self.source().map(|s| s.ui_tree(&t)) {
                    self.pending.insert(
                        id,
                        Pending::Agent(AgentWait::Ui {
                            tab: call.tab,
                            reply: call.reply,
                            target: t,
                        }),
                    );
                }
                self.drain();
            }
            AgentReq::Act(op) => self.agent_act(call, t, op),
            _ => {}
        }
    }

    fn agent_look(&mut self, call: AgentCall, t: Target, tree: bool) {
        self.agent.next_look += 1;
        let id = self.agent.next_look;
        self.agent.looks.insert(
            id,
            LookCall {
                tab: call.tab,
                reply: call.reply,
                target: t.clone(),
                tree,
                png: None,
                node: None,
                left: if tree { 2 } else { 1 },
            },
        );
        if let Some(r) = self.source().map(|s| s.snapshot(&t)) {
            self.pending
                .insert(r, Pending::Agent(AgentWait::Look(id, true)));
        }
        if tree && let Some(r) = self.source().map(|s| s.ui_tree(&t)) {
            self.pending
                .insert(r, Pending::Agent(AgentWait::Look(id, false)));
        }
        self.drain();
    }

    /// The screen of `t` as the agent sees it: points in the current orientation, and the orientation.
    fn agent_screen(&self, t: &Target) -> (Option<Size>, Orientation) {
        let o = self
            .devs
            .get(t)
            .and_then(|d| d.screen)
            .map(|s| s.orientation)
            .unwrap_or_default();
        (self.screen_pt(t), o)
    }

    fn tree_json(&self, t: &Target, node: &UiNode) -> (Value, RefMap, usize) {
        let screen = self.screen_pt(t).unwrap_or(Size::new(0.0, 0.0));
        let list = filter_tree(node, screen);
        let refs = list.ref_map();
        (
            serde_json::to_value(&list.refs).unwrap_or(Value::Null),
            refs,
            list.left_out,
        )
    }

    fn look_done(&mut self, id: u64) {
        let Some(look) = self.agent.looks.remove(&id) else {
            return;
        };
        let (screen, o) = self.agent_screen(&look.target);
        let info = self.info_of(&look.target).cloned();
        let mut v = json!({
            "device": {
                "target": look.target.name(),
                "name": info.as_ref().map(|d| d.name.clone()),
                "os": info.as_ref().map(|d| d.os.clone()),
            },
            "screen": screen.map(|s| json!({"w": s.w, "h": s.h})),
            "orientation": o,
        });
        match &look.png {
            Some(png) => {
                v["image"] = json!({
                    "mime": "image/png",
                    "data": base64::engine::general_purpose::STANDARD.encode(png),
                    "width": screen.map(|s| s.w),
                    "height": screen.map(|s| s.h),
                });
            }
            None => v["image_error"] = json!("the device's picture could not be taken"),
        }
        if look.tree {
            match &look.node {
                Some(node) => {
                    let (list, refs, left_out) = self.tree_json(&look.target, node);
                    v["tree"] = list;
                    v["left_out"] = json!(left_out);
                    self.agent.refs.insert(look.tab.clone(), refs);
                }
                None => v["tree_error"] = json!("the device's UI tree could not be read"),
            }
        }
        send(&look.reply, Ok(v));
    }

    fn agent_act(&mut self, call: AgentCall, t: Target, op: DeviceOp) {
        if self.agent.jobs.contains_key(&call.tab) {
            send(
                &call.reply,
                Err(fail(
                    Fail::Busy,
                    "another device call is still running in this tab; wait for it",
                )),
            );
            return;
        }
        let Some(screen) = self.screen_pt(&t) else {
            send(
                &call.reply,
                Err(fail(
                    Fail::Unavailable,
                    "the device's screen is not known yet; look again in a moment",
                )),
            );
            return;
        };
        let refs = self.agent.refs.get(&call.tab).cloned().unwrap_or_default();
        let long = matches!(&op, DeviceOp::Tap { hold_ms: Some(h), .. } if *h > TAP_MS);
        let mut job = match Job::start(op, &refs, screen) {
            Ok(j) => j,
            Err(e) => {
                send(&call.reply, Err(ctl_err(e)));
                return;
            }
        };
        if job.types() && self.agent.typed.insert(t.clone()) {
            job.first_key();
        }
        self.agent.took_over.remove(&call.tab);
        let now = self.now();
        let drawn = self.indicators_drawn(&call.tab);
        if drawn {
            if self.agent.ind_tab.as_deref() != Some(call.tab.as_str()) {
                self.agent.ind.hide();
            }
            self.agent.ind_tab = Some(call.tab.clone());
            self.agent.label = call.label.clone();
            self.agent.ind.set_still(!(self.agent.motion)());
            self.agent.ind.set_screen(screen);
            self.agent.ind.job_started();
            if let Some(b) = job.badge() {
                self.agent.ind.badge(&call.label, b, now);
            }
        }
        if let Some((x, y)) = job.take_point() {
            let glide = if drawn {
                self.agent.ind.move_to(x, y, now)
            } else {
                0
            };
            job.glide_for(glide, now);
        }
        self.agent.jobs.insert(
            call.tab.clone(),
            AgentJob {
                job,
                target: t,
                reply: call.reply,
                cancel: call.cancel,
                long,
            },
        );
        self.ensure_agent_tick();
        self.agent_tick();
    }

    /// The indicators of `tab` are drawn: its panel is the front one and the window shows.
    fn indicators_drawn(&self, tab: &str) -> bool {
        self.shown.as_deref() == Some(tab) && !self.minimised && !self.occluded
    }

    // ---- the tick ------------------------------------------------------------------------------------------

    fn ensure_agent_tick(&mut self) {
        if self.agent.ticking {
            return;
        }
        self.agent.ticking = true;
        let post = self.post.clone();
        crate::timer_registry::set(AGENT_TICK, AGENT_TICK_MS, true, move || {
            post(crate::appctl::UiMsg::Device(DeviceMsg::AgentTick));
        });
    }

    /// The jobs move on (cancels, glides, timed input, the settle) and the indicators age. True while there is more.
    pub fn agent_tick(&mut self) -> bool {
        self.drain();
        let now = self.now();
        let tabs: Vec<String> = self.agent.jobs.keys().cloned().collect();
        for tab in tabs {
            let Some(j) = self.agent.jobs.get_mut(&tab) else {
                continue;
            };
            if j.cancel.is_set() {
                self.finish_job(&tab, Err(Job::cancelled()));
                continue;
            }
            let changed = self.devs.get(&j.target).map_or(0, |d| d.changed);
            let out = j.job.on_tick(now, changed);
            self.job_out(&tab, out);
        }
        // The indicators belong to the front panel only: gone at once when it goes behind or the window hides.
        if let Some(t) = self.agent.ind_tab.clone()
            && !self.indicators_drawn(&t)
        {
            self.agent.ind.hide();
            self.agent.ind_tab = None;
            self.agent.held = false;
        }
        let more = !self.agent.jobs.is_empty() || self.agent.ind.needs_tick(now);
        if !more && self.agent.ticking {
            self.agent.ticking = false;
            crate::timer_registry::clear(AGENT_TICK);
        }
        more
    }

    /// Carries out what a job asked.
    fn job_out(&mut self, tab: &str, out: Out) {
        match out {
            Out::Idle => {}
            Out::Done(r) => self.finish_job(tab, r),
            Out::Send(inputs) => {
                let Some((t, long)) = self.agent.jobs.get(tab).map(|j| (j.target.clone(), j.long))
                else {
                    return;
                };
                let now = self.now();
                let drawn = self.agent.ind_tab.as_deref() == Some(tab);
                for i in inputs {
                    match i {
                        Input::Touch(touch) => {
                            if drawn {
                                match touch.phase {
                                    Phase::Start => {
                                        self.agent.ind.press(now);
                                        self.agent.held = long;
                                    }
                                    Phase::Move => self.agent.ind.drag_to(touch.x, touch.y, now),
                                    Phase::End => self.agent.held = false,
                                }
                            }
                            if let Some(s) = self.source() {
                                s.touch(&t, touch);
                            }
                        }
                        Input::Key(k) => {
                            if let Some(s) = self.source() {
                                s.key(&t, k);
                            }
                        }
                        Input::Button(b) => {
                            if let Some(s) = self.source() {
                                s.button(&t, b);
                            }
                        }
                        Input::Paste(text) => {
                            if let Some(id) = self.source().map(|s| s.text(&t, &text)) {
                                self.pending.insert(
                                    id,
                                    Pending::Agent(AgentWait::Paste {
                                        tab: tab.to_string(),
                                    }),
                                );
                            }
                        }
                    }
                }
                self.drain();
            }
        }
    }

    /// The call is over: what it held down is let go of (when it ended early), the answer goes, the indicators hold
    /// a moment and fade.
    fn finish_job(&mut self, tab: &str, result: Result<Value, JobError>) {
        let Some(mut j) = self.agent.jobs.remove(tab) else {
            return;
        };
        if result.is_err() {
            let t = j.target.clone();
            for i in j.job.release() {
                match i {
                    Input::Touch(touch) => {
                        if let Some(s) = self.source() {
                            s.touch(&t, touch);
                        }
                    }
                    Input::Key(k) => {
                        if let Some(s) = self.source() {
                            s.key(&t, k);
                        }
                    }
                    Input::Button(_) | Input::Paste(_) => {}
                }
            }
        }
        if self.agent.ind_tab.as_deref() == Some(tab) {
            self.agent.held = false;
            let now = self.now();
            self.agent.ind.job_ended(now);
        }
        let op = j.job.op();
        send(
            &j.reply,
            result
                .map(|mut v| {
                    v["op"] = json!(op);
                    v
                })
                .map_err(ctl_err),
        );
    }

    /// Every job on `t` ends with `err` (the device shut down, its touch connection dropped, the source broke).
    fn fail_jobs_on(&mut self, t: &Target, err: &JobError) {
        let tabs: Vec<String> = self
            .agent
            .jobs
            .iter()
            .filter(|(_, j)| &j.target == t)
            .map(|(tab, _)| tab.clone())
            .collect();
        for tab in tabs {
            self.finish_job(&tab, Err(err.clone()));
        }
    }

    // ---- what the source said -------------------------------------------------------------------------

    /// An answer to a request an agent call waits on.
    pub(super) fn on_agent_answer(
        &mut self,
        w: AgentWait,
        r: Result<Answer, flare_device::source::SourceError>,
    ) {
        match w {
            AgentWait::Look(id, png) => {
                let Some(look) = self.agent.looks.get_mut(&id) else {
                    return;
                };
                match (png, r) {
                    (true, Ok(Answer::Png(bytes))) => look.png = Some(bytes),
                    (false, Ok(Answer::Tree(node))) => look.node = Some(node),
                    (_, Ok(_)) => {}
                    (_, Err(e)) => log::info!("device look: {}", e.message),
                }
                look.left = look.left.saturating_sub(1);
                if look.left == 0 {
                    self.look_done(id);
                }
            }
            AgentWait::Ui { tab, reply, target } => match r {
                Ok(Answer::Tree(node)) => {
                    let (list, refs, left_out) = self.tree_json(&target, &node);
                    self.agent.refs.insert(tab, refs);
                    let (screen, o) = self.agent_screen(&target);
                    send(
                        &reply,
                        Ok(json!({
                            "screen": screen.map(|s| json!({"w": s.w, "h": s.h})),
                            "orientation": o,
                            "tree": list,
                            "left_out": left_out,
                        })),
                    );
                }
                Ok(_) => send(
                    &reply,
                    Err(CtlError::new(code::INTERNAL, "the source sent no tree")),
                ),
                Err(e) => send(&reply, Err(source_err(e))),
            },
            AgentWait::Done { reply, said } => {
                send(&reply, r.map(|_| said).map_err(source_err));
            }
            AgentWait::Shutdown { tab, reply, target } => match r {
                Ok(_) => {
                    if self.booted.forget(target.udid()) {
                        self.save_booted();
                    }
                    self.agent.effects.push(Effect::Close(tab));
                    send(&reply, Ok(json!({"shut_down": target.name()})));
                }
                Err(e) => send(&reply, Err(source_err(e))),
            },
            AgentWait::Paste { tab } => {
                if let Err(e) = r {
                    self.finish_job(
                        &tab,
                        Err(JobError {
                            fail: e.fail,
                            message: e.message,
                        }),
                    );
                }
            }
        }
    }

    /// Source events the agent's calls follow (called from `on_event`, after the controller's own handling).
    pub(super) fn agent_event(&mut self, ev: &AgentEvent) {
        match ev {
            AgentEvent::Listed(err) => self.open_gate(&Gate::List, err.clone()),
            AgentEvent::Attached(t, err) => self.open_gate(&Gate::Attach(t.clone()), err.clone()),
            AgentEvent::Booted(t, r) => self.boots_done(t, r.clone()),
            AgentEvent::Changed(t, count) => {
                let tabs: Vec<String> = self
                    .agent
                    .jobs
                    .iter()
                    .filter(|(_, j)| &j.target == t)
                    .map(|(tab, _)| tab.clone())
                    .collect();
                for tab in tabs {
                    if let Some(j) = self.agent.jobs.get_mut(&tab) {
                        let out = j.job.on_changed(*count);
                        self.job_out(&tab, out);
                    }
                }
            }
            AgentEvent::NewSession(t) => {
                self.agent.typed.remove(t);
            }
            AgentEvent::InputLost(t) => self.fail_jobs_on(
                t,
                &JobError {
                    fail: Fail::Retry,
                    message: "the device's touch connection dropped; try again".into(),
                },
            ),
            AgentEvent::ShutDown(t) => {
                self.fail_jobs_on(
                    t,
                    &JobError {
                        fail: Fail::Unavailable,
                        message: "the device was shut down".into(),
                    },
                );
                self.boots_done(t, Err(fail(Fail::Unavailable, "the device was shut down")));
            }
            AgentEvent::Broken(why) => {
                let tabs: Vec<String> = self.agent.jobs.keys().cloned().collect();
                for tab in tabs {
                    self.finish_job(
                        &tab,
                        Err(JobError {
                            fail: Fail::Unavailable,
                            message: format!("the live view stopped: {why}"),
                        }),
                    );
                }
            }
        }
    }

    // ---- the person -----------------------------------------------------------------------------------

    /// The person took over the tab's panel: its running input call ends `interrupted`, what it held is let go of
    /// and the indicators go at once (rule 1320). True when a call was running.
    pub(super) fn agent_takeover(&mut self, tab: &str) -> bool {
        if !self.agent.jobs.contains_key(tab) {
            return false;
        }
        self.finish_job(tab, Err(Job::interrupted()));
        self.agent.took_over.insert(tab.to_string());
        if self.agent.ind_tab.as_deref() == Some(tab) {
            self.agent.ind.hide();
            self.agent.held = false;
        }
        true
    }

    /// A tab's panel closed or its device changed: its calls end, its refs go.
    pub(super) fn agent_forget_tab(&mut self, tab: &str) {
        if self.agent.jobs.contains_key(tab) {
            self.finish_job(
                tab,
                Err(JobError {
                    fail: Fail::Unavailable,
                    message: "the device panel was closed".into(),
                }),
            );
        }
        self.agent.refs.remove(tab);
        if self.agent.ind_tab.as_deref() == Some(tab) {
            self.agent.ind.hide();
            self.agent.ind_tab = None;
        }
    }

    /// Animations off, minimised, occluded: the indicators go at once when they are not drawn.
    pub(super) fn agent_hide_indicators(&mut self) {
        self.agent.ind.hide();
        self.agent.ind_tab = None;
        self.agent.held = false;
    }

    // ---- what the agent reads ---------------------------------------------------------------------------

    /// `device_state`: the tab's panel and device, live or still and why, the input connection, frames a second and
    /// whether the person took over the last call.
    pub fn agent_state(&self, tab: &str) -> Value {
        let target = self.target_of(tab);
        let info = target.as_ref().and_then(|t| self.info_of(t));
        let dev = target.as_ref().and_then(|t| self.devs.get(t));
        let (screen, o) = target
            .as_ref()
            .map_or((None, Orientation::Rot0), |t| self.agent_screen(t));
        let fps = match (self.agent.fps.front(), self.agent.fps.back()) {
            (Some(a), Some(b)) if b.0 > a.0 => {
                (b.1.saturating_sub(a.1)) as f64 * 1000.0 / (b.0 - a.0) as f64
            }
            _ => 0.0,
        };
        json!({
            "panel": self.tabs.contains_key(tab),
            "setup": self.setup_of(tab).as_ref().map(Setup::name),
            "target": target.as_ref().map(Target::name),
            "name": info.map(|d| d.name.clone()),
            "os": info.map(|d| d.os.clone()),
            "booted": info.is_some_and(|d| d.state == DeviceState::Booted),
            "booted_by_flare": target.as_ref().is_some_and(|t| self.booted.has(t.udid())),
            "live": dev.is_some_and(|d| d.attached) && self.broken.is_none(),
            "still": self.broken,
            "input": dev.and_then(|d| d.input.clone()),
            "driven_here": dev.is_none_or(|d| d.driver.as_deref().is_none_or(|x| x == tab)),
            "screen": screen.map(|s| json!({"w": s.w, "h": s.h})),
            "orientation": o,
            "fps": (fps * 10.0).round() / 10.0,
            "took_over_last": self.agent.took_over.contains(tab),
            "running": self.agent.jobs.get(tab).map(|j| j.job.op()),
        })
    }

    /// One sample of the frames shown, for `device_state`'s frames a second (each push).
    pub(super) fn sample_fps(&mut self, total: u64) {
        let now = self.now();
        let f = &mut self.agent.fps;
        if f.back().is_some_and(|(t, _)| now.saturating_sub(*t) < 100) {
            return;
        }
        f.push_back((now, total));
        while f
            .front()
            .is_some_and(|(t, _)| now.saturating_sub(*t) > 2_000)
        {
            f.pop_front();
        }
    }

    /// The indicators the window draws now (tests and the probe).
    pub fn indicator_frame(&mut self) -> Option<IndicatorFrame> {
        let tab = self.agent.ind_tab.clone()?;
        if !self.indicators_drawn(&tab) {
            return None;
        }
        let now = self.now();
        Some(self.agent.ind.frame(now))
    }

    /// The indicators into the window's `panel-device-*` properties (only what changed).
    pub fn push_indicators(&mut self, app: &AppWindow) {
        let frame = self.indicator_frame().unwrap_or_default();
        if self.agent.pushed.as_ref() == Some(&frame) {
            return;
        }
        match &frame.dot {
            Some(d) => {
                app.set_panel_device_dot(true);
                app.set_panel_device_dot_x(d.at.x as f32);
                app.set_panel_device_dot_y(d.at.y as f32);
                app.set_panel_device_dot_alpha(d.alpha);
                app.set_panel_device_ripples(ModelRc::from(std::rc::Rc::new(VecModel::from(
                    d.ripples.clone(),
                ))));
            }
            None => {
                app.set_panel_device_dot(false);
                app.set_panel_device_ripples(ModelRc::default());
            }
        }
        app.set_panel_device_dot_held(self.agent.held && frame.dot.is_some());
        app.set_panel_device_tag(
            if frame.dot.is_some() {
                crate::web_ctl::cursor_tag(&self.agent.label)
            } else {
                String::new()
            }
            .into(),
        );
        let trail: Vec<DeviceTrailPoint> = frame
            .trail
            .iter()
            .map(|&(x, y, a)| DeviceTrailPoint {
                x: x as f32,
                y: y as f32,
                a,
            })
            .collect();
        app.set_panel_device_trail(ModelRc::from(std::rc::Rc::new(VecModel::from(trail))));
        app.set_panel_device_badge(
            frame
                .badge
                .as_ref()
                .map(|b| format!("{} {}", crate::web_ctl::cursor_tag(&b.agent), b.text))
                .unwrap_or_default()
                .into(),
        );
        app.set_panel_device_glow(frame.glow.unwrap_or(0.0));
        self.agent.pushed = Some(frame);
    }

    /// The agent's calls and indicators for the `device.state` probe.
    pub(super) fn agent_probe(&self) -> Value {
        json!({
            "jobs": self.agent.jobs.iter().map(|(t, j)| json!({"tab": t, "op": j.job.op()})).collect::<Vec<_>>(),
            "indicators": self.agent.ind_tab,
            "glow": self.agent.pushed.as_ref().and_then(|f| f.glow),
            "dot": self.agent.pushed.as_ref().and_then(|f| f.dot.as_ref()).map(|d| json!({"x": d.at.x, "y": d.at.y, "alpha": d.alpha, "ripples": d.ripples.len()})),
            "badge": self.agent.pushed.as_ref().and_then(|f| f.badge.as_ref()).map(|b| b.text.clone()),
            "trail": self.agent.pushed.as_ref().map_or(0, |f| f.trail.len()),
        })
    }
}

/// What the controller's event handling tells the agent's half.
#[derive(Clone, Debug)]
pub(super) enum AgentEvent {
    Listed(Option<CtlError>),
    Attached(Target, Option<CtlError>),
    Booted(Target, Result<(), CtlError>),
    Changed(Target, u64),
    NewSession(Target),
    InputLost(Target),
    ShutDown(Target),
    Broken(String),
}
