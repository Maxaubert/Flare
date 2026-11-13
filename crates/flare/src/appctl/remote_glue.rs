//! Remote sessions PR 2 on the UI thread: what a remote viewer's `remote.*` call changes in this window. The control
//! channel's worker checked every param first (`ctl_app::AppDispatcher::remote_call`); here the tab must still be open.
//!
//! - `remote.open` opens a tab the way a person's new tab does, with a resume id that passed `valid_resume`.
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
            CtlOp::RemoteOpen { folder, resume } => {
                let id = self.model.open_tab(&folder);
                // Only an id that passed the check is a resume (the worker checked it, and the host checks it again).
                let resume = valid_resume(resume.as_deref()).map(str::to_string);
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
                let mut v = json!({
                    "tab": f.id,
                    "state": f.state,
                    "kind": f.agent,
                    "session": f.session,
                    "folder": f.folder,
                    "title": f.title,
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
            self.hub.on_remote_status(&id, &st);
        }
    }
}
