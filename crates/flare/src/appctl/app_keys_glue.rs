//! The app keys of rule 1236 on the UI thread: Ctrl+Shift+T reopens the tab closed last, Ctrl+Shift+N asks for a
//! folder, Ctrl+Shift+R lists the linked machines, Ctrl+Shift+L opens the browser panel with its address field holding
//! the keys, Ctrl+Shift+E shows the tab's folder in Explorer (Finder on a Mac). On a Mac they are Cmd+Shift.
//!
//! The closed tabs are kept in memory only, for this run: a short list, the last one last. A Claude tab comes back with
//! its own session when the hooks named it; otherwise a worker finds the newest session of the folder that no open tab
//! holds (rule 4: nothing is guessed beyond that). Codex and agy come back with their own resume (`codex resume
//! --last`, `agy -c`). A remote tab comes back on its machine, in its folder; the host owns its session.

use std::collections::HashSet;
use std::sync::Arc;

use flare_agent::kinds::{AGY_RESUME, AgentKind, CODEX_RESUME};
use flare_agent::resume::valid_resume;
use flare_agent::reveal::agent_of_resume;
use flare_tabs::tabs::TabKind;

use super::{AppCtl, BAR_H, UiMsg};
use crate::menus_ui::{MenuKind, machine_menu_rows, machine_of_row};

/// How many closed tabs Ctrl+Shift+T can walk back through.
pub(crate) const CLOSED_KEPT: usize = 10;

/// The left edge of the machine menu Ctrl+Shift+R opens, under the bar.
const MACHINE_MENU_X: f32 = 12.0;

/// A tab closed this run, as Ctrl+Shift+T needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClosedTab {
    /// Its folder (the host's folder for a remote tab).
    pub cwd: String,
    /// Its place in the strip when it closed.
    pub at: usize,
    /// The resumable agent it hosted (never for a remote tab: the host owns that session).
    pub agent: Option<AgentKind>,
    /// The Claude session the hooks named for it, checked by `valid_resume`.
    pub session: Option<String>,
    /// The machine's id, for a remote tab.
    pub machine: Option<String>,
}

/// A worker's answer for a closed Claude tab whose session was not known (rule 1236).
#[derive(Debug)]
pub(crate) struct Reopen {
    pub cwd: String,
    pub at: usize,
    pub resume: Option<String>,
}

/// Keeps `tab` as the newest closed tab; the oldest goes past [`CLOSED_KEPT`].
pub(crate) fn push_closed(list: &mut Vec<ClosedTab>, tab: ClosedTab) {
    list.push(tab);
    if list.len() > CLOSED_KEPT {
        let extra = list.len() - CLOSED_KEPT;
        list.drain(..extra);
    }
}

/// What a closed tab starts with: Codex and agy their own resume, Claude its known session. `Err` for a Claude tab
/// with no session known: a worker looks for one.
fn resume_of(t: &ClosedTab) -> Result<Option<String>, ()> {
    match t.agent {
        Some(AgentKind::Codex) => Ok(Some(CODEX_RESUME.to_string())),
        Some(AgentKind::Agy) => Ok(Some(AGY_RESUME.to_string())),
        Some(AgentKind::Claude) => match &t.session {
            Some(s) => Ok(Some(s.clone())),
            None => Err(()),
        },
        None => Ok(None),
    }
}

impl AppCtl {
    /// The closed tabs Ctrl+Shift+T can still open, the newest last (the tests read it).
    pub(crate) fn closed_tabs(&self) -> &[ClosedTab] {
        &self.closed
    }

    /// Keeps what Ctrl+Shift+T needs of a shell tab about to close. Settings and a placeholder are not kept.
    pub(super) fn remember_closed(&mut self, id: &str) {
        let Some(at) = self.model.state.tabs.iter().position(|t| t.id == id) else {
            return;
        };
        let tab = &self.model.state.tabs[at];
        if tab.kind != TabKind::Shell || tab.pending.is_some() || tab.cwd.is_empty() {
            return;
        }
        let machine = tab.remote.as_ref().map(|m| m.id.clone());
        let cwd = tab.cwd.clone();
        let agent = if machine.is_some() {
            None
        } else {
            self.hub
                .saved_agent(id)
                .or_else(|| self.model.agent_kind(id))
        };
        let session = (agent == Some(AgentKind::Claude))
            .then(|| self.hub.claude_session(id))
            .flatten()
            .and_then(|s| valid_resume(Some(s)))
            .map(str::to_string);
        push_closed(
            &mut self.closed,
            ClosedTab {
                cwd,
                at,
                agent,
                session,
                machine,
            },
        );
    }

    /// Ctrl+Shift+T: the newest closed tab opens again in its place. Nothing when no tab was closed.
    pub(crate) fn reopen_closed_tab(&mut self) {
        let Some(t) = self.closed.pop() else {
            return;
        };
        self.close_menu_quietly();
        if let Some(machine) = &t.machine {
            let id = self.open_remote(machine, &t.cwd, None, true);
            self.put_back(&id, t.at);
            self.focus_terminal();
            return;
        }
        // Two Claudes on one session write one file: a session an open tab took since is not resumed again (rule 4),
        // the tab looks for one no open tab holds instead.
        let held = self.held_sessions();
        match resume_of(&t) {
            Ok(Some(s)) if t.agent == Some(AgentKind::Claude) && held.contains(&s) => {
                self.find_session_then_reopen(t, held);
            }
            Ok(resume) => {
                self.open_resuming(&t.cwd, resume.as_deref(), t.at);
            }
            Err(()) => self.find_session_then_reopen(t, held),
        }
    }

    /// The Claude sessions the open tabs hold.
    fn held_sessions(&self) -> HashSet<String> {
        self.model
            .state
            .tabs
            .iter()
            .filter_map(|o| self.hub.claude_session(&o.id).map(str::to_string))
            .collect()
    }

    /// A worker reads the folder's Claude sessions (newest first) and the tab opens with the newest no open tab
    /// holds (`held`), or as a plain shell when there is none.
    fn find_session_then_reopen(&mut self, t: ClosedTab, held: HashSet<String>) {
        let lookup = Arc::clone(&self.deps.lookup);
        let post = Arc::clone(&self.deps.post);
        let (cwd, at) = (t.cwd.clone(), t.at);
        let spawned = std::thread::Builder::new()
            .name("flare-reopen".into())
            .spawn(move || {
                let resume = lookup
                    .sessions(&cwd)
                    .into_iter()
                    .find(|s| !held.contains(s));
                post(UiMsg::Reopen(Reopen { cwd, at, resume }));
            });
        if let Err(e) = spawned {
            log::warn!("could not start the reopen worker: {e}");
            self.open_resuming(&t.cwd, None, t.at);
        }
    }

    /// The worker's answer: the tab opens, resuming what it found.
    pub(super) fn reopened(&mut self, r: Reopen) {
        self.open_resuming(&r.cwd, r.resume.as_deref(), r.at);
    }

    /// A local tab in `dir` at strip place `at`, its shell starting the agent `resume` names (as the restore does).
    fn open_resuming(&mut self, dir: &str, resume: Option<&str>, at: usize) -> String {
        // Only an id that passed the check is a resume; the rest is a plain shell.
        let resume = valid_resume(resume);
        let id = self.model.open_tab(dir);
        self.deps.host.spawn(&id, dir, resume);
        self.note_spawn(&id, dir, resume.is_some());
        self.hub.on_spawned(&id, dir, resume);
        if let Some(k) = resume.map(agent_of_resume) {
            self.model.set_agent(&id, Some(k));
            let now = self.now();
            self.restoring.insert(id.clone(), (k, now));
        }
        self.put_back(&id, at);
        self.focus_terminal();
        id
    }

    /// The reopened tab goes back to the place it had, when the strip still reaches it.
    fn put_back(&mut self, id: &str, at: usize) {
        if at < self.model.state.tabs.len() {
            self.model.reorder(id, at);
        }
        self.refresh();
    }

    /// Ctrl+Shift+R: the linked machines as a menu under the bar; a pick opens a shell tab there. With none linked
    /// the menu says so.
    pub(crate) fn open_machine_menu(&mut self) {
        self.close_overflow_quietly();
        let rows = machine_menu_rows(&self.machines);
        self.show_menu(MenuKind::Machines, rows, MACHINE_MENU_X, BAR_H);
    }

    /// A row of the machine menu: a shell tab on that machine, in the host's home folder (as the + menu's row).
    pub(super) fn machine_menu_picked(&mut self, path: &str) {
        if let Some(machine) = machine_of_row(path) {
            let machine = machine.to_string();
            self.open_remote(&machine, crate::machines::HOST_HOME, None, true);
            self.focus_terminal();
        }
    }

    /// Ctrl+Shift+L: the browser panel beside the tab in front, with the address field holding the keys. A page the
    /// tab shows (or hid) stays; otherwise an empty page opens.
    pub(crate) fn open_browser_panel(&mut self) {
        let Some(tab) = self.model.active_shell().map(|t| t.id.clone()) else {
            return;
        };
        self.close_menu_quietly();
        // A plain web page (not a mini-app or a mod's panel), shown or hidden.
        let page = self
            .panels
            .snap(&tab)
            .filter(|s| s.url.is_some() && !self.panels.is_app(&tab));
        if let Some(s) = page {
            if !s.open && self.panels.reopen(&tab) {
                self.refresh();
                self.panel_changed(&tab);
            }
        } else if let Err(e) = self.browser_open(&tab, "about:blank") {
            log::info!("the browser panel did not open: {}", e.message);
            return;
        }
        self.focus_address();
    }

    /// The panel's address field takes the keys, its address selected; a page holding the keyboard lets it go first.
    fn focus_address(&mut self) {
        let Some(app) = self.app() else { return };
        let hwnd = crate::window::hwnd_of(&app);
        self.web.release_focus(hwnd);
        app.invoke_focus_address();
    }

    /// Ctrl+Shift+E: the front tab's folder in Explorer (Finder on a Mac), on a worker: the process start can wait on
    /// a slow disk. A remote tab's folder is on another machine: nothing opens.
    pub(crate) fn open_front_folder(&mut self) {
        let Some(tab) = self.model.active_shell() else {
            return;
        };
        if tab.remote.is_some() || tab.cwd.is_empty() {
            return;
        }
        let cwd = tab.cwd.clone();
        let openers = Arc::clone(&self.deps.term.openers);
        let spawned = std::thread::Builder::new()
            .name("flare-open-folder".into())
            .spawn(move || {
                openers.open_folder(&cwd);
            });
        if let Err(e) = spawned {
            log::warn!("could not start the folder opener: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn closed(cwd: &str) -> ClosedTab {
        ClosedTab {
            cwd: cwd.to_string(),
            at: 0,
            agent: None,
            session: None,
            machine: None,
        }
    }

    #[test]
    fn the_list_keeps_the_newest_ten_the_last_one_last() {
        let mut list = Vec::new();
        for i in 0..13 {
            push_closed(&mut list, closed(&format!("C:\\{i}")));
        }
        assert_eq!(list.len(), CLOSED_KEPT);
        assert_eq!(list.first().unwrap().cwd, "C:\\3");
        assert_eq!(list.last().unwrap().cwd, "C:\\12");
    }

    #[test]
    fn each_agent_comes_back_with_its_own_resume() {
        let mut t = closed("C:\\a");
        assert_eq!(resume_of(&t), Ok(None));
        t.agent = Some(AgentKind::Codex);
        assert_eq!(resume_of(&t), Ok(Some(CODEX_RESUME.to_string())));
        t.agent = Some(AgentKind::Agy);
        assert_eq!(resume_of(&t), Ok(Some(AGY_RESUME.to_string())));
        t.agent = Some(AgentKind::Claude);
        assert_eq!(resume_of(&t), Err(()), "no session known: a worker looks");
        t.session = Some("3f2a9c1e-77aa-4c0d-9b1e-0a1b2c3d4e5f".into());
        assert_eq!(
            resume_of(&t),
            Ok(Some("3f2a9c1e-77aa-4c0d-9b1e-0a1b2c3d4e5f".to_string()))
        );
    }
}
