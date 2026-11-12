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
            _ => Err(CtlError::new(
                code::INTERNAL,
                "not a remote op reached the remote handler",
            )),
        }
    }
}
