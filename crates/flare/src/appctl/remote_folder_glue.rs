//! The remote folder browser on the viewer's UI thread (rule 1282; Max picked mockup B on 2026-11-17).
//!
//! A machine's row in the + menu, or in Ctrl+Shift+R's menu, opens the dialog at the host's home; each step (Back,
//! Home, a crumb, a double-click) asks a worker for one listing (`remote_folder::answer`, one `remote.ls` over a short
//! link) and the answer comes back as `UiMsg::RemoteFolder`. Only the newest ask's answer is taken, and none once the
//! dialog closed. Open here opens the remote tab in the picked folder (else the folder shown, else the host's home as
//! before), which joins the remote recents (rule 1213, `start_remote`). Nothing here waits (rule 34).

use std::rc::Rc;
use std::sync::Arc;

use serde_json::json;
use slint::{ModelRc, VecModel};

use super::{AppCtl, UiMsg};
use crate::remote_folder::{Ask, Browser, FolderLister, FolderMsg};
use crate::{RemoteCrumb, RemoteFolderRow};

impl AppCtl {
    /// What lists a host's folder: the machines' links in the window (ssh, or the e2e's loopback), a fake in a test.
    pub fn set_folder_lister(&mut self, lister: Arc<dyn FolderLister>) {
        self.rfolder_lister = lister;
    }

    /// How long the dialog waits for one listing before it says the machine cannot be reached (rule 1373):
    /// `remote_folder::LIST_LIMIT`, shorter in a test.
    #[cfg(test)]
    pub fn set_folder_limit(&mut self, limit: std::time::Duration) {
        self.rfolder_limit = limit;
    }

    /// Opens the folder browser on `machine` (its id), at the host's home.
    pub(crate) fn open_remote_folder(&mut self, machine: &str) {
        let Some(m) = self.machines.iter().find(|m| m.id == machine).cloned() else {
            return;
        };
        self.close_menu_quietly();
        self.close_overflow_quietly();
        self.clear_tip();
        let (b, ask) = Browser::new(&m.id, &m.name, m.kind, self.rfolder_seq + 1);
        self.rfolder = Some(b);
        if let Some(app) = self.app() {
            // The dialog has the keys: a page that holds them gives them back (Escape must reach the dialog).
            self.web.release_focus(crate::window::hwnd_of(&app));
        }
        self.rfolder_ask(ask);
    }

    /// Hands one ask to a worker; its answer comes back as `UiMsg::RemoteFolder`.
    fn rfolder_ask(&mut self, ask: Ask) {
        self.rfolder_seq = ask.seq;
        self.push_rfolder();
        let Some(b) = &self.rfolder else { return };
        let (lister, post) = (
            Arc::clone(&self.rfolder_lister),
            Arc::clone(&self.deps.post),
        );
        let (machine, name, limit) = (b.machine.clone(), b.name.clone(), self.rfolder_limit);
        // Rule 1373: the dialog hears within `limit` whatever the link does; a late listing still comes.
        let r = std::thread::Builder::new()
            .name("flare-remote-ls".into())
            .spawn(move || {
                crate::remote_folder::answer_in_time(lister, &machine, &name, &ask, limit, &|m| {
                    post(UiMsg::RemoteFolder(m));
                });
            });
        if let Err(e) = r {
            log::warn!("could not start the folder list worker: {e}");
            if let Some(b) = &mut self.rfolder {
                b.answered(FolderMsg {
                    seq: b.seq(),
                    answer: Err("Flare could not ask the machine.".into()),
                });
            }
            self.push_rfolder();
        }
    }

    /// A worker's answer: taken only by the open dialog, for its newest ask.
    pub(super) fn rfolder_answered(&mut self, m: FolderMsg) {
        if self.rfolder.as_mut().is_some_and(|b| b.answered(m)) {
            self.push_rfolder();
        }
    }

    /// Puts the dialog's state on the window.
    fn push_rfolder(&self) {
        let Some(app) = self.app() else { return };
        let Some(b) = &self.rfolder else {
            app.set_rfolder_open(false);
            return;
        };
        use crate::remote_folder::CrumbGo;
        let crumbs: Vec<RemoteCrumb> = b
            .crumbs()
            .into_iter()
            .map(|c| RemoteCrumb {
                label: c.label.into(),
                live: c.go != CrumbGo::Nothing && !c.last,
                last: c.last,
            })
            .collect();
        let rows: Vec<RemoteFolderRow> = b
            .entries()
            .iter()
            .map(|e| RemoteFolderRow {
                name: e.name.as_str().into(),
                dir: e.dir,
            })
            .collect();
        app.set_rfolder_title(crate::remote_folder::title(&b.name).into());
        app.set_rfolder_crumbs(ModelRc::new(Rc::new(VecModel::from(crumbs))));
        app.set_rfolder_rows(ModelRc::new(Rc::new(VecModel::from(rows))));
        app.set_rfolder_selected(b.selected().map_or(-1, |i| i as i32));
        app.set_rfolder_path(b.footer().into());
        app.set_rfolder_loading(b.loading());
        app.set_rfolder_error(b.error().unwrap_or("").into());
        app.set_rfolder_truncated(b.truncated());
        app.set_rfolder_can_back(b.can_back());
        app.set_rfolder_open(true);
    }

    /// Runs a step of the open dialog that may ask for a listing.
    fn rfolder_step_with(&mut self, f: impl FnOnce(&mut Browser) -> Option<Ask>) {
        let Some(b) = &mut self.rfolder else { return };
        match f(b) {
            Some(ask) => self.rfolder_ask(ask),
            None => self.push_rfolder(),
        }
    }

    pub(super) fn rfolder_back(&mut self) {
        self.rfolder_step_with(Browser::go_back);
    }

    pub(super) fn rfolder_home(&mut self) {
        self.rfolder_step_with(Browser::home);
    }

    /// Try again (rule 1373): the failed folder asked once more.
    pub(super) fn rfolder_retry(&mut self) {
        self.rfolder_step_with(Browser::retry);
    }

    pub(super) fn rfolder_crumb(&mut self, i: i32) {
        let Ok(i) = usize::try_from(i) else { return };
        self.rfolder_step_with(|b| b.crumb(i));
    }

    /// Puts only the pick on the window: the picked row and the footer's path. The rows and the crumbs stay the same
    /// models (rule 1299): a new rows model builds every row again, which scrolls the list back to its top and makes the
    /// second press of a double-click land on a new row, so Slint never sees a double-click and nothing is entered.
    fn push_rfolder_pick(&self) {
        let (Some(app), Some(b)) = (self.app(), &self.rfolder) else {
            return;
        };
        app.set_rfolder_selected(b.selected().map_or(-1, |i| i as i32));
        app.set_rfolder_path(b.footer().into());
    }

    /// A click on a row (rule 1299): it picks a folder and nothing else; a double-click or Enter does the rest.
    pub(super) fn rfolder_pick(&mut self, i: i32) {
        let Ok(i) = usize::try_from(i) else { return };
        let Some(b) = &mut self.rfolder else { return };
        b.select(i);
        self.push_rfolder_pick();
    }

    pub(super) fn rfolder_enter(&mut self, i: i32) {
        let Ok(i) = usize::try_from(i) else { return };
        self.rfolder_step_with(|b| b.enter(i));
    }

    pub(super) fn rfolder_step(&mut self, down: bool) {
        let Some(b) = &mut self.rfolder else { return };
        b.step(down);
        self.push_rfolder_pick();
    }

    /// Open here: the remote tab in the picked folder, else the folder shown, else the host's home.
    pub(super) fn rfolder_open_here(&mut self) {
        let Some(b) = self.rfolder.take() else { return };
        let target = b.target();
        self.push_rfolder();
        self.clear_tip();
        self.open_remote(&b.machine, &target, None, true);
        self.focus_terminal();
    }

    /// Cancel, Escape or a press outside: the dialog closes and nothing opens.
    pub(super) fn rfolder_cancel(&mut self) {
        if self.rfolder.take().is_some() {
            self.push_rfolder();
            self.clear_tip();
            self.refresh();
            self.focus_terminal();
        }
    }

    /// The dialog for the e2e (`app.state`'s `remote_folder`) and the tests.
    pub fn rfolder_state(&self) -> serde_json::Value {
        let Some(b) = &self.rfolder else {
            return json!({ "open": false });
        };
        json!({
            "open": true,
            "machine": b.machine,
            "path": b.path(),
            "target": b.target(),
            "loading": b.loading(),
            "error": b.error(),
            "names": b.entries().iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            "selected": b.selected(),
            "crumbs": b.crumbs().iter().map(|c| c.label.as_str().to_string()).collect::<Vec<_>>(),
        })
    }
}
