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

/// How often the host reads the conversation again while its own fitted view stands (the viewer polls as often).
pub(crate) const HOST_CONVO_EVERY_MS: u64 = 1500;

/// Pure (rule 1211): whether the host's own window shows the fitted conversation view of its tab in front instead of
/// the terminal. Only while a remote viewer sizes the tab and typed there since it took the size (the last typist is
/// on the viewer's screen, rule 1212; a size alone is not typing), only for a tab of this host (a remote tab has the
/// viewer's view, rule 1193), and only for an agent tab: one whose Claude session is known. A plain shell keeps its
/// terminal at the viewer's size.
pub(crate) fn host_view_stands(
    viewer_typed_last: bool,
    remote_tab: bool,
    has_session: bool,
) -> bool {
    viewer_typed_last && !remote_tab && has_session
}

/// What a worker read of a tab's conversation for the host's own fitted view: the session it read, and the answer as
/// `remote.convo` gives it (None: the transcript could not be read). Prints its kind only, never the words.
pub struct HostConvoRead {
    pub tab: String,
    pub session: String,
    pub answer: Option<Value>,
}

impl std::fmt::Debug for HostConvoRead {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HostConvoRead")
    }
}

/// The conversation the host read for one tab, as its fitted view shows it.
struct HostMirror {
    tab: String,
    session: String,
    mirror: crate::remote_convo::ConvoMirror,
    /// Counts up with each change (0: nothing to show yet).
    version: u64,
}

/// The host's own fitted view (rule 1211): while a remote viewer sizes a tab of this host and that tab is in front,
/// this window shows the tab's conversation the way the viewer's window shows a remote one (rule 1193), read here on a
/// worker with the same numbered feed `remote.convo` uses (rule 297), every [`HOST_CONVO_EVERY_MS`].
#[derive(Default)]
pub(crate) struct HostConvoView {
    /// Per tab a remote viewer sized: the viewer's machine, as its link named it (empty when it did not).
    viewers: std::collections::HashMap<String, String>,
    /// The front tab's conversation as last read, while its view stands or until another tab's is read.
    read: Option<HostMirror>,
    feeds: Arc<crate::remote_convo::ConvoFeeds>,
    /// A read is on a worker.
    reading: bool,
    /// When the next read is due (the window clock), None: at once.
    due: Option<u64>,
}

impl HostConvoView {
    /// A viewer's `remote.resize` named its machine (or not).
    pub(crate) fn sized_by(&mut self, tab: &str, viewer: Option<&str>) {
        self.viewers
            .insert(tab.to_string(), viewer.unwrap_or_default().to_string());
    }

    /// The viewer machine the view names for `tab` (empty: none named).
    pub(crate) fn viewer(&self, tab: &str) -> &str {
        self.viewers.get(tab).map_or("", String::as_str)
    }

    /// Pure: the conversation to show for `tab` in `session`, and its version: None until a read brought words.
    pub(crate) fn shown_for(
        &self,
        tab: &str,
        session: &str,
    ) -> Option<(u64, &[(u64, flare_convo::said::Entry)])> {
        self.read
            .as_ref()
            .filter(|r| r.tab == tab && r.session == session && r.version > 0)
            .map(|r| (r.version, r.mirror.entries()))
    }

    /// The number to ask the feed from for `tab` in `session` (0: what is held is another tab's or session's).
    pub(crate) fn since(&self, tab: &str, session: &str) -> u64 {
        self.read
            .as_ref()
            .filter(|r| r.tab == tab && r.session == session)
            .map_or(0, |r| r.mirror.since())
    }

    /// Takes a worker's read: a read of another tab or session than the one held starts over. True when what the view
    /// would show changed.
    pub(crate) fn take(&mut self, r: &HostConvoRead) -> bool {
        self.reading = false;
        let Some(answer) = &r.answer else {
            return false;
        };
        let same = self
            .read
            .as_ref()
            .is_some_and(|m| m.tab == r.tab && m.session == r.session);
        if !same {
            self.read = Some(HostMirror {
                tab: r.tab.clone(),
                session: r.session.clone(),
                mirror: crate::remote_convo::ConvoMirror::new(),
                version: 0,
            });
        }
        let Some(m) = self.read.as_mut() else {
            return false;
        };
        if m.mirror.apply(answer) {
            m.version += 1;
            return true;
        }
        false
    }

    /// The tab closed: its viewer, its words and its feed go.
    pub(crate) fn forget(&mut self, tab: &str) {
        self.viewers.remove(tab);
        if self.read.as_ref().is_some_and(|r| r.tab == tab) {
            self.read = None;
        }
        self.feeds.forget(tab);
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
                // Rule 1212: the viewer that sizes the tab typed last, so an agent tab in front shows its fitted view here.
                self.deps.host.note_viewer_typed(&tab);
                self.push_remote_convo();
                self.kick();
                Ok(json!({"tab": tab, "typed": typed}))
            }
            CtlOp::RemoteResize {
                tab,
                cols,
                rows,
                viewer,
            } => {
                if !self.is_shell_tab(&tab) {
                    return Err(closed());
                }
                if !self.deps.host.remote_resize(&tab, cols, rows) {
                    return Err(not_running());
                }
                // Rule 1211: the view names this viewer. It stands only once the viewer typed (rule 1212): an attach, a
                // reconnect, a refit or a tab switch over there sends a size too, and none of those is typing.
                self.hconvo.sized_by(&tab, viewer.as_deref());
                self.push_remote_convo();
                self.kick();
                Ok(json!({"tab": tab, "cols": cols, "rows": rows}))
            }
            CtlOp::RemoteRelease { tab } => {
                self.deps.host.take_size_back(&tab);
                // The last viewer went (a detach or a dropped link): its work no longer holds the system awake.
                self.sync_keep_awake();
                // The size is this window's again: the terminal, not the fitted view (rule 1211).
                self.push_remote_convo();
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

    /// Rule 1211: the tab in front and its Claude session, when the host's own fitted view stands for it: a tab of this
    /// host that a remote viewer sizes and typed in last (rule 1212), with a session.
    fn host_convo_want(&self) -> Option<(String, String)> {
        let id = self.model.state.active_id.clone()?;
        if !self.is_shell_tab(&id) {
            return None;
        }
        let session = self.hub.claude_session(&id);
        host_view_stands(
            self.deps.host.viewer_typed_last(&id),
            self.model.remote_of(&id).is_some(),
            session.is_some(),
        )
        .then(|| (id, session.unwrap_or_default().to_string()))
    }

    /// Whether the fitted view is up now (the key path reads it).
    pub(crate) fn remote_convo_shown(&self) -> bool {
        self.rconvo.shown.is_some()
    }

    /// The window shows the fitted view of the tab in front (a remote tab's, rule 1193, or this host's own, rule 1211),
    /// or the terminal. The lines are written again only when the conversation or the tab changed.
    pub(crate) fn push_remote_convo(&mut self) {
        let Some(app) = self.app() else { return };
        let remote = self.remote_convo_front();
        let host = self.host_convo_want();
        let now = match (&remote, &host) {
            (Some((id, live)), _) => Some((id.clone(), live.convo_version())),
            (None, Some((id, session))) => self
                .hconvo
                .shown_for(id, session)
                .map(|(v, _)| (id.clone(), v)),
            (None, None) => None,
        };
        if now == self.rconvo.shown {
            return;
        }
        let line = |role: &str, text: &str, tool: Option<&str>| crate::RemoteConvoLine {
            role: role.into(),
            text: text.into(),
            tool: tool.unwrap_or_default().into(),
        };
        let shown: Option<(Vec<crate::RemoteConvoLine>, String)> = match (remote, host) {
            (Some((id, live)), _) => Some((
                live.convo()
                    .1
                    .iter()
                    .map(|l| line(&l.role, &l.text, l.tool.as_deref()))
                    .collect(),
                self.model
                    .remote_of(&id)
                    .map(|m| m.name.clone())
                    .unwrap_or_default(),
            )),
            (None, Some((id, session))) => self.hconvo.shown_for(&id, &session).map(|(_, e)| {
                let mirror_lines = crate::remote_link::entry_lines(e);
                (
                    mirror_lines
                        .iter()
                        .map(|l| line(&l.role, &l.text, l.tool.as_deref()))
                        .collect(),
                    // The live terminal is on the viewer's screen: the line names that machine.
                    self.hconvo.viewer(&id).to_string(),
                )
            }),
            (None, None) => None,
        };
        match shown {
            Some((lines, machine)) => {
                app.set_rconvo_lines(slint::ModelRc::new(slint::VecModel::from(lines)));
                app.set_rconvo_machine(machine.into());
                app.set_rconvo_open(true);
            }
            None => app.set_rconvo_open(false),
        }
        self.rconvo.shown = now;
    }

    /// A click on the fitted view, or a key while it is up (rule 1193): the live terminal comes back on this screen and
    /// this window sizes it again (rule 282, the last typist sets the size). On a remote tab the link sends this
    /// window's size; on a tab of this host (rule 1211) the tab takes this window's size back from the remote viewer,
    /// whose window then shows its own fitted view.
    pub(crate) fn remote_convo_take_over(&mut self) {
        let Some((id, _)) = self.rconvo.shown.clone() else {
            return;
        };
        if self.model.remote_of(&id).is_some() {
            if let Some(live) = self.model.remote_live(&id) {
                live.retake_size();
                live.want_convo(false);
            }
            self.rconvo.taken.insert(id);
        } else {
            self.deps.host.take_size_back(&id);
        }
        self.push_remote_convo();
        self.refresh();
    }

    /// Rule 1211: when the host's own fitted view next wants its conversation read (the window clock), while it stands.
    pub(crate) fn host_convo_due(&self) -> Option<u64> {
        self.host_convo_want()?;
        if self.hconvo.reading {
            return None;
        }
        Some(self.hconvo.due.unwrap_or_else(|| self.now()))
    }

    /// The timer's turn (rule 1211): while the host's own fitted view stands, its conversation is read again on a
    /// worker every [`HOST_CONVO_EVERY_MS`], one read at a time. The transcript is found and read there, never here.
    pub(crate) fn host_convo_tick(&mut self, now: u64) {
        let Some((tab, session)) = self.host_convo_want() else {
            self.hconvo.due = None;
            return;
        };
        if self.hconvo.reading || self.hconvo.due.is_some_and(|d| now < d) {
            return;
        }
        let Some(dir) = self.convo.claude_dir() else {
            return;
        };
        let Some(folder) = self
            .shell_tabs()
            .into_iter()
            .find(|(id, _)| id == &tab)
            .map(|(_, f)| f)
        else {
            return;
        };
        self.hconvo.due = Some(now + HOST_CONVO_EVERY_MS);
        self.hconvo.reading = true;
        let since = self.hconvo.since(&tab, &session);
        let feeds = Arc::clone(&self.hconvo.feeds);
        let post = Arc::clone(&self.deps.post);
        let spawned = std::thread::Builder::new()
            .name("flare-host-convo".into())
            .spawn(move || {
                let answer = crate::remote_convo::transcript_of(&dir, &folder, &session)
                    .and_then(|path| feeds.read(&tab, &session, &path, since).ok())
                    .map(|a| a.to_json(&tab));
                post(super::UiMsg::HostConvo(HostConvoRead {
                    tab,
                    session,
                    answer,
                }));
            });
        if let Err(e) = spawned {
            self.hconvo.reading = false;
            log::warn!("could not start the conversation read: {e}");
        }
    }

    /// A worker's read for the host's own fitted view (rule 1211): taken, and the window shows it while it stands.
    pub(crate) fn on_host_convo(&mut self, read: HostConvoRead) {
        if self.hconvo.take(&read) {
            self.push_remote_convo();
        }
        self.kick();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(tab: &str, session: &str, answer: Value) -> HostConvoRead {
        HostConvoRead {
            tab: tab.into(),
            session: session.into(),
            answer: Some(answer),
        }
    }

    fn said(seq: u64, text: &str) -> Value {
        json!({"seq": seq, "role": "assistant", "text": text})
    }

    fn texts(v: &HostConvoView, tab: &str, session: &str) -> Vec<String> {
        v.shown_for(tab, session)
            .map(|(_, e)| e.iter().map(|(_, e)| e.text.clone()).collect())
            .unwrap_or_default()
    }

    /// Rule 1211: the host's own window shows the fitted view only for a tab of its own that a remote viewer sizes and
    /// typed in last (rule 1212) and that has a Claude session; its own sizing or typing, a remote tab or a plain shell
    /// keeps the terminal.
    #[test]
    fn the_host_view_stands_only_for_an_agent_tab_of_this_host_a_viewer_sizes() {
        assert!(host_view_stands(true, false, true));
        assert!(
            !host_view_stands(false, false, true),
            "this window sizes it or typed last"
        );
        assert!(!host_view_stands(true, false, false), "a plain shell");
        assert!(
            !host_view_stands(true, true, true),
            "a remote tab has the viewer's view"
        );
        assert!(!host_view_stands(false, true, false));
    }

    /// The view shows nothing until a read brought words; each entry is taken once by its number, and the version
    /// counts each change.
    #[test]
    fn the_host_view_shows_once_words_came_and_takes_each_entry_once() {
        let mut v = HostConvoView::default();
        assert_eq!(v.shown_for("t1", "s"), None);
        assert!(!v.take(&read(
            "t1",
            "s",
            json!({"entries": [], "reset": false, "to": 0})
        )));
        assert_eq!(
            v.shown_for("t1", "s"),
            None,
            "an empty transcript shows nothing yet"
        );
        assert!(v.take(&read(
            "t1",
            "s",
            json!({"entries": [said(1, "one"), said(2, "two")], "reset": false})
        )));
        assert_eq!(texts(&v, "t1", "s"), ["one", "two"]);
        assert_eq!(v.since("t1", "s"), 2);
        let first = v.shown_for("t1", "s").map(|(n, _)| n);
        assert!(!v.take(&read(
            "t1",
            "s",
            json!({"entries": [said(2, "two")], "reset": false})
        )));
        assert_eq!(v.shown_for("t1", "s").map(|(n, _)| n), first, "nothing new");
        assert!(v.take(&read(
            "t1",
            "s",
            json!({"entries": [said(3, "three")], "reset": false})
        )));
        assert_eq!(texts(&v, "t1", "s"), ["one", "two", "three"]);
        // A read that failed changes nothing.
        assert!(!v.take(&HostConvoRead {
            tab: "t1".into(),
            session: "s".into(),
            answer: None,
        }));
        assert_eq!(texts(&v, "t1", "s").len(), 3);
    }

    /// Another session in the tab, or another tab, starts over: what was held is not shown for it, and it is asked
    /// from nothing.
    #[test]
    fn another_session_or_tab_starts_the_host_view_over() {
        let mut v = HostConvoView::default();
        v.take(&read(
            "t1",
            "s",
            json!({"entries": [said(1, "old")], "reset": false}),
        ));
        assert_eq!(v.since("t1", "s2"), 0);
        assert_eq!(v.shown_for("t1", "s2"), None);
        assert_eq!(v.since("t2", "s"), 0);
        v.take(&read(
            "t1",
            "s2",
            json!({"entries": [said(2, "new")], "reset": true}),
        ));
        assert_eq!(texts(&v, "t1", "s2"), ["new"]);
        assert_eq!(v.shown_for("t1", "s"), None);
        // The tab closed: its words and its viewer go.
        v.sized_by("t1", Some("Mac Mini"));
        assert_eq!(v.viewer("t1"), "Mac Mini");
        v.forget("t1");
        assert_eq!(v.viewer("t1"), "");
        assert_eq!(v.shown_for("t1", "s2"), None);
    }

    /// The viewer's machine as its resize named it; a bridge that named none leaves the line without a machine.
    #[test]
    fn the_host_view_names_the_viewer_that_sized_the_tab() {
        let mut v = HostConvoView::default();
        v.sized_by("t1", Some("Mac Mini"));
        assert_eq!(v.viewer("t1"), "Mac Mini");
        v.sized_by("t1", None);
        assert_eq!(v.viewer("t1"), "");
        assert_eq!(v.viewer("t2"), "");
    }
}
