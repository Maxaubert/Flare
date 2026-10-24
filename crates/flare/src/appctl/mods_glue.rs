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

use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::Sender;

use flare_ctl::{CtlError, Dispatcher, code};
use flare_mods::bridge;
use flare_mods::hub::{Change, Hub};
use flare_surfaces::app::AppSpec;
use flare_surfaces::modview;
use serde_json::{Value, json};

use super::{AppCtl, UiMsg};
use crate::ctl_app::CtlOp;
use crate::mods_ctl::{ModCtl, ModsMsg, ModsSettings};
use crate::settings_ctl::{ModsPort, SettingsMsg};
use crate::web_ctl::WebOut;

type Reply = Sender<Result<Value, CtlError>>;

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
                tab,
                mod_id,
                generation,
                json,
            } => {
                let live = match self.mods.as_mut() {
                    Some(c) => {
                        c.call_done(&mod_id);
                        // Call ids restart in every document: a reply for an older one is nobody's.
                        c.mod_of(&tab) == Some(mod_id.as_str())
                            && c.generation(&tab) == Some(generation)
                    }
                    None => false,
                };
                // The page that asked may be gone (its panel closed, the mod turned off): then nobody listens.
                if live {
                    self.web.post_message(&tab, &json);
                }
            }
            ModsMsg::Rescan => {
                if let Some(c) = self.mods.as_mut() {
                    c.start_scan();
                }
            }
            ModsMsg::SetEnabled { id, on, shown } => {
                let applied = self
                    .mods
                    .as_ref()
                    .map(|c| (c.hub().set_enabled_shown(&id, on, shown.as_deref()), c));
                match applied {
                    Some((Ok(change), c)) => {
                        c.persist(&id);
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
        if change.tools_changed {
            self.publish_mods_changed();
        }
    }

    /// Lets go of the pages that are gone: a tab whose panel no longer shows the mod it was bound to has nothing
    /// to answer with, so the calls waiting on that mod fail (unless another tab still shows it).
    pub(super) fn sync_mods(&mut self) {
        let Some(m) = self.mods.as_mut() else { return };
        for (tab, id) in m.bound() {
            if self.panels.mod_of(&tab) != Some(id.as_str()) {
                m.unbind(&tab);
                if m.tabs_of(&id).is_empty() {
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
                            && m.tabs_of(&id).iter().all(|t| *t == tab)
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
                    if m.tabs_of(&id).iter().all(|t| *t == tab) {
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
        let CtlOp::ShowMod { tab, mod_id } = op else {
            let _ = reply.send(Err(CtlError::new(code::INTERNAL, "not a mod call")));
            return;
        };
        if !self.is_shell_tab(&tab) {
            let _ = reply.send(Err(CtlError::new(code::NO_TAB, "that tab is closed")));
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
            })).collect::<Vec<_>>(),
            "errors": s.errors.iter().map(|e| json!({"folder": e.folder, "reason": e.reason})).collect::<Vec<_>>(),
            "notes": s.notes,
            "tools": m.hub().tools().iter().map(|t| t.name.clone()).collect::<Vec<_>>(),
            "bound": m.bound().iter().map(|(t, id)| json!({
                "tab": t, "mod": id, "ready": m.is_ready(t),
            })).collect::<Vec<_>>(),
            "pending": s.mods.iter().map(|e| (e.id.clone(), m.hub().pending_of(&e.id))).filter(|(_, n)| *n > 0)
                .map(|(id, n)| json!({"mod": id, "calls": n})).collect::<Vec<_>>(),
            "panels": self.panels.mod_tabs().iter().map(|(t, id)| json!({"tab": t, "mod": id})).collect::<Vec<_>>(),
        })
    }
}
