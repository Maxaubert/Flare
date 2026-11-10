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

use flare_agent::question::looks_like_question;
use flare_convo::follow::Said;
use flare_ctl::{CtlError, Grant, Permission, code};
use flare_mods::bridge;
use flare_shells::paste::{path_shell_of, quote_path, sanitize_paste};
use flare_surfaces::slot::Slot;
use flare_term::handle::PluginInput;
use serde_json::{Value, json};

use super::{AppCtl, UiMsg};
use crate::mods_ctl::{ModsMsg, ModsMsgBox};
use crate::plugin_sessions::{ENTER_AFTER_MS, Want, entry_json};

/// How long the "Ask agent" note stays up.
const ASK_NOTE_MS: u64 = 4_000;

impl AppCtl {
    /// Why this tab cannot be asked now, if it cannot: no live agent, or a question box a paste could answer.
    fn ask_refusal(&self, tab: &str) -> Option<&'static str> {
        use crate::ask_agent::{NOTE_NO_AGENT, NOTE_QUESTION_UP};
        let Some(h) = self
            .handle_of(tab)
            .filter(|h| self.is_shell_tab(tab) && h.shell_id().is_some())
        else {
            return Some(NOTE_NO_AGENT);
        };
        if !self.hub.agent_here(tab) {
            return Some(NOTE_NO_AGENT);
        }
        if self.hub.question_up(tab) || looks_like_question(&h.question_rows()) {
            return Some(NOTE_QUESTION_UP);
        }
        None
    }

    /// Says why nothing was sent, over the terminal, for a few seconds.
    fn ask_note(&self, why: &str) {
        let Some(app) = self.app() else { return };
        app.set_ask_note(why.into());
        let weak = self.me.clone();
        crate::timer_registry::set("ask.note", ASK_NOTE_MS, false, move || {
            if let Some(rc) = weak.upgrade()
                && let Ok(c) = rc.try_borrow()
                && let Some(app) = c.app()
            {
                app.set_ask_note("".into());
            }
        });
    }

    /// Types `text` at the tab's agent's prompt and gives the terminal the keys. It is never submitted: no Enter
    /// follows, and where the program did not ask for bracketed paste no CR is written at all. Checks the gates
    /// again, for an answer that arrives after the click.
    fn deliver_ask(&mut self, tab: &str, text: &str) -> bool {
        if let Some(why) = self.ask_refusal(tab) {
            self.ask_note(why);
            return false;
        }
        let Some(h) = self.handle_of(tab) else {
            return false;
        };
        h.paste_unsent(text);
        self.focus_terminal();
        true
    }

    /// The panel header's "Ask agent": what is sent depends on what the panel shows.
    pub(super) fn panel_ask_agent(&mut self) {
        let Some(tab) = self.front_id() else { return };
        if let Some(why) = self.ask_refusal(&tab) {
            self.ask_note(why);
            return;
        }
        let Some(snap) = self.panels.snap(&tab).filter(|s| s.open) else {
            return;
        };
        // A markdown view's page: its selection, named by the file (with nothing selected, the file's path goes).
        if self.panels.is_md_page(&tab) {
            if !self.web.ask_selection(&tab) {
                self.ask_note(crate::ask_agent::NOTE_PAGE_FAILED);
            }
            return;
        }
        if let Some(path) = &snap.path {
            self.ask_about_path(&tab, path);
        } else if snap.kind == "web" || snap.kind == "app" || snap.kind.starts_with("mod:") {
            if !self.web.ask_selection(&tab) {
                self.ask_note(crate::ask_agent::NOTE_PAGE_FAILED);
            }
        } else {
            self.ask_note(crate::ask_agent::NOTE_NO_FILE);
        }
    }

    /// A text or image panel has no selection: its file's path is sent, quoted for the tab's shell.
    fn ask_about_path(&mut self, tab: &str, path: &str) {
        let shell = self.handle_of(tab).and_then(|h| h.shell_id());
        let quoted = quote_path(path, Some(path_shell_of(shell.as_deref())))
            .filter(|q| sanitize_paste(q) == *q);
        match quoted {
            Some(q) => {
                self.deliver_ask(tab, &q);
            }
            None => self.ask_note(crate::ask_agent::NOTE_PATH_REFUSED),
        }
    }

    /// The page answered with its selection. Dropped when the person has since moved to another tab: what they
    /// asked about is no longer in front of them, and nothing may be typed where they are not looking.
    pub(super) fn ask_selection_done(
        &mut self,
        tab: &str,
        label: &str,
        result: Result<String, String>,
    ) {
        use crate::ask_agent::{NOTE_NO_SELECTION, NOTE_PAGE_FAILED, frame_selection};
        if self.front_id().as_deref() != Some(tab) {
            return;
        }
        let Ok(text) = result else {
            self.ask_note(NOTE_PAGE_FAILED);
            return;
        };
        match frame_selection(label, &text) {
            Some(framed) => {
                self.deliver_ask(tab, &framed);
            }
            // Nothing selected on a markdown view's page: the file is what the panel shows, as in Code.
            None => match self.panels.snap(tab).and_then(|s| s.path) {
                Some(path) if self.panels.is_md_page(tab) => self.ask_about_path(tab, &path),
                _ => self.ask_note(NOTE_NO_SELECTION),
            },
        }
    }

    /// The terminal menu's row: the grid selection the menu was opened over goes to the tab's agent, and the
    /// selection is let go as a copy lets it go (with it still up, the next keys would act on it).
    pub(super) fn ask_about_grid(&mut self, tab: &str, selection: &str) {
        let Some(framed) = crate::ask_agent::frame_selection("the terminal", selection) else {
            self.ask_note(crate::ask_agent::NOTE_NO_SELECTION);
            return;
        };
        if self.deliver_ask(tab, &framed)
            && let Some(h) = self.handle_of(tab)
        {
            h.with_emu(|em| em.sel_clear());
        }
    }

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
        // A plugin's helper hears what its background page would (plugins phase 7): it has no slot, so its plugin is
        // listed with the pages it has, or none.
        if let Some(host) = m.helpers.as_ref() {
            for plugin in host.running() {
                let Some(Grant::Mod { permissions, .. }) = hub.grant_of_mod(&plugin) else {
                    continue;
                };
                if permissions.contains(&Permission::Sessions)
                    && (permissions.contains(&Permission::Tabs)
                        || hub.has_click_scope(&plugin, tab))
                {
                    out.entry(plugin).or_default();
                }
            }
        }
        out
    }

    /// Follows the transcript of every tab some page listens to, and no other ([`crate::plugin_sessions`]).
    pub(super) fn sync_session_followers(&mut self) {
        let listening = self
            .mods
            .as_ref()
            .is_some_and(|m| m.any_listens(&["session"]))
            || self.helpers_run();
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
                if self
                    .mods
                    .as_ref()
                    .and_then(|m| m.helpers.as_ref())
                    .is_some_and(|h| h.is_running(plugin))
                {
                    self.to_helper(plugin, "session", &data);
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
