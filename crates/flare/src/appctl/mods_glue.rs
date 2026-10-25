//! Mods in the window (Surfaces stage 8): the part of the mods controller that needs `AppCtl`'s state, the tabs,
//! the panels, the browser and the channel's events.
//!
//! - A mod's panel is a web panel at the mod's own address (`flare_surfaces::modview`), made by [`AppCtl::show_mod_panel`]
//!   from the Settings button, the channel's `panel.open` kind `mod:<id>` or the agent's tool call.
//! - A tool call (`mods.call`) is put in front of the page of the tab it came from: the mod's panel is opened
//!   there if it is not showing, the page is made (hidden when the tab is behind), and the call waits for the page
//!   to be ready. The page's answer comes back through the hub, never through here.
//! - What a mod's page sends over its bridge is checked against the mod's live grant by `ModCtl` and run on a worker.
//! - Turning a mod off, or a rescan that loses it, closes its panels and fails its calls at once.
//! - Plugins phase 3: a protocol 2 plugin that is on and has a `background` page gets it, hidden and tied to no
//!   tab ([`AppCtl::sync_backgrounds`]); its tools are answered there instead of in a panel. A page that ends is
//!   started again (3 times in 5 minutes, then it stays stopped and Settings says so), and one that does not
//!   answer the ping in 10 seconds is closed and counts as ended. Turning a plugin off closes every view of it and
//!   takes down its decor before Settings hears it is off.
//! - Events: after each refresh the tabs are compared with what the plugins last heard ([`tab_events`]), and each
//!   change goes to every page that listens to it and may see that tab (`tabs` sees all of them, a page without it
//!   only its own tab), at most [`MAX_EVENTS_PER_ROUND`] a page a round.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::Instant;

use flare_ctl::{CtlError, Dispatcher, code};
use flare_mods::bridge;
use flare_mods::hub::{Change, Hub};
use flare_surfaces::app::AppSpec;
use flare_surfaces::modview;
use flare_surfaces::slot::{Slot, ViewKind};
use flare_tabs::decor::Source as DecorSource;
use flare_tabs::tabs::TabKind;
use serde_json::{Value, json};

use super::{AppCtl, UiMsg};
use crate::ctl_app::CtlOp;
use crate::mods_ctl::{
    ModCtl, ModsMsg, ModsMsgBox, ModsSettings, PING_EVERY, STOPPED_CAP, STOPPED_CRASHED,
};
use crate::settings_ctl::{ModsPort, SettingsMsg};
use crate::web_ctl::WebOut;

type Reply = Sender<Result<Value, CtlError>>;

/// The timer that pings the background pages.
const PING_TIMER: &str = "plugins.ping";
/// Events one page is handed in one round (one refresh) at most; the rest of the round is dropped for it.
pub const MAX_EVENTS_PER_ROUND: usize = 32;

/// A tab as a plugin is told of it (`tabs.list` for a plugin that holds `tabs`, and every `tabs` event). The id
/// is the tab's public one, never its `FLARE_TAB_TOKEN`.
#[derive(Clone, Debug, PartialEq)]
pub struct TabFact {
    pub id: String,
    pub title: String,
    pub folder: String,
    pub active: bool,
    /// `claude`, `codex`, or None when no agent the plugin can name runs there.
    pub agent: Option<&'static str>,
    /// `working`, `question`, `done`, `failed` or `idle`.
    pub state: &'static str,
    /// The Claude session the tab's agent is in, when one is known.
    pub session: Option<String>,
    /// What the tab's open panel shows: `browser` for the web panel, else the panel's kind (`image`, `text`,
    /// `changes`, `tasks`, `app`, `mod:<id>`); None when no panel is open.
    pub panel: Option<String>,
}

impl TabFact {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "title": self.title,
            "folder": self.folder,
            "active": self.active,
            "agent": self.agent,
            "state": self.state,
            "session": self.session,
            "panel": self.panel,
        })
    }

    fn agent_json(&self) -> Value {
        json!({"tab": self.id, "kind": self.agent, "state": self.state, "session": self.session})
    }
}

/// What changed between two looks at the tabs, as events: `(event name, tab id, payload)`. `tabs` events say
/// `opened`, `closed`, `focused`, `renamed`, `moved` (its place among the tabs that were there both times) and
/// `panel` (what its panel shows), with the tab; an `agent` event says the agent's kind, state or session changed.
pub fn tab_events(before: &[TabFact], now: &[TabFact]) -> Vec<(&'static str, String, Value)> {
    let mut out = Vec::new();
    let tabs = |kind: &str, f: &TabFact| {
        (
            "tabs",
            f.id.clone(),
            json!({"kind": kind, "tab": f.to_json()}),
        )
    };
    let was = |id: &str| before.iter().find(|b| b.id == id);
    for f in now {
        if was(&f.id).is_none() {
            out.push(tabs("opened", f));
        }
    }
    for b in before {
        if !now.iter().any(|f| f.id == b.id) {
            out.push(tabs("closed", b));
        }
    }
    // The tabs that were there both times, in their order now: the longest run of them that kept its order stayed,
    // the others moved (one tab dragged across three is one move, not four).
    let kept_now: Vec<&TabFact> = now.iter().filter(|f| was(&f.id).is_some()).collect();
    let places: Vec<usize> = kept_now
        .iter()
        .filter_map(|f| before.iter().position(|b| b.id == f.id))
        .collect();
    let stayed = longest_rising(&places);
    for (i, f) in kept_now.iter().enumerate() {
        let Some(b) = was(&f.id) else { continue };
        if f.active && !b.active {
            out.push(tabs("focused", f));
        }
        if f.title != b.title {
            out.push(tabs("renamed", f));
        }
        if !stayed.get(i).copied().unwrap_or(true) {
            out.push(tabs("moved", f));
        }
        if f.panel != b.panel {
            out.push(tabs("panel", f));
        }
        if (f.agent, f.state, &f.session) != (b.agent, b.state, &b.session) {
            out.push(("agent", f.id.clone(), f.agent_json()));
        }
    }
    // A tab opened in front is focused as well.
    for f in now {
        if f.active && was(&f.id).is_none() && before.iter().any(|b| b.active) {
            out.push(tabs("focused", f));
        }
    }
    out
}

/// A `tabs` event as a page without `tabs` hears it: its kind, and of the tab what own-tab `tabs.list` gives.
fn own_tab_event(data: &Value) -> Value {
    let tab = &data["tab"];
    json!({
        "kind": data["kind"],
        "tab": {"id": tab["id"], "title": tab["title"], "active": tab["active"]},
    })
}

/// Which entries of `xs` make up its longest rising run (the first such run when there are several).
fn longest_rising(xs: &[usize]) -> Vec<bool> {
    let n = xs.len();
    let mut len = vec![1usize; n];
    let mut prev = vec![usize::MAX; n];
    for i in 0..n {
        for j in 0..i {
            if xs[j] < xs[i] && len[j] + 1 > len[i] {
                len[i] = len[j] + 1;
                prev[i] = j;
            }
        }
    }
    let mut keep = vec![false; n];
    let best = (0..n).max_by_key(|&i| (len[i], std::cmp::Reverse(i)));
    let mut at = best;
    while let Some(i) = at {
        keep[i] = true;
        at = (prev[i] != usize::MAX).then_some(prev[i]);
    }
    keep
}

impl AppCtl {
    /// Hands the window the mods hub and the dispatcher a mod's page calls through (startup, after the control
    /// channel is up). The first scan starts at once, on a worker.
    pub fn set_mods(&mut self, hub: Arc<Hub>, dispatcher: Option<Arc<dyn Dispatcher>>) {
        let mut ctl = ModCtl::new(hub, Arc::clone(&self.deps.post));
        if let Some(d) = dispatcher {
            ctl.set_dispatcher(d);
        }
        ctl.start_scan();
        self.mods = Some(ctl);
    }

    /// Where safe mode's record of this run is (startup; None in the e2e and the tests).
    pub fn set_plugin_run_record(&mut self, file: Option<std::path::PathBuf>) {
        if let Some(m) = self.mods.as_mut() {
            m.set_run_record(file);
        }
    }

    /// The Mods page's door to the mods, for the Settings controller.
    pub fn mods_port(&self) -> Option<Rc<dyn ModsPort>> {
        let m = self.mods.as_ref()?;
        Some(Rc::new(ModsSettings {
            hub: Arc::clone(m.hub()),
            post: Arc::clone(&self.deps.post),
            save_error: Arc::clone(&m.save_error),
        }))
    }

    fn settings_changed(&self) {
        (self.deps.post)(UiMsg::Settings(SettingsMsg::Mods));
    }

    /// Tells the channel's clients (the MCP servers) that the tools the agent sees changed.
    fn publish_mods_changed(&self) {
        if let Some(events) = &self.ctl_events {
            events("mods.changed", json!({}), None);
        }
    }

    /// One message of the mods controller (a worker's answer, or the Settings page).
    pub(super) fn on_mods_msg(&mut self, m: ModsMsg) {
        if self.mods.is_none() {
            return;
        }
        match m {
            ModsMsg::Scanned(change) => {
                if let Some(c) = self.mods.as_mut() {
                    c.scan_done();
                }
                self.apply_mods_change(&change);
                self.settings_changed();
            }
            ModsMsg::Persisted { id, result } => {
                if let Some(c) = &self.mods {
                    let mut slot = c.save_error.lock().unwrap_or_else(|e| e.into_inner());
                    *slot = match result {
                        Ok(()) => None,
                        Err(e) => {
                            log::warn!("the switch of mod {id} was not saved: {e}");
                            Some(e)
                        }
                    };
                }
                self.settings_changed();
            }
            ModsMsg::Reply {
                slot,
                mod_id,
                generation,
                json,
            } => {
                let live = match self.mods.as_mut() {
                    Some(c) => {
                        c.call_done(&mod_id);
                        // Call ids restart in every document: a reply for an older one is nobody's.
                        c.mod_of(&slot) == Some(mod_id.as_str())
                            && c.generation(&slot) == Some(generation)
                    }
                    None => false,
                };
                // The page that asked may be gone (its panel closed, the mod turned off): then nobody listens.
                if live {
                    self.post_to_slot(&slot, &json);
                }
            }
            ModsMsg::Tick => self.plugins_tick(Instant::now()),
            ModsMsg::Rescan => {
                if let Some(c) = self.mods.as_mut() {
                    c.start_scan();
                }
            }
            ModsMsg::SetEnabled { id, on, shown } => {
                let applied = self
                    .mods
                    .as_ref()
                    .map(|c| (c.hub().set_enabled_shown(&id, on, shown.as_ref()), c));
                match applied {
                    Some((Ok(change), c)) => {
                        c.persist(&id);
                        // A switch starts the restart count afresh: a stopped page may run again.
                        if let Some(m) = self.mods.as_mut() {
                            m.bg_forget(&id);
                        }
                        self.apply_mods_change(&change);
                    }
                    Some((Err(why), c)) => {
                        *c.save_error.lock().unwrap_or_else(|e| e.into_inner()) = Some(why);
                    }
                    None => {}
                }
                self.settings_changed();
            }
            ModsMsg::ShowPanel { id } => self.show_mod_in_recent_tab(&id),
        }
    }

    /// What a change of the mods means for the window: the panels of a mod that lost its token close, and the
    /// agent's clients hear that the tools changed.
    fn apply_mods_change(&mut self, change: &Change) {
        // Every view of a plugin that lost its token closes (its background page too) and its decor goes, before
        // Settings hears that it is off.
        for id in &change.revoked {
            self.plugin_off(id);
        }
        let mut closed = Vec::new();
        for id in &change.revoked {
            for (tab, mod_id) in self.panels.mod_tabs() {
                if mod_id == *id && self.panels.close(&tab) {
                    closed.push(tab);
                }
            }
        }
        if !closed.is_empty() {
            self.refresh();
            for tab in &closed {
                self.panel_changed(tab);
            }
        }
        self.sync_mods();
        self.sync_backgrounds();
        // What the plugins that are on add to the sidebar and the action places, read again.
        self.sync_plugin_surfaces();
        self.refresh();
        if let Some(m) = self.mods.as_mut() {
            m.note_plugins_on();
        }
        if change.tools_changed {
            self.publish_mods_changed();
        }
    }

    /// What a plugin that lost its token leaves: its pages are told (they close right after), its background page
    /// closes, and everything it drew on the tabs goes.
    fn plugin_off(&mut self, id: &str) {
        let off = bridge::event_message("plugin", &json!({"kind": "off"}));
        let pages: Vec<Slot> = self
            .mods
            .as_ref()
            .map(|m| {
                m.listeners("plugin")
                    .into_iter()
                    .filter(|(_, mid)| mid == id)
                    .map(|(s, _)| s)
                    .collect()
            })
            .unwrap_or_default();
        for slot in pages {
            if self.mods.as_ref().is_some_and(|m| m.is_ready(&slot)) {
                self.post_to_slot(&slot, &off);
            }
        }
        self.stop_background(id);
        self.clear_plugin_decor(id);
        self.clear_plugin_surfaces(id);
    }

    /// Everything the plugin put on any tab goes. True when a tab lost something it showed.
    fn clear_plugin_decor(&mut self, id: &str) -> bool {
        let changed = self
            .decor
            .clear_source_everywhere(&DecorSource::Plugin(id.to_string()));
        if changed {
            self.refresh();
        }
        changed
    }

    /// Speaks to the page in `slot`: a tab's panel or a background page (a bar has no page yet).
    pub(super) fn post_to_slot(&mut self, slot: &Slot, json: &str) {
        match slot.kind {
            ViewKind::Panel => self.web.post_message(&slot.owner, json),
            ViewKind::Background => self.web.post_to_background(&slot.owner, json),
            ViewKind::Bar => {}
        }
    }

    // ---- background pages -------------------------------------------------------------------------------------

    /// Makes the background pages that run agree with the plugins that are on: one that is off or gone closes,
    /// one that is on and not stopped opens (at most [`flare_mods::hub::MAX_BACKGROUND`]).
    pub(super) fn sync_backgrounds(&mut self) {
        let Some(m) = self.mods.as_ref() else { return };
        let hub = Arc::clone(m.hub());
        let wanted = hub.with_background();
        let running = m.backgrounds();
        for id in &running {
            if !wanted.contains(id) {
                self.stop_background(id);
            }
        }
        let mut count = self.mods.as_ref().map_or(0, |m| m.backgrounds().len());
        for id in wanted {
            if self.mods.as_ref().is_some_and(|m| m.bg_running(&id)) || hub.stopped(&id).is_some() {
                continue;
            }
            if count >= flare_mods::hub::MAX_BACKGROUND {
                hub.set_stopped(&id, Some(STOPPED_CAP));
                self.settings_changed();
                continue;
            }
            if self.start_background(&id) {
                count += 1;
            }
        }
    }

    /// Opens the plugin's background page. False when there is none to open (the plugin is off, or the browser
    /// cannot be made in this run).
    fn start_background(&mut self, id: &str) -> bool {
        let Some(m) = self.mods.as_mut() else {
            return false;
        };
        let Some((folder, entry)) = m.hub().background_of(id) else {
            return false;
        };
        m.bg_started(id);
        self.web
            .open_background(id, modview::mod_spec(id, &folder, &entry));
        if !self.web.background_ids().iter().any(|b| b == id) {
            if let Some(m) = self.mods.as_mut() {
                m.bg_ended(id);
            }
            return false;
        }
        log::info!("plugin {id}: its background page started");
        self.arm_ping();
        true
    }

    /// Closes the plugin's background page (turned off, gone, or stopped). The calls only it could answer fail.
    fn stop_background(&mut self, id: &str) {
        let Some(m) = self.mods.as_mut() else { return };
        if !m.bg_running(id) {
            return;
        }
        m.bg_ended(id);
        if m.hub().answers_in_background(id) {
            m.fail_calls(id, "the plugin's background page closed before it answered");
        }
        self.web.close_background(id);
        log::info!("plugin {id}: its background page closed");
        if self
            .mods
            .as_ref()
            .is_some_and(|m| m.backgrounds().is_empty())
        {
            crate::timer_registry::clear(PING_TIMER);
        }
    }

    /// The plugin's background page ended on its own (its process, a failed load, a ping with no answer): its
    /// calls fail and its decor goes, and it is started again unless it ended too often.
    fn background_lost(&mut self, id: &str, why: &str) {
        let Some(m) = self.mods.as_mut() else { return };
        if !m.bg_running(id) {
            return;
        }
        let again = m.bg_crashed(id, Instant::now());
        if m.hub().answers_in_background(id) {
            m.fail_calls(id, "the plugin's background page ended before it answered");
        }
        // Started again at once: the plugins' browser is kept for the new page.
        self.web.close_background_keeping(id, again);
        self.clear_plugin_decor(id);
        // What it set in its sections and on its actions goes too; it sets them again when it starts.
        self.clear_plugin_surfaces(id);
        log::warn!("plugin {id}: its background page ended ({why})");
        if again {
            self.start_background(id);
        } else {
            if let Some(m) = self.mods.as_ref() {
                m.hub().set_stopped(id, Some(STOPPED_CRASHED));
            }
            log::warn!("plugin {id}: its background page ended too often and stays stopped");
            // A stopped plugin's sections and actions leave the screen.
            self.sync_plugin_surfaces();
            self.refresh();
            self.settings_changed();
        }
        if self
            .mods
            .as_ref()
            .is_some_and(|m| m.backgrounds().is_empty())
        {
            crate::timer_registry::clear(PING_TIMER);
        }
    }

    /// The ping timer runs while any background page does.
    fn arm_ping(&self) {
        if crate::timer_registry::running(PING_TIMER) {
            return;
        }
        let post = Arc::clone(&self.deps.post);
        let every = u64::try_from(PING_EVERY.as_millis()).unwrap_or(10_000);
        crate::timer_registry::set(PING_TIMER, every, true, move || {
            post(UiMsg::Mods(ModsMsgBox(ModsMsg::Tick)));
        });
    }

    /// One round of the ping timer at `now`: every ready background page is pinged, and one that left the last
    /// ping unanswered is closed and counts as ended.
    pub(crate) fn plugins_tick(&mut self, now: Instant) {
        let Some(m) = self.mods.as_mut() else { return };
        let round = m.ping_round(now);
        for (id, msg) in round.ping {
            self.web.post_to_background(&id, &msg);
        }
        for id in round.silent {
            self.background_lost(&id, "no answer to the ping in 10 seconds");
        }
    }

    // ---- events ---------------------------------------------------------------------------------------------------

    /// Every shell tab as a plugin is told of it, in strip order.
    pub(crate) fn tab_facts(&self) -> Vec<TabFact> {
        use flare_agent::kinds::DetectedAgent as D;
        let active = self.model.state.active_id.as_deref();
        let ind = self.hub.indicator();
        self.model
            .state
            .tabs
            .iter()
            .zip(self.model.labels())
            .filter(|(t, _)| t.kind == TabKind::Shell)
            .map(|(t, title)| {
                let id = t.id.as_str();
                let state = if ind.question_ids().contains(id) {
                    "question"
                } else if ind.failed_ids().contains(id) {
                    "failed"
                } else if ind.working_ids().contains(id) {
                    "working"
                } else if ind.done_ids().contains(id) {
                    "done"
                } else {
                    "idle"
                };
                TabFact {
                    id: t.id.clone(),
                    title,
                    folder: t.cwd.clone(),
                    active: active == Some(id),
                    agent: match self.hub.agent_kind(id) {
                        Some(D::Claude) => Some("claude"),
                        Some(D::Codex) => Some("codex"),
                        Some(D::Other) | None => None,
                    },
                    state,
                    session: self.hub.claude_session(id).map(str::to_string),
                    panel: self.panels.snap(id).filter(|s| s.open).map(|s| {
                        if s.kind == "web" {
                            "browser".to_string()
                        } else {
                            s.kind
                        }
                    }),
                }
            })
            .collect()
    }

    /// What changed on the tabs since the plugins last heard goes to the pages that listen (after each refresh).
    /// Nothing is looked at while no page listens to `tabs` or `agent`.
    pub(super) fn plugin_events(&mut self) {
        if !self
            .mods
            .as_ref()
            .is_some_and(|m| m.any_listens(&["tabs", "agent"]))
        {
            self.plugin_seen = None;
            return;
        }
        let now = self.tab_facts();
        let Some(before) = self.plugin_seen.replace(now.clone()) else {
            return;
        };
        if before == now {
            return;
        }
        let events = tab_events(&before, &now);
        self.deliver_events(&events);
    }

    /// Hands each event to every page that listens to it and may see its tab: a plugin that holds `tabs` sees every
    /// tab, any other page only the tab it sits in (a background page, in none). A page without `tabs` hears of its
    /// own tab what own-tab `tabs.list` gives (id, title, active) and no `agent` event: its approval never covered
    /// the folder, the agent or the session.
    pub(crate) fn deliver_events(&mut self, events: &[(&'static str, String, Value)]) {
        let Some(m) = self.mods.as_mut() else { return };
        let hub = Arc::clone(m.hub());
        let mut wide: HashMap<String, bool> = HashMap::new();
        let mut handed: HashMap<Slot, usize> = HashMap::new();
        let mut out: Vec<(Slot, String)> = Vec::new();
        for (name, tab, data) in events {
            let msg = bridge::event_message(name, data);
            let narrow =
                (*name == "tabs").then(|| bridge::event_message(name, &own_tab_event(data)));
            for (slot, mod_id) in m.listeners(name) {
                let sees_all = *wide.entry(mod_id.clone()).or_insert_with(|| {
                    hub.grant_of_mod(&mod_id)
                        .is_some_and(|g| g.may_act_on_any_tab())
                });
                if !sees_all && slot.tab() != Some(tab.as_str()) {
                    continue;
                }
                let msg = match (sees_all, &narrow) {
                    (true, _) => &msg,
                    (false, Some(n)) => n,
                    // An `agent` event is for a plugin that holds `tabs` only.
                    (false, None) => continue,
                };
                let n = handed.entry(slot.clone()).or_insert(0);
                if *n >= MAX_EVENTS_PER_ROUND {
                    m.dropped += 1;
                    continue;
                }
                *n += 1;
                if let Some(now) = m.queue_or_send(&slot, msg.to_string()) {
                    out.push((slot, now));
                }
            }
        }
        for (slot, msg) in out {
            self.post_to_slot(&slot, &msg);
        }
    }

    /// Lets go of the pages that are gone: a tab whose panel no longer shows the mod it was bound to has nothing
    /// to answer with, so the calls waiting on that mod fail (unless another tab still shows it).
    pub(super) fn sync_mods(&mut self) {
        let Some(m) = self.mods.as_mut() else { return };
        for (slot, id) in m.bound() {
            // A tab's panel is the only slot with a page so far; the panels say which mod each one shows.
            if slot.is_panel() && self.panels.mod_of(&slot.owner) != Some(id.as_str()) {
                m.unbind(&slot);
                if m.slots_of(&id).is_empty() {
                    m.fail_calls(&id, "the mod's panel was closed before it answered");
                }
            }
        }
    }

    /// The quit: every call waiting on a mod's page is answered.
    pub(super) fn mods_shutdown(&mut self) {
        if let Some(m) = &self.mods {
            m.hub().fail_all_calls("Flare is closing");
        }
    }

    /// What the browser told the window about a mod's page.
    pub(super) fn on_mod_web(&mut self, out: WebOut) {
        match out {
            WebOut::BgMessage { plugin, json } => {
                let slot = Slot::background(&plugin);
                let reply = self
                    .mods
                    .as_mut()
                    .filter(|m| m.mod_of(&slot) == Some(plugin.as_str()))
                    .and_then(|m| m.on_page_message(&slot, &json));
                if let Some(r) = reply {
                    self.web.post_to_background(&plugin, &r);
                }
            }
            WebOut::BgPage { plugin, ready } => {
                let slot = Slot::background(&plugin);
                let queued = match self.mods.as_mut() {
                    Some(m) if m.mod_of(&slot) == Some(plugin.as_str()) => {
                        // A page that was ready began to load again: the calls already handed to it are gone.
                        let reload = !ready && m.is_ready(&slot);
                        let queued = m.set_ready(&slot, ready);
                        if reload && m.hub().answers_in_background(&plugin) {
                            m.fail_calls(
                                &plugin,
                                "the plugin's background page reloaded before it answered",
                            );
                        }
                        queued
                    }
                    _ => Vec::new(),
                };
                for q in queued {
                    self.web.post_to_background(&plugin, &q);
                }
            }
            WebOut::BgLost { plugin } => self.background_lost(&plugin, "its page or browser ended"),
            WebOut::ModMessage { tab, mod_id, json } => {
                // The message counts only for the mod the tab's panel shows now: a message the page of another
                // mod queued before the panel switched is dropped, never run with this mod's grant.
                if self.panels.mod_of(&tab) != Some(mod_id.as_str()) {
                    return;
                }
                self.bind_mod_tab(&tab);
                if self.mods.as_ref().and_then(|m| m.mod_of(&tab)) != Some(mod_id.as_str()) {
                    return;
                }
                let reply = self
                    .mods
                    .as_mut()
                    .and_then(|m| m.on_page_message(&tab, &json));
                if let Some(r) = reply {
                    self.web.post_message(&tab, &r);
                }
            }
            WebOut::ModPage { tab, ready } => {
                self.bind_mod_tab(&tab);
                let queued = match self.mods.as_mut() {
                    Some(m) => {
                        // A page that was ready began to load again: the calls already handed to the old
                        // document are gone with it.
                        let reload = !ready && m.is_ready(&tab);
                        let queued = m.set_ready(&tab, ready);
                        if reload
                            && let Some(id) = m.mod_of(&tab).map(str::to_string)
                            && m.slots_of(&id).iter().all(|s| *s == Slot::panel(&tab))
                        {
                            m.fail_calls(&id, "the mod's page reloaded before it answered");
                        }
                        queued
                    }
                    None => Vec::new(),
                };
                for q in queued {
                    self.web.post_message(&tab, &q);
                }
            }
            WebOut::ModLost { tab } => {
                // The tab keeps showing the mod (the next call makes a new page), so the binding stays while the
                // panel names it; the calls waiting on the dead page fail.
                let showing = self.panels.mod_of(&tab).map(str::to_string);
                if let Some(m) = self.mods.as_mut()
                    && let Some(id) = m.mod_of(&tab).map(str::to_string)
                {
                    if showing.as_deref() == Some(id.as_str()) {
                        m.page_lost(&tab);
                    } else {
                        m.unbind(&tab);
                    }
                    if m.slots_of(&id).iter().all(|s| *s == Slot::panel(&tab)) {
                        m.fail_calls(&id, "the mod's page ended before it answered");
                    }
                }
            }
            _ => {}
        }
    }

    /// The tab's panel shows a mod: the controller knows the page (it may have been shown before it was told).
    fn bind_mod_tab(&mut self, tab: &str) {
        let Some(id) = self.panels.mod_of(tab).map(str::to_string) else {
            return;
        };
        if let Some(m) = self.mods.as_mut() {
            m.bind(tab, &id);
        }
    }

    /// Shows the mod's panel on the tab, if it is on and has one. A tab that already shows it is left alone
    /// (opened again if it was closed). The address the panel's browser is made for comes back.
    pub(super) fn show_mod_panel(&mut self, tab: &str, mod_id: &str) -> Result<AppSpec, CtlError> {
        let Some(m) = self.mods.as_ref() else {
            return Err(CtlError::new(
                code::UNAVAILABLE,
                "mods are not available in this run of Flare",
            ));
        };
        if m.hub().grant_of_mod(mod_id).is_none() {
            return Err(CtlError::new(
                code::NOT_FOUND,
                format!("the mod \"{mod_id}\" is not turned on"),
            ));
        }
        let Some(found) = m.hub().found(mod_id) else {
            return Err(CtlError::new(
                code::NOT_FOUND,
                format!("there is no mod \"{mod_id}\""),
            ));
        };
        let Some(panel) = found.manifest.panel.clone() else {
            return Err(CtlError::new(
                code::NOT_FOUND,
                format!("the mod \"{mod_id}\" has no panel"),
            ));
        };
        let spec = modview::mod_spec(mod_id, &found.folder, &panel.entry);
        if self.panels.mod_of(tab) == Some(mod_id) {
            if let Some(m) = self.mods.as_mut() {
                m.bind(tab, mod_id);
            }
            if !self.panels.is_open(tab) {
                self.panels.reopen(tab);
                self.refresh();
                self.panel_changed(tab);
            }
            return Ok(spec);
        }
        if !self.panels.show_mod(tab, spec.clone(), &panel.title) {
            return Err(CtlError::new(
                code::INTERNAL,
                "the mod's panel could not be shown",
            ));
        }
        if let Some(m) = self.mods.as_mut() {
            m.bind(tab, mod_id);
        }
        self.refresh();
        self.panel_changed(tab);
        Ok(spec)
    }

    /// `panel.open` kind `mod:<id>`: the panel on the caller's tab (a tab behind keeps its place).
    pub(super) fn ctl_show_mod(&mut self, op: CtlOp, reply: Reply) {
        let CtlOp::ShowMod {
            tab,
            mod_id,
            own,
            generation,
        } = op
        else {
            let _ = reply.send(Err(CtlError::new(code::INTERNAL, "not a mod call")));
            return;
        };
        if !self.is_shell_tab(&tab) {
            let _ = reply.send(Err(CtlError::new(code::NO_TAB, "that tab is closed")));
            return;
        }
        // Checked while the plugin was on, but switched off since (and maybe on again): its old run opens nothing.
        if self
            .mods
            .as_ref()
            .is_some_and(|m| m.hub().generation(&mod_id) != generation)
        {
            let _ = reply.send(Err(CtlError::new(
                code::UNAUTHORIZED,
                "the plugin was turned off",
            )));
            return;
        }
        // On a tab that is not the caller's own, the panel is taken only when it is closed or shows this mod already
        // (design 13): the person's web page or another plugin's panel stays.
        if !own && self.panels.is_open(&tab) && self.panels.mod_of(&tab) != Some(mod_id.as_str()) {
            let _ = reply.send(Err(CtlError::new(
                code::BUSY,
                "that tab's panel shows something else",
            )));
            return;
        }
        let answer = self.show_mod_panel(&tab, &mod_id).map(|spec| {
            self.mark_agent_panel(&tab);
            // A tab that is behind gets its page made hidden, so the mod is running whether or not anyone looks.
            self.web.ensure_hidden(&tab, &spec.url.clone(), Some(spec));
            self.web_moved();
            let snap = self.panels.snap(&tab);
            json!({"panel": snap.map(|s| Self::panel_info(&s))})
        });
        let _ = reply.send(answer);
    }

    /// `mods.call` on the window's side: the mod's panel is shown in the caller's tab, its page made, and the
    /// agent's call put in front of it (queued until the page has loaded). The answer says the call was handed
    /// over; the page answers through the hub.
    pub(super) fn ctl_mod_tool(&mut self, op: CtlOp, reply: Reply) {
        let CtlOp::ModTool {
            tab,
            mod_id,
            call_id,
            tool,
            input,
        } = op
        else {
            let _ = reply.send(Err(CtlError::new(code::INTERNAL, "not a mod call")));
            return;
        };
        if !self.is_shell_tab(&tab) {
            let _ = reply.send(Err(CtlError::new(code::NO_TAB, "that tab is closed")));
            return;
        }
        // A protocol 2 plugin with a background page answers there: no panel is opened (plugins phase 3).
        if let Some(m) = self.mods.as_mut()
            && m.hub().answers_in_background(&mod_id)
        {
            if !m.bg_running(&mod_id) {
                let why = m
                    .hub()
                    .stopped(&mod_id)
                    .unwrap_or_else(|| "the plugin's background page is not running".to_string());
                let _ = reply.send(Err(CtlError::new(code::UNAVAILABLE, why)));
                return;
            }
            // The page hears which tab the agent called from (plugins phase 4), so a tool can act on it.
            let msg = bridge::tool_event_from(&call_id, &tool, &input, &tab);
            if let Some(now) = m.queue_or_send(Slot::background(&mod_id), msg) {
                self.web.post_to_background(&mod_id, &now);
            }
            let _ = reply.send(Ok(json!({})));
            return;
        }
        let spec = match self.show_mod_panel(&tab, &mod_id) {
            Ok(s) => {
                self.mark_agent_panel(&tab);
                s
            }
            Err(e) => {
                let _ = reply.send(Err(e));
                return;
            }
        };
        let url = spec.url.clone();
        // A page whose process ended (or failed to load) is made again, unless the browser itself is missing.
        if self.web.failure(&tab).is_some_and(|(_, missing)| !missing) {
            self.web.replace_tab(&tab, &url, Some(spec));
        } else {
            self.web.ensure_hidden(&tab, &url, Some(spec));
        }
        if let Some((why, missing)) = self.web.failure(&tab) {
            let msg = if missing {
                format!(
                    "the WebView2 Runtime is not installed ({})",
                    crate::web_ctl::RUNTIME_URL
                )
            } else {
                format!("the web view could not be made: {why}")
            };
            let _ = reply.send(Err(CtlError::new(code::UNAVAILABLE, msg)));
            return;
        }
        let msg = bridge::tool_event(&call_id, &tool, &input);
        let now = self.mods.as_mut().and_then(|m| m.queue_or_send(&tab, msg));
        if let Some(now) = now {
            self.web.post_message(&tab, &now);
        }
        self.web_moved();
        let _ = reply.send(Ok(json!({})));
    }

    /// The Settings button: the mod's panel in the shell tab used last, which comes to the front.
    fn show_mod_in_recent_tab(&mut self, id: &str) {
        let tab = self
            .model
            .mru()
            .iter()
            .find(|t| self.is_shell_tab(t))
            .cloned()
            .or_else(|| {
                flare_tabs::tabs::shell_tabs(&self.model.state)
                    .first()
                    .map(|t| t.id.clone())
            });
        let Some(tab) = tab else { return };
        match self.show_mod_panel(&tab, id) {
            Ok(spec) => {
                let url = spec.url.clone();
                self.web.ensure_hidden(&tab, &url, Some(spec));
                self.model.pick(&tab);
                self.refresh();
                self.focus_terminal();
            }
            Err(e) => log::info!("the mod's panel was not shown: {}", e.message),
        }
    }

    /// The `mods.state` probe: what the window and the hub hold (no token).
    pub fn e2e_mods_state(&self) -> Value {
        let Some(m) = &self.mods else {
            return Value::Null;
        };
        let s = m.hub().snapshot();
        json!({
            "scanned": s.scanned,
            "scans": m.scans,
            "mods": s.mods.iter().map(|e| json!({
                "id": e.id, "enabled": e.enabled, "tools": e.tool_count,
                "needs_approval": e.needs_approval,
                "panel": e.panel_title,
                "protocol": e.protocol,
            })).collect::<Vec<_>>(),
            "errors": s.errors.iter().map(|e| json!({"folder": e.folder, "reason": e.reason})).collect::<Vec<_>>(),
            "notes": s.notes,
            "tools": m.hub().tools().iter().map(|t| t.name.clone()).collect::<Vec<_>>(),
            "bound": m.bound().iter().filter(|(s, _)| s.is_panel()).map(|(s, id)| json!({
                "tab": s.owner, "mod": id, "ready": m.is_ready(s),
            })).collect::<Vec<_>>(),
            "pending": s.mods.iter().map(|e| (e.id.clone(), m.hub().pending_of(&e.id))).filter(|(_, n)| *n > 0)
                .map(|(id, n)| json!({"mod": id, "calls": n})).collect::<Vec<_>>(),
            "panels": self.panels.mod_tabs().iter().map(|(t, id)| json!({"tab": t, "mod": id})).collect::<Vec<_>>(),
            "backgrounds": m.backgrounds().iter().map(|id| json!({
                "plugin": id, "ready": m.is_ready(Slot::background(id)),
                "stopped": m.hub().stopped(id),
            })).collect::<Vec<_>>(),
            "dropped": m.dropped,
            "safe_mode": s.safe_mode,
        })
    }
}
