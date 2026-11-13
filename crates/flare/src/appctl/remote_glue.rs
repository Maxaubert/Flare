//! Remote sessions PR 2 on the UI thread: what a remote viewer's `remote.*` call changes in this window. The control
//! channel's worker checked every param first (`ctl_app::AppDispatcher::remote_call`); here the tab must still be open.
//!
//! - `remote.open` opens a tab the way a person's new tab does, with a resume id that passed `valid_resume`; the same
//!   `open_id` again is answered with the tab it opened (rule 1210).
//! - `remote.attach` puts the tab's handle and output tail into the `RemoteHost`, so the polls never come here.
//! - `remote.input` types through `TermHandle::remote_input`, the typed path (rule 279).
//! - `remote.resize` makes the remote viewer the size owner (rule 282); a release gives it back.
//! - `remote.close` closes as a person's close does: an agent at work asks first (rule 48).
//!
//! Nothing here ends a session that a viewer only stopped following (rule 280).

use std::sync::Arc;

use flare_agent::resume::valid_resume;
use flare_agent::reveal::agent_of_resume;
use flare_ctl::{CtlError, code};
use serde_json::{Value, json};

use super::AppCtl;
use crate::ctl_app::CtlOp;

/// A remote tab's fitted conversation view (remote sessions PR 7, rule 1193): what the host last said of each remote
/// tab, the tabs whose live terminal was taken back here, and what the window shows.
#[derive(Default)]
pub(crate) struct RemoteConvoView {
    /// Per remote tab: the host's window sizes it, and it has a Claude session there.
    host: std::collections::HashMap<String, (bool, bool)>,
    /// Tabs taken back here by a click or a key, until the host says this viewer sizes them.
    taken: std::collections::HashSet<String>,
    /// The tab and the conversation's number the window shows, None while the terminal shows.
    shown: Option<(String, u64)>,
}

impl RemoteConvoView {
    /// Pure: whether the view stands for this tab: the host sizes it, it has a session, and it was not taken back.
    pub(crate) fn wanted(&self, tab: &str) -> bool {
        self.host
            .get(tab)
            .is_some_and(|&(sized, session)| sized && session)
            && !self.taken.contains(tab)
    }

    /// The host said who sizes the tab: a status that gives it to this viewer ends a takeover.
    pub(crate) fn heard(&mut self, tab: &str, host_sized: bool, session: bool) {
        if !host_sized {
            self.taken.remove(tab);
        }
        self.host.insert(tab.to_string(), (host_sized, session));
    }

    pub(crate) fn forget(&mut self, tab: &str) {
        self.host.remove(tab);
        self.taken.remove(tab);
    }
}

impl AppCtl {
    /// The window hands over the remote host (startup, when the bridge key was written).
    pub fn set_remote_host(&mut self, host: Arc<crate::remote_host::RemoteHost>) {
        self.remote_host = Some(host);
    }

    /// The remote host, for the tests and the probe.
    #[cfg(test)]
    pub(crate) fn remote_host(&self) -> Option<Arc<crate::remote_host::RemoteHost>> {
        self.remote_host.clone()
    }

    /// One `remote.*` op on the UI thread.
    pub(super) fn ctl_remote(&mut self, op: CtlOp) -> Result<Value, CtlError> {
        let closed = || CtlError::new(code::NO_TAB, "that tab is closed");
        let not_running = || CtlError::new(code::UNAVAILABLE, "that tab's shell is not running");
        match op {
            CtlOp::RemoteOpen {
                folder,
                resume,
                open_id,
            } => {
                // Rule 1210: the same open sent again (its answer was lost to a broken link) gets the tab the first one
                // opened, while that tab is still open; never a second tab.
                if let (Some(oid), Some(host)) = (open_id.as_deref(), self.remote_host.as_ref())
                    && let Some(o) = host.opened(oid)
                    && self.is_shell_tab(&o.tab)
                {
                    return Ok(json!({"tab": o.tab, "session": o.session}));
                }
                let id = self.model.open_tab(&folder);
                // Only an id that passed the check is a resume (the worker checked it, and the host checks it again).
                let resume = valid_resume(resume.as_deref()).map(str::to_string);
                if let (Some(oid), Some(host)) = (open_id.as_deref(), self.remote_host.as_ref()) {
                    host.note_opened(
                        oid,
                        crate::remote_host::Opened {
                            tab: id.clone(),
                            session: resume.clone(),
                        },
                    );
                }
                self.deps.host.spawn(&id, &folder, resume.as_deref());
                self.note_spawn(&id, &folder, resume.is_some());
                self.hub.on_spawned(&id, &folder, resume.as_deref());
                let kind = resume.as_deref().map(agent_of_resume);
                if kind.is_some() {
                    self.model.set_agent(&id, kind);
                }
                self.refresh();
                Ok(json!({"tab": id, "session": resume}))
            }
            CtlOp::RemoteTabs => {
                let tabs: Vec<Value> = self
                    .tab_facts()
                    .into_iter()
                    .map(|f| {
                        json!({
                            "tab": f.id,
                            "cwd": f.folder,
                            "title": f.title,
                            "state": f.state,
                            "session": f.session,
                        })
                    })
                    .collect();
                Ok(json!({"tabs": tabs}))
            }
            CtlOp::RemoteAttach { tab, conn } => {
                if !self.is_shell_tab(&tab) {
                    return Err(closed());
                }
                let host = self.remote_host.clone().ok_or_else(|| {
                    CtlError::new(
                        code::UNAVAILABLE,
                        "this Flare does not serve remote sessions",
                    )
                })?;
                let handle = self.handle_of(&tab).ok_or_else(not_running)?;
                let tail = self
                    .deps
                    .host
                    .manager()
                    .subscribe_tail(&tab)
                    .ok_or_else(not_running)?;
                let seq = tail.end();
                let (cols, rows) = handle.size();
                host.follow(&tab, conn, handle, tail);
                // A tab that works as a viewer attaches holds the system awake from now (rule 296).
                self.sync_keep_awake();
                Ok(json!({"tab": tab, "cols": cols, "rows": rows, "seq": seq}))
            }
            CtlOp::RemoteInput { tab, intents } => {
                if !self.is_shell_tab(&tab) {
                    return Err(closed());
                }
                let handle = self.handle_of(&tab).ok_or_else(not_running)?;
                let typed = handle.remote_input(&intents);
                Ok(json!({"tab": tab, "typed": typed}))
            }
            CtlOp::RemoteResize { tab, cols, rows } => {
                if !self.is_shell_tab(&tab) {
                    return Err(closed());
                }
                if !self.deps.host.remote_resize(&tab, cols, rows) {
                    return Err(not_running());
                }
                Ok(json!({"tab": tab, "cols": cols, "rows": rows}))
            }
            CtlOp::RemoteRelease { tab } => {
                self.deps.host.take_size_back(&tab);
                // The last viewer went (a detach or a dropped link): its work no longer holds the system awake.
                self.sync_keep_awake();
                Ok(json!({"tab": tab}))
            }
            CtlOp::RemoteClose { tab } => {
                if !self.model.state.tabs.iter().any(|t| t.id == tab) {
                    return Err(closed());
                }
                self.request_close(&tab);
                // An agent at work asks first (rule 48): the tab is still there until the person answers.
                let closed_now = !self.model.state.tabs.iter().any(|t| t.id == tab);
                Ok(json!({"tab": tab, "closed": closed_now}))
            }
            CtlOp::RemoteStatus => Ok(json!({"tabs": self.remote_status_tabs()})),
            _ => Err(CtlError::new(
                code::INTERNAL,
                "not a remote op reached the remote handler",
            )),
        }
    }

    /// `remote.status` (remote sessions PR 4, rule 294): every shell tab in strip order as this window's own marks say
    /// it: one state word, the agent, its Claude session, the folder and title, how long it has worked, the words of
    /// its question and the kind of its failure. Words only; nothing here is a path to read or a mark to copy.
    pub(crate) fn remote_status_tabs(&self) -> Vec<Value> {
        use flare_ctl::remote::{QUESTION_MAX, TabState};
        self.tab_facts()
            .into_iter()
            .map(|f| {
                let id = f.id.as_str();
                // Who sizes the tab now (remote sessions PR 7): `host` once someone typed in this window, else the
                // viewer that sent its size (rule 282). The other screen shows its fitted conversation view.
                let sized = match self.deps.host.size_owner(id) {
                    crate::tab_host::SizeOwner::Local => "host",
                    crate::tab_host::SizeOwner::Remote => "viewer",
                };
                let mut v = json!({
                    "tab": f.id,
                    "state": f.state,
                    "kind": f.agent,
                    "session": f.session,
                    "folder": f.folder,
                    "title": f.title,
                    "sized": sized,
                });
                match TabState::parse(f.state) {
                    Some(TabState::Working) => {
                        if let Some(ms) = self.hub.working_for(id) {
                            v["working_for"] = json!(ms);
                        }
                    }
                    Some(TabState::Question) => {
                        if let Some(q) = self.hub.question_text(id) {
                            let q: String = q.chars().take(QUESTION_MAX).collect();
                            v["question_text"] = json!(q);
                        }
                    }
                    Some(TabState::Failed) => {
                        if let Some(k) = self.hub.failed_kind(id) {
                            v["failed"] = json!(k);
                        }
                    }
                    _ => {}
                }
                v
            })
            .collect()
    }

    /// Rule 296: this host holds the system awake while a tab a remote viewer follows has an agent at work, and lets go
    /// once none does, the last viewer goes, or the window ends (the hold is dropped with it).
    pub(crate) fn sync_keep_awake(&mut self) {
        let want = self.remote_host.as_ref().is_some_and(|h| {
            h.followed_tabs()
                .iter()
                .any(|t| self.hub.working(t) && self.hub.agent_here(t))
        });
        match (want, self.awake_hold.is_some()) {
            (true, false) => {
                self.awake_hold = self.keep_awake.as_ref().map(|k| k.hold());
                if self.awake_hold.is_some() {
                    crate::diag_ctl::crumb_often(
                        "keep-awake",
                        &[("held", crate::diag_ctl::txt("1"))],
                    );
                }
            }
            (false, true) => {
                self.awake_hold = None;
                crate::diag_ctl::crumb_often("keep-awake", &[("held", crate::diag_ctl::txt("0"))]);
            }
            _ => {}
        }
    }

    /// The keep-awake the host holds while a remote session works (startup; a test gives a fake).
    pub fn set_keep_awake(&mut self, k: Arc<dyn flare_win::keep_awake::KeepAwake>) {
        self.keep_awake = Some(k);
    }

    /// Whether this host holds the system awake now (tests and the probe).
    pub(crate) fn holds_awake(&self) -> bool {
        self.awake_hold.is_some()
    }

    /// Remote sessions PR 7: the machines as `machines.json` holds them now (read at start, or saved by Settings >
    /// Machines). The + menu lists them, and every remote tab takes its machine's name and system (its logo).
    pub(crate) fn set_machines(&mut self, list: Vec<crate::machines::Machine>) {
        self.machines = list;
        self.tag_remote_tabs();
        self.refresh();
    }

    /// The machines this window knows (the tests).
    #[cfg(test)]
    pub(crate) fn machines(&self) -> &[crate::machines::Machine] {
        &self.machines
    }

    /// A remote tab's tag for the machine `id`: its name and system when the machines name it, else its id alone.
    pub(crate) fn machine_tag(&self, id: &str) -> flare_tabs::tabs::MachineTag {
        self.machines.iter().find(|m| m.id == id).map_or_else(
            || flare_tabs::tabs::MachineTag::of_id(id),
            |m| flare_tabs::tabs::MachineTag {
                id: m.id.clone(),
                name: m.name.clone(),
                kind: m.kind,
            },
        )
    }

    /// Every remote tab takes its machine's name and system from the machines known now (a restored tab knew its id
    /// only). A machine no longer listed keeps the tag it had.
    pub(crate) fn tag_remote_tabs(&mut self) {
        let tags: Vec<(usize, flare_tabs::tabs::MachineTag)> = self
            .model
            .state
            .tabs
            .iter()
            .enumerate()
            .filter_map(|(i, t)| t.remote.as_ref().map(|r| (i, r.id.clone())))
            .filter(|(_, id)| self.machines.iter().any(|m| &m.id == id))
            .map(|(i, id)| (i, self.machine_tag(&id)))
            .collect();
        for (i, tag) in tags {
            if let Some(t) = self.model.state.tabs.get_mut(i) {
                t.remote = Some(tag);
            }
        }
    }

    /// "Hide here only" (remote sessions PR 7, rule 1192): the remote tab goes from this window and its session goes
    /// on on its machine (`remote.detach`, never `remote.close`), so nothing asks first. Its plain Close still ends it
    /// everywhere and asks as any close does (rule 48).
    pub(crate) fn hide_remote(&mut self, id: &str) {
        if self.model.remote_of(id).is_none() {
            return;
        }
        self.note_close(id);
        self.deps.host.leave(id);
        self.working.remove(id);
        self.model.close(id);
        self.term.tab_closed(id);
        self.forget_tab_surfaces(id);
        self.refresh();
    }

    /// Remote sessions PR 4 on the viewer (rule 294): a remote tab's link brought the host's status of it. Each remote
    /// tab's newest status is applied once; the hub turns it into marks (question and working, never a copied done).
    pub(crate) fn sync_remote_status(&mut self) {
        let news: Vec<(String, flare_term::remote_pty::HostStatus)> = self
            .model
            .state
            .tabs
            .iter()
            .filter(|t| t.remote.is_some())
            .filter_map(|t| {
                self.model
                    .remote_live(&t.id)
                    .and_then(flare_term::remote_pty::RemoteLive::take_status)
                    .map(|s| (t.id.clone(), s))
            })
            .collect();
        for (id, st) in news {
            self.rconvo.heard(&id, st.host_sized, st.session.is_some());
            self.hub.on_remote_status(&id, &st);
        }
    }

    /// Remote sessions PR 7 (rule 1193): each remote tab's link polls its conversation only while its view is wanted,
    /// and the window shows the view of the tab in front. True when what the window shows changed.
    pub(crate) fn sync_remote_convo(&mut self) -> bool {
        for t in &self.model.state.tabs {
            if t.remote.is_some()
                && let Some(live) = self.model.remote_live(&t.id)
            {
                live.want_convo(self.rconvo.wanted(&t.id));
            }
        }
        let before = self.rconvo.shown.clone();
        self.push_remote_convo();
        before != self.rconvo.shown
    }

    /// The tab in front, when its fitted view stands: it is remote, the host sizes it, and its conversation came.
    fn remote_convo_front(&self) -> Option<(String, flare_term::remote_pty::RemoteLive)> {
        let id = self.model.state.active_id.clone()?;
        self.model.remote_of(&id)?;
        let live = self.model.remote_live(&id)?.clone();
        (self.rconvo.wanted(&id) && live.convo_version() > 0).then_some((id, live))
    }

    /// Whether the fitted view is up now (the key path reads it).
    pub(crate) fn remote_convo_shown(&self) -> bool {
        self.rconvo.shown.is_some()
    }

    /// The window shows the fitted view of the tab in front, or the terminal. The lines are written again only when the
    /// conversation or the tab changed.
    pub(crate) fn push_remote_convo(&mut self) {
        let Some(app) = self.app() else { return };
        let front = self.remote_convo_front();
        let now = front
            .as_ref()
            .map(|(id, live)| (id.clone(), live.convo_version()));
        if now == self.rconvo.shown {
            return;
        }
        match front {
            Some((id, live)) => {
                let lines: Vec<crate::RemoteConvoLine> = live
                    .convo()
                    .1
                    .iter()
                    .map(|l| crate::RemoteConvoLine {
                        role: l.role.as_str().into(),
                        text: l.text.as_str().into(),
                        tool: l.tool.clone().unwrap_or_default().into(),
                    })
                    .collect();
                let machine = self
                    .model
                    .remote_of(&id)
                    .map(|m| m.name.clone())
                    .unwrap_or_default();
                app.set_rconvo_lines(slint::ModelRc::new(slint::VecModel::from(lines)));
                app.set_rconvo_machine(machine.into());
                app.set_rconvo_open(true);
            }
            None => app.set_rconvo_open(false),
        }
        self.rconvo.shown = now;
    }

    /// A click on the fitted view, or a key while it is up (rule 1193): the live terminal comes back on this screen and
    /// this window sizes it again (the link sends its size; rule 282, the last typist sets the size).
    pub(crate) fn remote_convo_take_over(&mut self) {
        let Some((id, _)) = self.rconvo.shown.clone() else {
            return;
        };
        if let Some(live) = self.model.remote_live(&id) {
            live.retake_size();
            live.want_convo(false);
        }
        self.rconvo.taken.insert(id);
        self.push_remote_convo();
        self.refresh();
    }
}
