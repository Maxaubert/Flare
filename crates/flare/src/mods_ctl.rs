//! Mods inside the window (Surfaces stage 8): the state the UI thread keeps about the pages mods are showing,
//! and what happens to a message a mod's page sends.
//!
//! The rules live elsewhere and are tested there: the manifest, the token and the tool table in `flare_mods`,
//! the grant in `flare_ctl`, the page's address and walls in `flare_surfaces::modview` and `flare_web`.
//! This file is the part that needs the window's thread:
//!
//! - **The bridge.** `window.flare.call(method, params)` arrives as a web message. It is checked against the
//!   mod's grant HERE (the page never holds a token: the host looks the mod's live grant up, so a mod that was
//!   turned off is refused at once), then run through the same dispatcher a pipe connection uses, on a worker,
//!   with the tab the mod's panel sits in. `window.flare.respond` answers a tool call through the hub.
//! - **Which tab.** A mod acts on ONE tab: the tab whose panel shows its page. A plugin that holds `tabs` may name
//!   any tab in a call (the dispatcher decides, `Grant::may_act_on_any_tab`); its background page has no tab of
//!   its own, so it names one or acts on none.
//! - **Background pages** (plugins phase 3). A protocol 2 plugin that is on and declares `background` has one
//!   hidden page, tied to no tab, for as long as it is on. This controller keeps which ones run, the restarts of
//!   each (3 in 5 minutes, then it is stopped and Settings says so) and the ping that tells a hung page from a
//!   quiet one; the window opens and closes the views.
//! - **Events.** A page says which events it listens to (`window.flare.on`); the window hands each event to every
//!   page of the plugin that listens and may see it ([`ModCtl::listeners`]).
//! - **The budget.** A plugin's pages share one limiter (`flare_mods::limit`): each message is spent here, as it
//!   is handed over, and one over the budget is dropped unread.
//! - **Which page.** Pages are kept by slot (`flare_surfaces::slot`): a tab's panel today, and later a bar
//!   beside a tab or a plugin's background page. A method that is given a bare tab id means that tab's panel.
//!   A call from a background page has no tab of its own.
//! - **The switches.** Turning a mod on or off is applied in memory at once (a token minted or killed) and saved
//!   on a worker.
//!
//! Nothing here blocks the UI thread: scans, saves and calls run on workers and come back as `UiMsg::Mods`.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flare_ctl::{CallContext, Dispatcher, Method, Namespace, code};
use flare_mods::bridge::{self, PageMsg};
use flare_mods::hub::{Change, Hub};
use flare_settings::page::mods::{ModInfo, ModsInfo, Problem};
use flare_surfaces::slot::Slot;
use serde_json::Value;

use crate::appctl::{Post, UiMsg};

/// Calls from one mod's page running at once.
pub const MAX_PAGE_CALLS: usize = 16;
/// How often a background page is pinged, and how long it has to answer before it counts as dead (design 13).
pub const PING_EVERY: Duration = Duration::from_secs(10);
/// A background page is started again at most this many times in [`RESTART_WINDOW`]; the next death stops it.
pub const MAX_RESTARTS: usize = 3;
pub const RESTART_WINDOW: Duration = Duration::from_secs(5 * 60);
/// The sentence Settings shows for a background page that ended too often.
pub const STOPPED_CRASHED: &str = "Stopped: its background page ended again after 3 restarts in 5 minutes. Turn it off and on to start it again.";
/// The sentence for a plugin past the cap of background pages (only a switch file edited by hand gets there).
pub const STOPPED_CAP: &str = "Not started: at most 6 plugins with a background page run at once. Turn another one off first.";

/// What a worker or the Settings page says to the mods controller (carried by `UiMsg::Mods`).
pub enum ModsMsg {
    /// A scan finished (a worker): what changed.
    Scanned(Change),
    /// A switch was written to `mods.json` (or could not be).
    Persisted {
        id: String,
        result: Result<(), String>,
    },
    /// A call the mod's page made has its answer: `json` is the message for the page.
    Reply {
        /// The page that asked.
        slot: Slot,
        mod_id: String,
        /// The document the call came from (`ModCtl::generation`): a reply for an older one is dropped,
        /// because the bridge's call ids restart in every document.
        generation: u64,
        json: String,
    },
    /// Settings: scan again (the Mods page opened).
    Rescan,
    /// Settings: the switch of a mod.
    SetEnabled {
        id: String,
        on: bool,
        /// What the Settings card listed; None for a switch that did not come from a row (the tests).
        shown: Option<flare_mods::hub::Shown>,
    },
    /// Settings: show the mod's panel in the most recent shell tab.
    ShowPanel { id: String },
    /// The ping timer (every [`PING_EVERY`] while a background page runs).
    Tick,
    /// A tab's follower read new entries of its Claude session (plugins phase 5): for the pages that listen to
    /// `session` and may see the tab.
    Said {
        tab: String,
        session: String,
        said: flare_convo::follow::Said,
    },
    /// The Enter a plugin's `tabs.input` asked for, due now (it follows the paste after a short wait).
    Enter {
        plugin: String,
        tab: String,
        /// The plugin's generation when the call was checked: a plugin switched off since presses nothing.
        generation: u64,
    },
    /// Settings: install a plugin from a `.zip` file (the picker opens).
    InstallFile,
    /// Settings: install a plugin from an `https` address.
    InstallUrl { url: String },
    /// Settings: update the plugin (from its address, or a file).
    Update { id: String },
    /// Settings: remove the plugin.
    Remove { id: String },
    /// The file picker answered (None: cancelled). `expect` is the id an update must hold.
    Picked {
        path: Option<std::path::PathBuf>,
        expect: Option<String>,
    },
    /// A worker unpacked and checked a zip (or could not, with the sentence).
    Staged(Result<Box<flare_mods::hub::StagedInstall>, String>),
    /// A worker put the plugin in place and scanned (or could not).
    Installed(Result<flare_mods::hub::Installed, String>),
    /// A worker removed the plugin and scanned (or could not).
    Removed {
        id: String,
        result: Result<Change, String>,
    },
    /// A plugin's helper program started, ended and was started again, or stopped (its own threads).
    Helper(flare_mods::helper::Note),
    /// The time of a plugin's question ran out (the `fill` permission): it counts as Deny, unless it was answered.
    AskExpired { seq: u64 },
}

/// What Settings shows of the installs: one runs, and what the last one said.
#[derive(Clone, Debug, Default)]
pub struct InstallState {
    pub installing: bool,
    /// (failed, the sentence).
    pub note: Option<(bool, String)>,
}

/// Where the channel's events for one plugin's helper go: (event, params, plugin id). Answers how many of the
/// plugin's connections it was queued for (0: its helper is not connected).
pub type OwnerEvents = Arc<dyn Fn(&str, Value, &str) -> usize + Send + Sync>;
/// The file picker for a plugin zip; its answer comes on a worker.
pub type PickZip = std::rc::Rc<dyn Fn(crate::settings_ctl::PickAnswer)>;

/// A `ModsMsg` that can sit in a `UiMsg` (which is `Debug`): it prints its kind only, never a call's data.
pub struct ModsMsgBox(pub ModsMsg);

impl std::fmt::Debug for ModsMsgBox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self.0 {
            ModsMsg::Scanned(_) => "ModsMsg::Scanned",
            ModsMsg::Persisted { .. } => "ModsMsg::Persisted",
            ModsMsg::Reply { .. } => "ModsMsg::Reply",
            ModsMsg::Rescan => "ModsMsg::Rescan",
            ModsMsg::SetEnabled { .. } => "ModsMsg::SetEnabled",
            ModsMsg::ShowPanel { .. } => "ModsMsg::ShowPanel",
            ModsMsg::Tick => "ModsMsg::Tick",
            ModsMsg::Said { .. } => "ModsMsg::Said",
            ModsMsg::Enter { .. } => "ModsMsg::Enter",
            ModsMsg::InstallFile => "ModsMsg::InstallFile",
            ModsMsg::InstallUrl { .. } => "ModsMsg::InstallUrl",
            ModsMsg::Update { .. } => "ModsMsg::Update",
            ModsMsg::Remove { .. } => "ModsMsg::Remove",
            ModsMsg::Picked { .. } => "ModsMsg::Picked",
            ModsMsg::Staged(_) => "ModsMsg::Staged",
            ModsMsg::Installed(_) => "ModsMsg::Installed",
            ModsMsg::Removed { .. } => "ModsMsg::Removed",
            ModsMsg::Helper(_) => "ModsMsg::Helper",
            ModsMsg::AskExpired { .. } => "ModsMsg::AskExpired",
        })
    }
}

/// Messages kept for a page that is not ready (the oldest are dropped past this).
const QUEUE_CAP: usize = 64;

/// A mod's page in a slot.
#[derive(Debug, Default)]
struct ModPage {
    mod_id: String,
    /// The page has loaded and can be spoken to.
    ready: bool,
    /// Messages for the page that came before it was ready, in order.
    queue: Vec<String>,
    /// Which document this is: it changes whenever the page begins to load, so a reply meant for an older
    /// document is never given to a newer one.
    generation: u64,
    /// The events the document listens to (`window.flare.on`), besides `tool`.
    listens: BTreeSet<String>,
}

/// A background page that runs.
#[derive(Debug, Default)]
struct BgRun {
    /// The ping sent and not answered yet: its id and when.
    ping: Option<(u64, Instant)>,
    /// Since when the page is loading (started, or began to load again), None once it is ready. A page that stays
    /// loading past [`PING_EVERY`] is never pinged, so it counts as silent.
    loading: Option<Instant>,
}

/// What the ping timer found: the pages to ping now (with the message), and the ones that did not answer the last
/// ping in time.
#[derive(Debug, Default, PartialEq)]
pub struct PingRound {
    pub ping: Vec<(String, String)>,
    pub silent: Vec<String>,
}

/// The mods controller (UI thread).
pub struct ModCtl {
    hub: Arc<Hub>,
    post: Post,
    dispatcher: Option<Arc<dyn Dispatcher>>,
    pages: HashMap<Slot, ModPage>,
    /// Source of `ModPage::generation`.
    next_generation: u64,
    /// Calls a page made that a worker is still running, by mod.
    inflight: HashMap<String, usize>,
    /// A scan is on a worker; another was asked for meanwhile.
    scanning: Arc<AtomicBool>,
    rescan: bool,
    /// The last switch that could not be saved, for the Settings page.
    pub save_error: Arc<Mutex<Option<String>>>,
    /// Scans finished (the tests and the probe read it).
    pub scans: u64,
    /// The background pages that run, by plugin.
    bg: HashMap<String, BgRun>,
    /// When each plugin's background page was started again after it ended (the restart policy), by plugin. Kept
    /// while the page is stopped; forgotten when the plugin is switched.
    restarts: HashMap<String, Vec<Instant>>,
    /// Source of ping ids.
    ping_seq: u64,
    /// Messages dropped because a plugin was over its budget (the tests and the probe read it).
    pub dropped: u64,
    /// Safe mode's record of this run (`plugin_safe`), told once a plugin is on; None writes nothing (tests, e2e).
    run_record: Option<std::path::PathBuf>,
    /// The record was told already.
    record_told: bool,
    /// Installs, updates and removals: one at a time, and what the last one said (Settings reads it).
    pub install: Arc<Mutex<InstallState>>,
    /// The plugins' helper programs (plugins phase 7); None without the control channel (a helper needs its pipe).
    pub helpers: Option<flare_mods::helper::Host>,
    /// Where events for a plugin's helper go (the channel); None without it.
    pub owner_events: Option<OwnerEvents>,
    /// The file picker of Install from file; None in a run without a window (the tests answer through it too).
    pub pick_zip: Option<PickZip>,
}

impl ModCtl {
    pub fn new(hub: Arc<Hub>, post: Post) -> Self {
        Self {
            hub,
            post,
            dispatcher: None,
            pages: HashMap::new(),
            next_generation: 0,
            inflight: HashMap::new(),
            scanning: Arc::new(AtomicBool::new(false)),
            rescan: false,
            save_error: Arc::new(Mutex::new(None)),
            scans: 0,
            bg: HashMap::new(),
            restarts: HashMap::new(),
            ping_seq: 0,
            dropped: 0,
            run_record: None,
            record_told: false,
            install: Arc::new(Mutex::new(InstallState::default())),
            helpers: None,
            owner_events: None,
            pick_zip: None,
        }
    }

    /// Where this run's record for safe mode is (`plugin_safe`).
    pub fn set_run_record(&mut self, file: Option<std::path::PathBuf>) {
        self.run_record = file;
    }

    /// A plugin is on in this run: safe mode's record is told so, once, on a worker.
    pub fn note_plugins_on(&mut self) {
        if self.record_told || self.hub.snapshot().mods.iter().all(|m| !m.enabled) {
            return;
        }
        self.record_told = true;
        let Some(file) = self.run_record.clone() else {
            return;
        };
        if let Err(e) = std::thread::Builder::new()
            .name("flare-plugins-record".into())
            .spawn(move || crate::plugin_safe::plugins_on(&file))
        {
            log::debug!("the plugins' run record was not told: {e}");
        }
    }

    pub fn hub(&self) -> &Arc<Hub> {
        &self.hub
    }

    /// Where a page's calls run (the control channel's dispatcher). Without one every call of a page answers
    /// `unavailable`.
    pub fn set_dispatcher(&mut self, d: Arc<dyn Dispatcher>) {
        self.dispatcher = Some(d);
    }

    /// Reads the plugins folder on a worker (the first scan moves the old mods folder in). One scan at a time; one
    /// asked for meanwhile runs after it.
    pub fn start_scan(&mut self) {
        if self.scanning.swap(true, Ordering::SeqCst) {
            self.rescan = true;
            return;
        }
        let (hub, post, flag) = (
            Arc::clone(&self.hub),
            Arc::clone(&self.post),
            Arc::clone(&self.scanning),
        );
        let job = move || {
            // The plugins folder is made here (a worker, at start and each time the page opens) so the Open plugins
            // folder button has somewhere to go. The old mods folder is never made.
            let _ = std::fs::create_dir_all(&hub.config().dir); // sync-ok: the scan worker
            let change = hub.scan();
            flag.store(false, Ordering::SeqCst);
            post(UiMsg::Mods(ModsMsgBox(ModsMsg::Scanned(change))));
        };
        if let Err(e) = std::thread::Builder::new()
            .name("flare-mods-scan".into())
            .spawn(job)
        {
            self.scanning.store(false, Ordering::SeqCst);
            log::warn!("could not start the mods scan: {e}");
        }
    }

    /// A scan has finished. True when another was asked for meanwhile and has been started.
    pub fn scan_done(&mut self) -> bool {
        self.scans += 1;
        if std::mem::take(&mut self.rescan) {
            self.start_scan();
            return true;
        }
        false
    }

    /// Writes a switch on a worker (the file gets the mod's state as it is when the write runs).
    pub fn persist(&self, id: &str) {
        let (hub, post) = (Arc::clone(&self.hub), Arc::clone(&self.post));
        let id = id.to_string();
        let job = {
            let id = id.clone();
            move || {
                let result = hub.persist(&id);
                post(UiMsg::Mods(ModsMsgBox(ModsMsg::Persisted { id, result })));
            }
        };
        if let Err(e) = std::thread::Builder::new()
            .name("flare-mods-save".into())
            .spawn(job)
        {
            log::warn!("could not start the mods save: {e}");
        }
    }

    // ---- the pages ------------------------------------------------------------------------------------------------

    /// The slot (a bare tab: its panel) shows this mod's page now. A page the slot already had for the same mod
    /// keeps its state.
    pub fn bind(&mut self, slot: impl Into<Slot>, mod_id: &str) {
        let slot = slot.into();
        let same = self.pages.get(&slot).is_some_and(|t| t.mod_id == mod_id);
        if !same {
            self.next_generation += 1;
            self.pages.insert(
                slot,
                ModPage {
                    mod_id: mod_id.to_string(),
                    generation: self.next_generation,
                    ..ModPage::default()
                },
            );
        }
    }

    /// The slot no longer shows a mod's page. Gives the mod it was.
    pub fn unbind(&mut self, slot: impl Into<Slot>) -> Option<String> {
        self.pages.remove(&slot.into()).map(|t| t.mod_id)
    }

    pub fn bound(&self) -> Vec<(Slot, String)> {
        let mut v: Vec<(Slot, String)> = self
            .pages
            .iter()
            .map(|(s, m)| (s.clone(), m.mod_id.clone()))
            .collect();
        v.sort();
        v
    }

    pub fn mod_of(&self, slot: impl Into<Slot>) -> Option<&str> {
        self.pages.get(&slot.into()).map(|t| t.mod_id.as_str())
    }

    /// The document the slot's page is now (see `ModPage::generation`).
    pub fn generation(&self, slot: impl Into<Slot>) -> Option<u64> {
        self.pages.get(&slot.into()).map(|t| t.generation)
    }

    /// The page lost its process or failed to load: it can be spoken to no more, but the slot still shows the
    /// mod, so the next call makes a new page. Messages waiting for it are dropped (the caller fails their
    /// calls).
    pub fn page_lost(&mut self, slot: impl Into<Slot>) {
        self.next_generation += 1;
        let generation = self.next_generation;
        if let Some(t) = self.pages.get_mut(&slot.into()) {
            t.ready = false;
            t.queue.clear();
            t.generation = generation;
            t.listens.clear();
        }
    }

    pub fn is_ready(&self, slot: impl Into<Slot>) -> bool {
        self.pages.get(&slot.into()).is_some_and(|t| t.ready)
    }

    /// The page loaded (`true`) or began to load (`false`). Going ready gives back what was queued for it.
    pub fn set_ready(&mut self, slot: impl Into<Slot>, ready: bool) -> Vec<String> {
        if !ready {
            self.next_generation += 1;
        }
        let generation = self.next_generation;
        let slot = slot.into();
        if slot.tab().is_none()
            && let Some(run) = self.bg.get_mut(&slot.owner)
        {
            run.loading = if ready {
                None
            } else {
                run.loading.or_else(|| Some(Instant::now()))
            };
        }
        match self.pages.get_mut(&slot) {
            Some(t) => {
                t.ready = ready;
                if !ready {
                    t.generation = generation;
                    // A new document registers its own handlers.
                    t.listens.clear();
                }
                if ready {
                    // A tool call whose caller gave up while the page was not ready (timed out, cancelled, failed)
                    // is not run now: the agent was told it failed, and it would run twice.
                    let hub = &self.hub;
                    std::mem::take(&mut t.queue)
                        .into_iter()
                        .filter(|m| {
                            serde_json::from_str::<serde_json::Value>(m)
                                .ok()
                                .and_then(|v| {
                                    v.get("callId").and_then(|c| c.as_str().map(str::to_string))
                                })
                                .is_none_or(|id| hub.is_pending(&id))
                        })
                        .collect()
                } else {
                    Vec::new()
                }
            }
            None => Vec::new(),
        }
    }

    /// A message for the page: Some when it can be sent now, None when it was queued until the page is ready (or
    /// there is no such page).
    pub fn queue_or_send(&mut self, slot: impl Into<Slot>, msg: String) -> Option<String> {
        let t = self.pages.get_mut(&slot.into())?;
        if t.ready {
            Some(msg)
        } else {
            // Nobody reads it while the page is not ready: the oldest go first past the cap.
            if t.queue.len() >= QUEUE_CAP {
                t.queue.remove(0);
            }
            t.queue.push(msg);
            None
        }
    }

    /// The slots showing the mod's page.
    pub fn slots_of(&self, mod_id: &str) -> Vec<Slot> {
        let mut v: Vec<Slot> = self
            .pages
            .iter()
            .filter(|(_, t)| t.mod_id == mod_id)
            .map(|(slot, _)| slot.clone())
            .collect();
        v.sort();
        v
    }

    /// A call a page made has been answered.
    pub fn call_done(&mut self, mod_id: &str) {
        if let Some(n) = self.inflight.get_mut(mod_id) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.inflight.remove(mod_id);
            }
        }
    }

    pub fn inflight(&self, mod_id: &str) -> usize {
        self.inflight.get(mod_id).copied().unwrap_or(0)
    }

    /// The pages that listen to the event `name` (`tool` is never one: tool calls go to the page that answers),
    /// with their plugin, in slot order.
    pub fn listeners(&self, name: &str) -> Vec<(Slot, String)> {
        let mut v: Vec<(Slot, String)> = self
            .pages
            .iter()
            .filter(|(_, p)| p.listens.contains(name))
            .map(|(s, p)| (s.clone(), p.mod_id.clone()))
            .collect();
        v.sort();
        v
    }

    /// Whether any page listens to one of `names`.
    pub fn any_listens(&self, names: &[&str]) -> bool {
        self.pages
            .values()
            .any(|p| names.iter().any(|n| p.listens.contains(*n)))
    }

    // ---- background pages ---------------------------------------------------------------------------------------

    /// The plugins whose background page runs, sorted.
    pub fn backgrounds(&self) -> Vec<String> {
        let mut v: Vec<String> = self.bg.keys().cloned().collect();
        v.sort();
        v
    }

    pub fn bg_running(&self, id: &str) -> bool {
        self.bg.contains_key(id)
    }

    /// The window opened the plugin's background page: its slot shows the plugin from now on.
    pub fn bg_started(&mut self, id: &str) {
        self.bg.insert(
            id.to_string(),
            BgRun {
                ping: None,
                loading: Some(Instant::now()),
            },
        );
        self.bind(Slot::background(id), id);
    }

    /// The plugin's background page is closed (turned off, stopped, or ended): its slot shows nothing.
    pub fn bg_ended(&mut self, id: &str) {
        self.bg.remove(id);
        self.unbind(Slot::background(id));
    }

    /// The plugin was switched: its restarts are forgotten, so a page that was stopped may run again.
    pub fn bg_forget(&mut self, id: &str) {
        self.restarts.remove(id);
    }

    /// The plugin's background page ended on its own (its process, a failed load, no answer to a ping): true when
    /// it may be started again, false when it ended too often ([`MAX_RESTARTS`] restarts in [`RESTART_WINDOW`]) and
    /// is to stay stopped. The page is let go either way.
    pub fn bg_crashed(&mut self, id: &str, now: Instant) -> bool {
        self.bg_ended(id);
        let times = self.restarts.entry(id.to_string()).or_default();
        times.retain(|t| now.saturating_duration_since(*t) < RESTART_WINDOW);
        if times.len() >= MAX_RESTARTS {
            return false;
        }
        times.push(now);
        true
    }

    /// The ping timer: every running background page that is ready is pinged, unless the ping sent before is still
    /// unanswered after [`PING_EVERY`] (then the page counts as dead and is in `silent`). A page still loading
    /// [`PING_EVERY`] after it began is silent too (a script that never lets it finish).
    pub fn ping_round(&mut self, now: Instant) -> PingRound {
        let mut round = PingRound::default();
        for id in self.backgrounds() {
            let ready = self.is_ready(Slot::background(&id));
            let Some(run) = self.bg.get_mut(&id) else {
                continue;
            };
            match run.ping {
                Some((_, sent)) if now.saturating_duration_since(sent) >= PING_EVERY => {
                    round.silent.push(id);
                }
                Some(_) => {}
                None if ready => {
                    self.ping_seq += 1;
                    run.ping = Some((self.ping_seq, now));
                    round.ping.push((id, bridge::ping_message(self.ping_seq)));
                }
                None => {
                    if run
                        .loading
                        .is_some_and(|since| now.saturating_duration_since(since) >= PING_EVERY)
                    {
                        round.silent.push(id);
                    }
                }
            }
        }
        round
    }

    /// Whether a ping is waiting for the plugin's background page to answer (tests).
    #[cfg(test)]
    pub fn ping_waiting(&self, id: &str) -> bool {
        self.bg.get(id).is_some_and(|r| r.ping.is_some())
    }

    /// A message the page of `slot` (a bare tab: its panel) posted. Returns the message to post back to the
    /// page at once (an answer, or the reason for a refusal); a call that is allowed runs on a worker and
    /// answers later. A message from a slot that shows no mod's page is ignored.
    pub fn on_page_message(&mut self, slot: impl Into<Slot>, text: &str) -> Option<String> {
        let slot = slot.into();
        let mod_id = self.pages.get(&slot)?.mod_id.clone();
        // A plugin's pages spend one budget; a message over it is dropped unread (protocol 1 mods have none).
        if let Some(limiter) = self.hub.limits().of(&mod_id)
            && let Err(over) = limiter.admit(text.len(), Instant::now())
        {
            self.dropped += 1;
            log::debug!("a message of plugin {mod_id} was dropped: {}", over.why());
            return None;
        }
        let msg = match bridge::parse_page_message(text) {
            Ok(m) => m,
            Err(why) => {
                log::debug!("a mod's page sent something that is not a message: {why}");
                return None;
            }
        };
        match msg {
            PageMsg::Respond { call_id, outcome } => {
                // Answering needs the `tools` permission, like `tools.respond` over the pipe.
                let allowed = self
                    .hub
                    .grant_of_mod(&mod_id)
                    .is_some_and(|g| g.check(Method::ToolsRespond).is_ok());
                if allowed {
                    // A call that is gone (answered, timed out, another mod's) is not the page's problem.
                    if let Err(e) = self.hub.respond(&mod_id, &call_id, outcome) {
                        log::debug!("a mod answered a call that is not open: {}", e.message);
                    }
                }
                None
            }
            PageMsg::Call { id, method, params } => {
                self.page_call(&slot, &mod_id, id, &method, params)
            }
            PageMsg::Listen { name } => {
                let page = self.pages.get_mut(&slot)?;
                page.listens.insert(name.clone());
                // A page that listens to `plugin` hears at once that the plugin is on: it is, or the page would
                // not exist.
                (name == "plugin")
                    .then(|| bridge::event_message("plugin", &serde_json::json!({"kind": "on"})))
            }
            PageMsg::Pong { id } => {
                if let Some(run) = slot
                    .tab()
                    .is_none()
                    .then(|| self.bg.get_mut(&slot.owner))
                    .flatten()
                    && run.ping.is_some_and(|(p, _)| p == id)
                {
                    run.ping = None;
                }
                None
            }
        }
    }

    fn page_call(
        &mut self,
        slot: &Slot,
        mod_id: &str,
        id: u64,
        method: &str,
        params: Value,
    ) -> Option<String> {
        let refuse = |c: &str, m: &str| Some(bridge::reply_err(id, c, m));
        // The grant is the mod's live one: a mod that was turned off has none.
        let Some(grant) = self.hub.grant_of_mod(mod_id) else {
            return refuse(code::UNAUTHORIZED, "this mod is turned off");
        };
        let Some(m) = Method::parse(method) else {
            return refuse(code::UNKNOWN_METHOD, "that method is not in the protocol");
        };
        match m {
            Method::Hello => {
                return refuse(
                    code::BAD_PARAMS,
                    "hello is not needed: the page is already known",
                );
            }
            Method::ToolsRespond => {
                return refuse(
                    code::BAD_PARAMS,
                    "use window.flare.respond to answer a tool call",
                );
            }
            _ => {}
        }
        if let Err(e) = grant.check(m) {
            return refuse(&e.code, &e.message);
        }
        // A mod's own browser panel needs a second slot beside its page (recorded in decisions.md): until it
        // is built, a mod that holds `browser` is allowed the methods but they answer `unavailable`.
        if m.namespace() == Namespace::Browser {
            return refuse(
                code::UNAVAILABLE,
                "a mod's own browser panel is not built yet",
            );
        }
        let Some(d) = self.dispatcher.clone() else {
            return refuse(code::UNAVAILABLE, "the control channel is not running");
        };
        if self.inflight(mod_id) >= MAX_PAGE_CALLS {
            return refuse(code::RATE_LIMITED, "too many calls at once");
        }
        *self.inflight.entry(mod_id.to_string()).or_insert(0) += 1;
        let (post, page, mod_name) = (Arc::clone(&self.post), slot.clone(), mod_id.to_string());
        let generation = self.generation(slot).unwrap_or_default();
        let job = move || {
            let ctx = CallContext {
                conn: 0,
                client: format!("mod:{mod_name}"),
                client_version: String::new(),
                grant,
                // The tab the page sits in; a background page has none.
                tab: page.tab().map(str::to_string),
            };
            let json = match d.call(&ctx, m, params) {
                Ok(v) => bridge::reply_ok(id, &v),
                Err(e) => bridge::reply_err(id, &e.code, &e.message),
            };
            post(UiMsg::Mods(ModsMsgBox(ModsMsg::Reply {
                slot: page,
                mod_id: mod_name,
                generation,
                json,
            })));
        };
        if std::thread::Builder::new()
            .name("flare-mod-call".into())
            .spawn(job)
            .is_err()
        {
            self.call_done(mod_id);
            return refuse(code::UNAVAILABLE, "the call could not start");
        }
        None
    }

    /// Fails everything waiting on a mod: its calls from the agent get `unavailable` with `why`.
    pub fn fail_calls(&self, mod_id: &str, why: &str) {
        self.hub.fail_calls(mod_id, why);
    }
}

/// What the Plugins page shows, from the hub. A protocol 1 mod's rows are filled exactly as before; a protocol 2
/// plugin adds who made it and the warnings for its permissions.
pub fn info_of(
    hub: &Hub,
    save_error: &Mutex<Option<String>>,
    install: &Mutex<InstallState>,
) -> ModsInfo {
    let s = hub.snapshot();
    let install = install.lock().unwrap_or_else(|e| e.into_inner()).clone();
    ModsInfo {
        scanned: s.scanned,
        dir: hub.config().dir.to_string_lossy().into_owned(),
        mods: s
            .mods
            .into_iter()
            .map(|m| {
                let plugin = m.protocol == flare_mods::PROTOCOL_V2;
                ModInfo {
                    has_panel: m.panel_title.is_some(),
                    permissions: m
                        .permissions
                        .iter()
                        .map(|p| flare_mods::hub::permission_words(*p).to_string())
                        .collect(),
                    warnings: if plugin {
                        flare_mods::hub::permission_warnings(&m.permissions)
                            .into_iter()
                            .map(str::to_string)
                            .collect()
                    } else {
                        Vec::new()
                    },
                    full_control: plugin && flare_mods::hub::is_full_control(&m.permissions),
                    plugin,
                    author: m.author.unwrap_or_default(),
                    homepage: m.homepage.unwrap_or_default(),
                    id: m.id,
                    name: m.name,
                    version: m.version,
                    description: m.description,
                    enabled: m.enabled,
                    needs_approval: m.needs_approval,
                    stopped: m.stopped,
                    source: m.source.unwrap_or_default(),
                }
            })
            .collect(),
        problems: s
            .errors
            .into_iter()
            .map(|e| Problem {
                folder: e.folder,
                reason: e.reason,
            })
            .collect(),
        notes: s.notes,
        safe_mode: s.safe_mode,
        save_error: save_error.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        installing: install.installing,
        install_note: install.note,
        url: String::new(),
    }
}

/// Settings' door to the mods (the Mods page): what to show and what the person did. Every action goes to the
/// window as a message, so the same code runs whether it was a click or a probe.
pub struct ModsSettings {
    pub hub: Arc<Hub>,
    pub post: Post,
    pub save_error: Arc<Mutex<Option<String>>>,
    pub install: Arc<Mutex<InstallState>>,
}

impl crate::settings_ctl::ModsPort for ModsSettings {
    fn info(&self) -> ModsInfo {
        info_of(&self.hub, &self.save_error, &self.install)
    }

    fn page_shown(&self) {
        (self.post)(UiMsg::Mods(ModsMsgBox(ModsMsg::Rescan)));
    }

    fn set_enabled(&self, id: &str, on: bool, shown: flare_mods::hub::Shown) {
        (self.post)(UiMsg::Mods(ModsMsgBox(ModsMsg::SetEnabled {
            id: id.to_string(),
            on,
            shown: Some(shown),
        })));
    }

    fn show_panel(&self, id: &str) {
        (self.post)(UiMsg::Mods(ModsMsgBox(ModsMsg::ShowPanel {
            id: id.to_string(),
        })));
    }

    fn dir(&self) -> std::path::PathBuf {
        self.hub.config().dir.clone()
    }

    fn install_file(&self) {
        (self.post)(UiMsg::Mods(ModsMsgBox(ModsMsg::InstallFile)));
    }

    fn install_url(&self, url: &str) {
        (self.post)(UiMsg::Mods(ModsMsgBox(ModsMsg::InstallUrl {
            url: url.to_string(),
        })));
    }

    fn update(&self, id: &str) {
        (self.post)(UiMsg::Mods(ModsMsgBox(ModsMsg::Update {
            id: id.to_string(),
        })));
    }

    fn remove(&self, id: &str) {
        (self.post)(UiMsg::Mods(ModsMsgBox(ModsMsg::Remove {
            id: id.to_string(),
        })));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_ctl::{CtlError, Grant};
    use flare_mods::hub::Config;
    use std::sync::mpsc;
    use std::time::Duration;

    fn mod_json(perms: &str) -> String {
        let tools = if perms.contains("tools") {
            r#","tools":[{"name":"append_note","description":"d","input_schema":{"type":"object"}}]"#
        } else {
            ""
        };
        format!(
            r#"{{"id":"notes","name":"Notes","version":"1.0.0","protocol":1,"description":"d","permissions":{perms},"panel":{{"entry":"index.html","title":"Notes"}}{tools}}}"#
        )
    }

    type Recorded = Arc<Mutex<Vec<(Method, Value, Option<String>, String)>>>;

    struct Rig {
        _dir: tempfile::TempDir,
        ctl: ModCtl,
        rx: mpsc::Receiver<String>,
        calls: Recorded,
    }

    /// A dispatcher that records its calls and answers `{"ok":true}`.
    struct Rec {
        calls: Recorded,
    }

    impl Dispatcher for Rec {
        fn resolve_token(&self, _: &str) -> Option<Grant> {
            None
        }
        fn resolve_tab_token(&self, _: &str) -> Option<String> {
            None
        }
        fn call(&self, ctx: &CallContext, m: Method, p: Value) -> Result<Value, CtlError> {
            self.calls
                .lock()
                .unwrap()
                .push((m, p, ctx.tab.clone(), ctx.grant.kind().to_string()));
            Ok(serde_json::json!({"ok": true}))
        }
    }

    fn rig(perms: &str) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("plugins");
        let f = root.join("notes");
        std::fs::create_dir_all(&f).unwrap(); // sync-ok: test code only
        std::fs::write(f.join("mod.json"), mod_json(perms)).unwrap(); // sync-ok: test code only
        std::fs::write(f.join("index.html"), "<p>x</p>").unwrap(); // sync-ok: test code only
        let hub = Arc::new(Hub::new(Config {
            dir: root,
            old_mods_dir: None,
            state_file: dir.path().join("mods.json"),
        }));
        hub.scan();
        hub.set_enabled("notes", true).unwrap();
        let (tx, rx) = mpsc::channel::<String>();
        let tx = Mutex::new(tx);
        let post: Post = Arc::new(move |m: UiMsg| {
            if let UiMsg::Mods(ModsMsgBox(ModsMsg::Reply { json, .. })) = m {
                let _ = tx.lock().unwrap().send(json);
            }
        });
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut ctl = ModCtl::new(hub, post);
        ctl.set_dispatcher(Arc::new(Rec {
            calls: Arc::clone(&calls),
        }));
        ctl.bind("t1", "notes");
        Rig {
            _dir: dir,
            ctl,
            rx,
            calls,
        }
    }

    fn call(id: u64, method: &str, params: &str) -> String {
        format!(r#"{{"t":"call","id":{id},"method":"{method}","params":{params}}}"#)
    }

    fn reply_of(r: &Rig) -> Value {
        serde_json::from_str(&r.rx.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap()
    }

    #[test]
    fn a_mod_without_browser_is_refused_every_browser_method_before_anything_runs() {
        let mut r = rig(r#"["panel","tools"]"#);
        for (i, m) in [
            "browser.open",
            "browser.look",
            "browser.eval",
            "browser.click",
        ]
        .iter()
        .enumerate()
        {
            let out = r
                .ctl
                .on_page_message("t1", &call(i as u64 + 1, m, r#"{"url":"https://a.test/"}"#))
                .expect("refused at once");
            let v: Value = serde_json::from_str(&out).unwrap();
            assert_eq!(v["t"], "reply");
            assert_eq!(v["id"], i as u64 + 1);
            assert_eq!(v["error"]["code"], "forbidden", "{m}");
        }
        assert!(
            r.calls.lock().unwrap().is_empty(),
            "the dispatcher saw nothing"
        );
        assert_eq!(r.ctl.inflight("notes"), 0);
    }

    #[test]
    fn a_mod_with_browser_is_allowed_the_method_but_its_own_panel_is_not_built() {
        let mut r = rig(r#"["panel","browser"]"#);
        let out = r
            .ctl
            .on_page_message(
                "t1",
                &call(1, "browser.open", r#"{"url":"https://a.test/"}"#),
            )
            .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"]["code"], "unavailable");
        assert!(r.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn an_allowed_call_runs_on_the_dispatcher_with_the_mods_grant_and_its_own_tab() {
        let mut r = rig(r#"["panel","view","tools"]"#);
        assert_eq!(
            r.ctl
                .on_page_message("t1", &call(7, "view.text", r#"{"text":"hi"}"#)),
            None,
            "answered later"
        );
        assert_eq!(r.ctl.inflight("notes"), 1);
        let v = reply_of(&r);
        assert_eq!(
            v,
            serde_json::json!({"t":"reply","id":7,"result":{"ok":true}})
        );
        let calls = r.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, Method::ViewText);
        assert_eq!(calls[0].1, serde_json::json!({"text":"hi"}));
        assert_eq!(
            calls[0].2.as_deref(),
            Some("t1"),
            "the tab the page sits in"
        );
        assert_eq!(calls[0].3, "mod");
    }

    #[test]
    fn a_permission_the_mod_does_not_hold_is_forbidden_and_hello_and_respond_are_not_calls() {
        let mut r = rig(r#"["panel","tools"]"#);
        for (m, code) in [
            ("view.text", "forbidden"),
            ("convo.state", "forbidden"),
            ("mods.call", "forbidden"),
            ("settings.set", "forbidden"),
            ("mods.enable", "forbidden"),
            ("shell.input", "forbidden"),
            ("tools.respond", "bad_params"),
            ("hello", "bad_params"),
            ("nope.nothing", "unknown_method"),
        ] {
            let out = r.ctl.on_page_message("t1", &call(1, m, "{}")).unwrap();
            let v: Value = serde_json::from_str(&out).unwrap();
            assert_eq!(v["error"]["code"], code, "{m}");
        }
        assert!(r.calls.lock().unwrap().is_empty());
        // The words say which grant and method.
        let out = r
            .ctl
            .on_page_message("t1", &call(1, "view.text", "{}"))
            .unwrap();
        assert!(out.contains("view.text"), "{out}");
    }

    #[test]
    fn a_mod_that_is_turned_off_is_refused_at_once_even_with_its_page_open() {
        let mut r = rig(r#"["panel","view"]"#);
        r.ctl.hub().set_enabled("notes", false).unwrap();
        let out = r
            .ctl
            .on_page_message("t1", &call(1, "view.text", "{}"))
            .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"]["code"], "unauthorized");
        assert!(r.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn a_page_makes_only_so_many_calls_at_once() {
        let mut r = rig(r#"["panel"]"#);
        r.ctl.inflight.insert("notes".into(), MAX_PAGE_CALLS);
        let out = r
            .ctl
            .on_page_message("t1", &call(1, "tabs.list", "{}"))
            .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"]["code"], "rate_limited");
        r.ctl.call_done("notes");
        assert_eq!(r.ctl.inflight("notes"), MAX_PAGE_CALLS - 1);
    }

    #[test]
    fn without_a_dispatcher_a_call_is_unavailable() {
        let mut r = rig(r#"["panel"]"#);
        r.ctl.dispatcher = None;
        let out = r
            .ctl
            .on_page_message("t1", &call(1, "tabs.list", "{}"))
            .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"]["code"], "unavailable");
    }

    #[test]
    fn respond_answers_the_mods_own_call_only_and_needs_the_tools_permission() {
        let mut r = rig(r#"["panel","tools"]"#);
        let c = r
            .ctl
            .hub()
            .begin_call("mod_notes_append_note", serde_json::json!({"text": "a"}))
            .unwrap();
        let text = format!(
            r#"{{"t":"respond","call_id":"{}","result":{{"count":1}}}}"#,
            c.id
        );
        assert_eq!(r.ctl.on_page_message("t1", &text), None);
        let v = r
            .ctl
            .hub()
            .wait(c, Duration::from_secs(1), &|| false)
            .unwrap();
        assert_eq!(v, serde_json::json!({"count": 1}));

        // Without `tools` the answer is ignored, and the call stays open.
        let mut r = rig(r#"["panel"]"#);
        // (the tool table is empty without the permission, so open a call through the hub's own door)
        assert!(
            r.ctl
                .hub()
                .begin_call("mod_notes_append_note", serde_json::json!({}))
                .is_err()
        );
        let _ = r
            .ctl
            .on_page_message("t1", r#"{"t":"respond","call_id":"c1","result":1}"#);
    }

    #[test]
    fn a_message_from_a_tab_with_no_mod_page_or_garbage_is_ignored() {
        let mut r = rig(r#"["panel"]"#);
        assert_eq!(
            r.ctl.on_page_message("other", &call(1, "tabs.list", "{}")),
            None
        );
        assert_eq!(r.ctl.on_page_message("t1", "not json"), None);
        assert_eq!(r.ctl.on_page_message("t1", r#"{"t":"nope"}"#), None);
        assert!(r.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn messages_for_a_page_wait_until_it_is_ready_and_come_out_in_order() {
        let mut r = rig(r#"["panel"]"#);
        assert_eq!(r.ctl.queue_or_send("t1", "a".into()), None);
        assert_eq!(r.ctl.queue_or_send("t1", "b".into()), None);
        assert_eq!(
            r.ctl.set_ready("t1", true),
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(r.ctl.queue_or_send("t1", "c".into()), Some("c".into()));
        assert!(r.ctl.set_ready("t1", false).is_empty());
        assert_eq!(r.ctl.queue_or_send("t1", "d".into()), None);
        assert_eq!(r.ctl.queue_or_send("nowhere", "x".into()), None);
    }

    #[test]
    fn binding_the_same_mod_again_keeps_the_page_state_and_another_mod_starts_over() {
        let mut r = rig(r#"["panel"]"#);
        r.ctl.set_ready("t1", true);
        r.ctl.bind("t1", "notes");
        assert!(r.ctl.is_ready("t1"));
        r.ctl.bind("t1", "other");
        assert!(!r.ctl.is_ready("t1"));
        assert_eq!(r.ctl.mod_of("t1"), Some("other"));
        assert_eq!(r.ctl.unbind("t1").as_deref(), Some("other"));
        assert_eq!(r.ctl.mod_of("t1"), None);
        assert!(r.ctl.bound().is_empty());
    }

    /// Pages are kept by slot: a tab's panel and a bar beside the same tab are two pages, a background page
    /// belongs to no tab, and a reply names the slot that asked.
    #[test]
    fn pages_are_kept_by_slot_and_a_background_page_calls_with_no_tab() {
        let mut r = rig(r#"["panel","view"]"#);
        let (panel, bar, bg) = (
            Slot::panel("t1"),
            Slot::bar("t1"),
            Slot::background("notes"),
        );
        assert_eq!(
            r.ctl.mod_of(&panel),
            Some("notes"),
            "a bare tab is its panel"
        );
        r.ctl.bind(&bar, "notes");
        r.ctl.bind(&bg, "notes");
        // In slot order: by owner, then kind.
        assert_eq!(
            r.ctl.slots_of("notes"),
            [bg.clone(), panel.clone(), bar.clone()]
        );
        assert_ne!(r.ctl.generation(&panel), r.ctl.generation(&bar));
        // Each page is ready, and queues, on its own.
        r.ctl.set_ready(&bar, true);
        assert!(r.ctl.is_ready(&bar) && !r.ctl.is_ready("t1"));
        assert_eq!(r.ctl.queue_or_send(&bar, "b".into()), Some("b".into()));
        assert_eq!(r.ctl.queue_or_send("t1", "p".into()), None);

        // A call from the background page runs with no tab, and its reply names the background slot.
        let (tx, rx) = mpsc::channel::<(Slot, u64)>();
        let tx = Mutex::new(tx);
        r.ctl.post = Arc::new(move |m: UiMsg| {
            if let UiMsg::Mods(ModsMsgBox(ModsMsg::Reply {
                slot, generation, ..
            })) = m
            {
                let _ = tx.lock().unwrap().send((slot, generation));
            }
        });
        assert_eq!(
            r.ctl.on_page_message(&bg, &call(1, "view.text", "{}")),
            None
        );
        let (slot, generation) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(slot, bg);
        assert_eq!(Some(generation), r.ctl.generation(&bg));
        assert_eq!(
            r.calls.lock().unwrap()[0].2,
            None,
            "a background page has no tab"
        );
        r.ctl.on_page_message(&bar, &call(2, "view.text", "{}"));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap().0, bar);
        assert_eq!(r.calls.lock().unwrap()[1].2.as_deref(), Some("t1"));

        // Letting the panel go leaves the bar and the background page.
        assert_eq!(r.ctl.unbind("t1").as_deref(), Some("notes"));
        assert_eq!(r.ctl.slots_of("notes"), [bg, bar]);
    }

    #[test]
    fn the_debug_form_of_a_message_prints_its_kind_only() {
        let m = ModsMsgBox(ModsMsg::Reply {
            slot: Slot::panel("t"),
            mod_id: "notes".into(),
            generation: 0,
            json: "SECRET".into(),
        });
        assert_eq!(format!("{m:?}"), "ModsMsg::Reply");
    }

    #[test]
    fn the_page_info_lists_the_permissions_in_plain_words_and_no_token() {
        let r = rig(r#"["panel","tools"]"#);
        let info = info_of(r.ctl.hub(), &r.ctl.save_error, &r.ctl.install);
        assert_eq!(info.mods.len(), 1);
        assert_eq!(
            info.mods[0].permissions,
            vec![
                "Show its panel in a tab".to_string(),
                "Answer the agent's tool calls".to_string()
            ]
        );
        assert!(info.mods[0].enabled && info.mods[0].has_panel);
        assert!(!format!("{info:?}").contains("token"));
    }

    #[test]
    fn a_scan_runs_on_a_worker_and_a_second_ask_runs_after_it() {
        let dir = tempfile::tempdir().unwrap();
        let hub = Arc::new(Hub::new(Config {
            dir: dir.path().join("plugins"),
            old_mods_dir: Some(dir.path().join("mods")),
            state_file: dir.path().join("mods.json"),
        }));
        let (tx, rx) = mpsc::channel::<String>();
        let tx = Mutex::new(tx);
        let post: Post = Arc::new(move |m: UiMsg| {
            if let UiMsg::Mods(ModsMsgBox(ModsMsg::Scanned(_))) = m {
                let _ = tx.lock().unwrap().send("scanned".into());
            }
        });
        let mut ctl = ModCtl::new(hub, post);
        ctl.start_scan();
        ctl.start_scan(); // asked while the first may still run
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "scanned");
        // The folder was made so the Open plugins folder button has somewhere to go; the old mods folder was not.
        assert!(dir.path().join("plugins").is_dir());
        assert!(!dir.path().join("mods").exists());
        // Whether or not the second ask was folded into the first, finishing the scan starts at most one more.
        let again = ctl.scan_done();
        if again {
            assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "scanned");
        }
        assert_eq!(ctl.scans, 1);
    }

    /// The page's words for a protocol 2 plugin come from the hub: who made it, the permissions in plain words and the
    /// warnings; a protocol 1 mod beside it is filled exactly as before.
    #[test]
    fn the_page_info_of_a_plugin_has_its_maker_and_warnings_and_a_mod_has_none() {
        let dir = tempfile::tempdir().unwrap();
        // The mod starts in the old mods folder: the scan moves it in beside the plugin.
        let mods = dir.path().join("mods");
        let f = mods.join("notes");
        std::fs::create_dir_all(&f).unwrap(); // sync-ok: test code only
        std::fs::write(f.join("mod.json"), mod_json(r#"["panel"]"#)).unwrap(); // sync-ok: test code only
        std::fs::write(f.join("index.html"), "<p>x</p>").unwrap(); // sync-ok: test code only
        let plugins = dir.path().join("plugins");
        let p = plugins.join("steer");
        std::fs::create_dir_all(&p).unwrap(); // sync-ok: test code only
        std::fs::write(
            p.join("plugin.json"),
            r#"{"id":"steer","name":"Steer","version":"2.0.0","protocol":2,"description":"d","author":"someone","homepage":"https://example.com/","permissions":["sessions","input","network"]}"#,
        )
        .unwrap(); // sync-ok: test code only
        std::fs::create_dir_all(plugins.join("Junk")).unwrap(); // sync-ok: test code only
        let hub = Hub::new(Config {
            dir: plugins.clone(),
            old_mods_dir: Some(mods),
            state_file: dir.path().join("mods.json"),
        });
        hub.scan();
        let info = info_of(
            &hub,
            &Mutex::new(None),
            &Mutex::new(InstallState::default()),
        );
        assert_eq!(info.dir, plugins.to_string_lossy());
        let notes = info.mods.iter().find(|m| m.id == "notes").unwrap();
        assert!(!notes.plugin && notes.author.is_empty() && notes.warnings.is_empty());
        assert_eq!(notes.permissions, ["Show its panel in a tab"]);
        let steer = info.mods.iter().find(|m| m.id == "steer").unwrap();
        assert!(steer.plugin && steer.full_control);
        assert_eq!(
            (steer.author.as_str(), steer.homepage.as_str()),
            ("someone", "https://example.com/")
        );
        assert_eq!(
            steer.permissions,
            [
                "Read what you and Claude say in sessions",
                "Type and run commands in your terminals (full control)",
                "Use the internet"
            ]
        );
        assert_eq!(
            steer.warnings,
            [
                flare_mods::hub::WARN_INPUT,
                flare_mods::hub::WARN_INPUT_SESSIONS,
                flare_mods::hub::WARN_NETWORK_SESSIONS,
                flare_mods::hub::WARN_NETWORK
            ]
        );
        assert_eq!(info.problems.len(), 1);
        assert_eq!(info.problems[0].folder, "Junk");
        assert!(
            !dir.path().join("mods").exists(),
            "the emptied mods folder is gone"
        );
    }
}
