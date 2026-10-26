//! Install, update and remove a plugin, and run its helper program (plugins phase 7), in the window.
//!
//! - **Install** (Settings > Plugins): from a file (the picker answers on a worker) or an `https` address. A worker
//!   unpacks and checks the zip (`crate::plugin_install`); when it replaces a plugin that is on, its views and helper are
//!   stopped here first (the hub keeps it paused meanwhile), then a worker puts it in place and scans, and the change is
//!   applied as a scan's is. One install, update or removal at a time; Settings shows what the last one said.
//! - **Update**: from the address it was installed from, or else a file; the zip must hold the same plugin.
//! - **Remove**: the plugin is turned off (every view closes, its helper is stopped) before a worker removes it.
//! - **Helpers**: after every change of the plugins, the helpers that run are brought in line with the plugins that may
//!   have one ([`AppCtl::sync_helpers`]); turning a plugin off stops its helper. A helper hears, over the channel, the
//!   events a background page of the plugin would: `tabs` and `agent` when it holds `tabs`, `session` with `sessions`.

use std::sync::Arc;

use serde_json::Value;

use super::{AppCtl, UiMsg};
use crate::mods_ctl::{ModsMsg, ModsMsgBox, OwnerEvents, PickZip};
use crate::settings_ctl::SettingsMsg;

/// The sentence while no file picker exists in this run.
const NO_PICKER: &str = "No file picker is available in this run of Flare.";

impl AppCtl {
    /// The plugins' helpers log to `logs` and are told `pipe` (startup, once the channel is up).
    pub fn set_plugin_helpers(&mut self, logs: std::path::PathBuf, pipe: String) {
        let post = Arc::clone(&self.deps.post);
        if let Some(m) = self.mods.as_mut() {
            m.helpers = Some(flare_mods::helper::Host::new(logs, pipe, move |note| {
                post(UiMsg::Mods(ModsMsgBox(ModsMsg::Helper(note))));
            }));
        }
        self.sync_helpers();
    }

    /// Where the channel's events for one plugin's helper go (startup).
    pub fn set_plugin_owner_events(&mut self, events: OwnerEvents) {
        if let Some(m) = self.mods.as_mut() {
            m.owner_events = Some(events);
        }
    }

    /// The file picker of Install from file (startup).
    pub fn set_pick_zip(&mut self, pick: PickZip) {
        if let Some(m) = self.mods.as_mut() {
            m.pick_zip = Some(pick);
        }
    }

    fn settings_redraw(&self) {
        (self.deps.post)(UiMsg::Settings(SettingsMsg::Mods));
    }

    /// Marks an install as running. False (and nothing changes) when one already runs.
    fn install_begin(&self) -> bool {
        let Some(m) = self.mods.as_ref() else {
            return false;
        };
        let mut s = m.install.lock().unwrap_or_else(|e| e.into_inner());
        if s.installing {
            return false;
        }
        s.installing = true;
        s.note = None;
        drop(s);
        self.settings_redraw();
        true
    }

    /// An install, update or removal ended with `note` (failed, the sentence).
    fn install_end(&self, failed: bool, note: String) {
        if let Some(m) = self.mods.as_ref() {
            let mut s = m.install.lock().unwrap_or_else(|e| e.into_inner());
            s.installing = false;
            s.note = Some((failed, note));
        }
        self.settings_redraw();
    }

    /// A sentence for Settings with no install running.
    fn install_say(&self, failed: bool, note: &str) {
        if let Some(m) = self.mods.as_ref() {
            let mut s = m.install.lock().unwrap_or_else(|e| e.into_inner());
            if !s.installing {
                s.note = Some((failed, note.to_string()));
            }
        }
        self.settings_redraw();
    }

    fn pick_zip_for(&self, expect: Option<String>) {
        let Some(m) = self.mods.as_ref() else { return };
        let Some(pick) = m.pick_zip.clone() else {
            self.install_say(true, NO_PICKER);
            return;
        };
        let post = Arc::clone(&self.deps.post);
        pick(Arc::new(move |path| {
            post(UiMsg::Mods(ModsMsgBox(ModsMsg::Picked {
                path,
                expect: expect.clone(),
            })));
        }));
    }

    /// The install messages of the mods controller (Settings and the workers).
    pub(super) fn on_install_msg(&mut self, m: ModsMsg) {
        let Some(hub) = self.mods.as_ref().map(|c| Arc::clone(c.hub())) else {
            return;
        };
        let post = Arc::clone(&self.deps.post);
        match m {
            ModsMsg::InstallFile => self.pick_zip_for(None),
            ModsMsg::InstallUrl { url } => {
                if url.trim().is_empty() {
                    self.install_say(true, "Paste the https address of a plugin's .zip first.");
                } else if self.install_begin()
                    && !crate::plugin_install::stage_url(hub, url, None, post)
                {
                    self.install_end(true, "The install could not start.".into());
                }
            }
            ModsMsg::Update { id } => {
                let source = hub
                    .snapshot()
                    .mods
                    .into_iter()
                    .find(|e| e.id == id)
                    .and_then(|e| e.source);
                match source {
                    Some(url) => {
                        if self.install_begin()
                            && !crate::plugin_install::stage_url(hub, url, Some(id), post)
                        {
                            self.install_end(true, "The update could not start.".into());
                        }
                    }
                    None => self.pick_zip_for(Some(id)),
                }
            }
            ModsMsg::Picked { path: None, .. } => {}
            ModsMsg::Picked {
                path: Some(path),
                expect,
            } => {
                if self.install_begin()
                    && !crate::plugin_install::stage_file(hub, path, expect, post)
                {
                    self.install_end(true, "The install could not start.".into());
                }
            }
            ModsMsg::Staged(Err(why)) => self.install_end(true, why),
            ModsMsg::Staged(Ok(staged)) => {
                let id = staged.id().to_string();
                if staged.was_on {
                    // Its views and helper stop before the folder is replaced, and stay stopped until it is.
                    hub.pause(&id);
                    self.close_plugin_views(&id);
                }
                let helpers = self.mods.as_ref().and_then(|m| m.helpers.clone());
                if !crate::plugin_install::commit(hub, staged, helpers, post) {
                    if let Some(m) = self.mods.as_ref() {
                        m.hub().unpause(&id);
                    }
                    self.install_end(true, "The install could not start.".into());
                    self.apply_mods_change(&flare_mods::hub::Change::default());
                }
            }
            ModsMsg::Installed(Ok(done)) => {
                if let Some(m) = self.mods.as_mut() {
                    m.bg_forget(&done.id);
                }
                self.apply_mods_change(&done.change);
                self.install_end(false, done.sentence());
            }
            ModsMsg::Installed(Err(why)) => {
                // A plugin paused for the update starts again as it was.
                self.apply_mods_change(&flare_mods::hub::Change::default());
                self.install_end(true, why);
            }
            ModsMsg::Remove { id } => {
                if !self.install_begin() {
                    return;
                }
                // Off first: every view closes and its helper stops before its folder goes.
                match hub.set_enabled(&id, false) {
                    Ok(change) => self.apply_mods_change(&change),
                    Err(why) => log::warn!("plugin {id} was not turned off before removal: {why}"),
                }
                self.stop_helper(&id);
                let name = hub.name_of(&id);
                let helpers = self.mods.as_ref().and_then(|m| m.helpers.clone());
                if !crate::plugin_install::remove(hub, id, helpers, post) {
                    self.install_end(true, format!("{name} could not be removed."));
                }
            }
            ModsMsg::Removed { id, result } => match result {
                Ok(change) => {
                    self.apply_mods_change(&change);
                    self.forget_plugin_sections(&id);
                    self.install_end(
                        false,
                        format!("{id} was removed. Its folder is in the Recycle Bin."),
                    );
                }
                Err(why) => {
                    // The folder stayed: the switch it was turned off with is saved, as any switch is.
                    if let Some(m) = self.mods.as_ref() {
                        m.persist(&id);
                    }
                    self.apply_mods_change(&flare_mods::hub::Change::default());
                    self.install_end(true, why);
                }
            },
            ModsMsg::Helper(note) => self.on_helper_note(note),
            _ => {}
        }
    }

    fn on_helper_note(&mut self, note: flare_mods::helper::Note) {
        use flare_mods::helper::Note;
        match note {
            Note::Started { id, .. } => log::debug!("plugin {id}: its helper runs"),
            Note::Restarted { id } => log::info!("plugin {id}: its helper was started again"),
            Note::Stopped { id, why } => {
                if let Some(m) = self.mods.as_ref() {
                    m.hub().set_helper_stopped(&id, Some(&why));
                }
                self.settings_redraw();
            }
        }
    }

    // ---- helpers --------------------------------------------------------------------------------------------------

    /// Makes the helpers that run agree with the plugins that may have one: one whose plugin is off, gone, paused or
    /// stopped is stopped, and one that may run and does not is started (on its own thread).
    pub(super) fn sync_helpers(&mut self) {
        let Some(m) = self.mods.as_ref() else { return };
        let Some(host) = m.helpers.clone() else {
            return;
        };
        let hub = Arc::clone(m.hub());
        let wanted = hub.with_helper();
        for id in host.running() {
            if !wanted.contains(&id) {
                host.stop(&id);
            }
        }
        for id in wanted {
            if !host.is_running(&id)
                && let Some(spec) = hub.helper_of(&id)
            {
                host.start(spec);
            }
        }
    }

    /// Stops the plugin's helper now (its job is terminated; nothing waits here).
    pub(super) fn stop_helper(&mut self, id: &str) {
        if let Some(host) = self.mods.as_ref().and_then(|m| m.helpers.clone()) {
            host.stop(id);
        }
    }

    /// Whether any helper runs (events are worked out for it even when no page listens).
    pub(super) fn helpers_run(&self) -> bool {
        self.mods
            .as_ref()
            .and_then(|m| m.helpers.as_ref())
            .is_some_and(|h| !h.running().is_empty())
    }

    /// The running helpers that may hear of every tab (their plugin holds `tabs`).
    pub(super) fn wide_helpers(&self) -> Vec<String> {
        let Some(m) = self.mods.as_ref() else {
            return Vec::new();
        };
        let Some(host) = m.helpers.as_ref() else {
            return Vec::new();
        };
        host.running()
            .into_iter()
            .filter(|id| {
                m.hub()
                    .grant_of_mod(id)
                    .is_some_and(|g| g.may_act_on_any_tab())
            })
            .collect()
    }

    /// Hands one event to the plugin's helper over the channel.
    pub(super) fn to_helper(&self, plugin: &str, name: &str, data: &Value) {
        if let Some(send) = self.mods.as_ref().and_then(|m| m.owner_events.as_ref()) {
            send(name, data.clone(), plugin);
        }
    }

    /// The `tabs` and `agent` events of a round, to every helper that may hear them (one without `tabs` has no tab
    /// of its own and hears none), at most `MAX_EVENTS_PER_ROUND` a helper.
    pub(super) fn deliver_to_helpers(&self, events: &[(&'static str, String, Value)]) {
        for id in self.wide_helpers() {
            for (name, _, data) in events.iter().take(super::mods_glue::MAX_EVENTS_PER_ROUND) {
                self.to_helper(&id, name, data);
            }
        }
    }
}
