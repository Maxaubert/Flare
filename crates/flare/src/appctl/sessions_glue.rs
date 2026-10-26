//! What plugins hear of Claude sessions and what they type (plugins phase 5, design 6, 7 and 13), in the window.
//!
//! - **Followers.** After each refresh and each message of a plugin's page, the tabs whose transcript is followed are
//!   brought in line with the pages that listen to `session` ([`AppCtl::sync_session_followers`]): a tab is followed
//!   while a page of a plugin that holds `sessions` listens and may see it, and its Claude session is known.
//! - **Events.** A follower's entries go to every such page as `session` `{tab, session, role, text, tool?, at?,
//!   dropped?}`, at most 50 a second per plugin; what is over is dropped and counted in the next event.
//! - **`session.read`.** The window says where the tab's session is filed; the call's thread reads it.
//! - **`tabs.input`.** Checked on the call's thread; here the tab, the plugin's generation and its `input` are looked
//!   at again and the text is typed as the person's paste is (`PluginInput::Paste`), with the Enter a moment later
//!   when asked (`PluginInput::Enter`), and only while the plugin is still on and the tab still open (rule 2).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use flare_convo::follow::Said;
use flare_ctl::{CtlError, Grant, Permission, code};
use flare_mods::bridge;
use flare_surfaces::slot::Slot;
use flare_term::handle::PluginInput;
use serde_json::{Value, json};

use super::{AppCtl, UiMsg};
use crate::mods_ctl::{ModsMsg, ModsMsgBox};
use crate::plugin_sessions::{ENTER_AFTER_MS, Want, entry_json};

impl AppCtl {
    /// Whether the plugin's live grant holds `p`.
    fn plugin_holds(&self, plugin: &str, p: Permission) -> bool {
        self.mods.as_ref().is_some_and(|m| {
            matches!(
                m.hub().grant_of_mod(plugin),
                Some(Grant::Mod { permissions, .. }) if permissions.contains(&p)
            )
        })
    }

    /// The pages that listen to `session` and may hear of `tab`, by plugin: the plugin holds `sessions`, and the page
    /// sits in the tab, or the plugin holds `tabs`, or the person clicked one of its actions on the tab.
    fn session_listeners(&self, tab: &str) -> BTreeMap<String, Vec<Slot>> {
        let mut out: BTreeMap<String, Vec<Slot>> = BTreeMap::new();
        let Some(m) = self.mods.as_ref() else {
            return out;
        };
        let hub = m.hub();
        for (slot, plugin) in m.listeners("session") {
            let Some(Grant::Mod { permissions, .. }) = hub.grant_of_mod(&plugin) else {
                continue;
            };
            if !permissions.contains(&Permission::Sessions) {
                continue;
            }
            let sees = permissions.contains(&Permission::Tabs)
                || slot.tab() == Some(tab)
                || hub.has_click_scope(&plugin, tab);
            if sees {
                out.entry(plugin).or_default().push(slot);
            }
        }
        out
    }

    /// Follows the transcript of every tab some page listens to, and no other ([`crate::plugin_sessions`]).
    pub(super) fn sync_session_followers(&mut self) {
        let listening = self
            .mods
            .as_ref()
            .is_some_and(|m| m.any_listens(&["session"]));
        if !listening && self.sessions.count() == 0 {
            return;
        }
        let mut wants = Vec::new();
        if listening {
            for (tab, folder) in self.shell_tabs() {
                let Some(session) = self.hub.claude_session(&tab).map(str::to_string) else {
                    continue;
                };
                if !self.session_listeners(&tab).is_empty() {
                    wants.push(Want {
                        tab,
                        folder,
                        session,
                    });
                }
            }
        }
        let dir = self.convo.claude_dir();
        let post = Arc::clone(&self.deps.post);
        if self.sessions.sync(&wants, dir, &post) {
            log::debug!("plugins: following {} session(s)", self.sessions.count());
        }
    }

    /// A follower's new entries: each goes as a `session` event to the pages that listen and may hear of the tab,
    /// within each plugin's share.
    pub(super) fn on_said(&mut self, tab: &str, session: &str, said: Said) {
        if !self.sessions.follows(tab, session) {
            return;
        }
        let groups = self.session_listeners(tab);
        if groups.is_empty() {
            return;
        }
        let now = Instant::now();
        let mut out: Vec<(Slot, String)> = Vec::new();
        for (plugin, slots) in &groups {
            self.sessions.add_dropped(plugin, said.skipped, now);
            for e in &said.entries {
                let Some(dropped) = self.sessions.spend(plugin, now) else {
                    continue;
                };
                let mut data = entry_json(e);
                data["tab"] = json!(tab);
                data["session"] = json!(session);
                if dropped > 0 {
                    data["dropped"] = json!(dropped);
                }
                let msg = bridge::event_message("session", &data);
                let Some(m) = self.mods.as_mut() else { return };
                for slot in slots {
                    if let Some(now) = m.queue_or_send(slot, msg.clone()) {
                        out.push((slot.clone(), now));
                    }
                }
            }
        }
        for (slot, msg) in out {
            self.post_to_slot(&slot, &msg);
        }
    }

    /// A plugin stopped: its share of the sessions starts afresh, and a tab nobody listens to is no longer followed.
    pub(super) fn sessions_plugin_off(&mut self, plugin: &str) {
        self.sessions.forget_plugin(plugin);
    }

    /// `session.read` on the UI thread: where the tab's Claude session is filed (the call's thread reads it).
    pub(super) fn ctl_session_of(&self, tab: &str) -> Result<Value, CtlError> {
        let Some((_, folder)) = self.shell_tabs().into_iter().find(|(id, _)| id == tab) else {
            return Err(CtlError::new(code::NO_TAB, "that tab is closed"));
        };
        let dir = self
            .convo
            .claude_dir()
            .map(|d| d.to_string_lossy().into_owned());
        Ok(json!({
            "session": self.hub.claude_session(tab),
            "folder": folder,
            "claude_dir": dir,
        }))
    }

    /// `tabs.input` on the UI thread: the worker checked the text and the tab's scope; the tab must still be open and
    /// the plugin still on in the generation that was checked, holding `input`. The text goes as a paste; the Enter
    /// follows after [`ENTER_AFTER_MS`], checked again then.
    pub(super) fn ctl_tab_input(
        &mut self,
        plugin: String,
        tab: String,
        text: String,
        enter: bool,
        generation: u64,
    ) -> Result<Value, CtlError> {
        if !self.is_shell_tab(&tab) {
            return Err(CtlError::new(code::NO_TAB, "that tab is closed"));
        }
        self.plugin_still_on(&plugin, generation)?;
        if !self.plugin_holds(&plugin, Permission::Input) {
            return Err(CtlError::new(
                code::FORBIDDEN,
                "typing into a terminal needs the input permission",
            ));
        }
        let Some(h) = self.handle_of(&tab) else {
            return Err(CtlError::new(
                code::UNAVAILABLE,
                "that tab's shell is not running",
            ));
        };
        if !h.plugin_input(PluginInput::Paste(&text)) {
            return Err(CtlError::new(code::BAD_PARAMS, "there is nothing to type"));
        }
        if enter {
            let post = Arc::clone(&self.deps.post);
            let job = move || {
                std::thread::sleep(Duration::from_millis(ENTER_AFTER_MS)); // sync-ok: a worker of its own waits
                post(UiMsg::Mods(ModsMsgBox(ModsMsg::Enter {
                    plugin,
                    tab,
                    generation,
                })));
            };
            if let Err(e) = std::thread::Builder::new()
                .name("flare-plugin-enter".into())
                .spawn(job)
            {
                log::warn!("a plugin's Enter could not be scheduled: {e}");
                return Ok(json!({"typed": true, "enter": false}));
            }
        }
        Ok(json!({"typed": true, "enter": enter}))
    }

    /// The Enter a plugin's `tabs.input` asked for: pressed only when the tab is still open and the plugin still on in
    /// the same generation with `input`.
    pub(super) fn plugin_enter(&mut self, plugin: &str, tab: &str, generation: u64) {
        if !self.is_shell_tab(tab)
            || self.plugin_still_on(plugin, generation).is_err()
            || !self.plugin_holds(plugin, Permission::Input)
        {
            return;
        }
        if let Some(h) = self.handle_of(tab) {
            h.plugin_input(PluginInput::Enter);
        }
    }

    /// The tabs whose session is followed now (the tests and the probe).
    pub(crate) fn followed_sessions(&self) -> Vec<String> {
        self.sessions.tabs()
    }
}
