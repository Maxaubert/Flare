//! Import remote sessions (rules 1291 to 1295) on the UI thread: the tabs that a linked machine's own Flare runs show
//! here as remote tab rows.
//!
//! - One watch link per machine in `machines.json` while the setting is on, or while an agent's adopted row of that
//!   machine is in the strip (rule 1291). Its answers come here as [`UiMsg::Import`]; no round runs before the restore
//!   answered, and [`crate::remote_import::import_plan`] says what changes.
//! - A row attaches only when it is in front and no Ctrl+Tab walk is held (rule 1292): the hook is at the top of
//!   `refresh()`. It never opens or resumes, and an imported Claude tab the host sizes sends no size until the first
//!   click or key here.
//! - Close on any remote tab puts its host tab on the closing list; an unattached row's close goes over the watch link.
//!   Hide here only remembers the host tab (rule 1293).
//! - The setting off removes imported rows by detach, never a close (rule 1294).
//! - An unattached row has no handle: no dictation, no plugin input, no close or install hold (rule 1295).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use flare_settings::machine_prefs;
use flare_tabs::tabs::ImportHow;
use flare_term::remote_pty::{HostStatus, LinkState};

use super::{AppCtl, UiMsg};
use crate::remote_import::{
    ClosingList, HiddenList, Round, Shown, Watch, WatchAnswer, WatchPort, import_plan,
};

/// The window's import state.
#[derive(Default)]
pub(crate) struct Imports {
    /// What starts the watch links (None: this run imports nothing, as in most tests).
    port: Option<Arc<dyn WatchPort>>,
    /// The watch link of each machine watched now.
    watches: HashMap<String, Box<dyn Watch>>,
    hidden: HiddenList,
    closing: ClosingList,
    /// The newest status the watch brought of each unattached row: how it attaches (rule 1292).
    last: HashMap<String, HostStatus>,
    /// Machines whose host was logged as older (one line each).
    older: HashSet<String>,
    /// The setting as last applied.
    on: bool,
    /// A Ctrl+Tab walk is held: the row it lands on attaches on the Control release, the rows passed over do not.
    pub(crate) stepping: bool,
}

impl AppCtl {
    /// The window hands over the watch links (startup; a test gives a fake). The hidden list is read, and every
    /// machine is watched while the setting is on.
    pub fn set_watch_port(&mut self, port: Arc<dyn WatchPort>) {
        self.imports.port = Some(port);
        self.imports.hidden = HiddenList::from_map(machine_prefs::import_hidden(&self.prefs));
        self.imports.on = machine_prefs::import_remote_on(&self.prefs);
        self.sync_watches();
    }

    /// The machines watched now (rule 1291): every one `machines.json` names while the setting is on; while it is
    /// off, only those of an agent's adopted rows.
    fn wanted_watches(&self) -> HashSet<String> {
        let known: HashSet<&str> = self.machines.iter().map(|m| m.id.as_str()).collect();
        let mut want: HashSet<String> = if self.imports.on {
            known.iter().map(|m| (*m).to_string()).collect()
        } else {
            HashSet::new()
        };
        for t in &self.model.state.tabs {
            if self.model.import_how(&t.id) == Some(ImportHow::Adopted)
                && let Some(r) = &t.remote
                && known.contains(r.id.as_str())
            {
                want.insert(r.id.clone());
            }
        }
        want
    }

    /// Starts and stops the watch links to match [`AppCtl::wanted_watches`]. A dropped watch stops its link.
    pub(crate) fn sync_watches(&mut self) {
        let Some(port) = self.imports.port.clone() else {
            return;
        };
        let want = self.wanted_watches();
        self.imports.watches.retain(|m, _| want.contains(m));
        for m in want {
            if self.imports.watches.contains_key(&m) {
                continue;
            }
            let post = Arc::clone(&self.deps.post);
            let w = port.watch(&m, Arc::new(move |a| post(UiMsg::Import(a))));
            self.imports.watches.insert(m, w);
        }
    }

    /// The machines watched now (tests).
    #[cfg(test)]
    pub(crate) fn watched(&self) -> Vec<String> {
        let mut v: Vec<String> = self.imports.watches.keys().cloned().collect();
        v.sort();
        v
    }

    /// The setting changed, maybe (every row's change posts `PrefsChanged`). Off (rule 1294): every imported row goes by
    /// detach, never a close, and an agent's adopted rows stay. On: the machines are watched again.
    pub(crate) fn sync_import_setting(&mut self) {
        if self.imports.port.is_none() {
            return;
        }
        let on = machine_prefs::import_remote_on(&self.prefs);
        if on == self.imports.on {
            return;
        }
        self.imports.on = on;
        if !on {
            let rows: Vec<String> = self
                .model
                .state
                .tabs
                .iter()
                .filter(|t| self.model.import_how(&t.id) == Some(ImportHow::Imported))
                .map(|t| t.id.clone())
                .collect();
            for id in rows {
                self.drop_import_row(&id);
            }
        }
        self.sync_watches();
        self.refresh();
    }

    /// The machines changed (Settings > Machines): a removed machine's watch stops, its imported and adopted rows go by
    /// detach and its hidden and closing lists go.
    pub(crate) fn machines_changed(&mut self, before: &[String]) {
        let now: HashSet<&str> = self.machines.iter().map(|m| m.id.as_str()).collect();
        let gone: Vec<String> = before
            .iter()
            .filter(|m| !now.contains(m.as_str()))
            .cloned()
            .collect();
        for m in &gone {
            let rows: Vec<String> = self
                .model
                .state
                .tabs
                .iter()
                .filter(|t| {
                    self.model.import_how(&t.id).is_some()
                        && t.remote.as_ref().is_some_and(|r| &r.id == m)
                })
                .map(|t| t.id.clone())
                .collect();
            for id in rows {
                self.drop_import_row(&id);
            }
            if self.imports.hidden.drop_machine(m) {
                self.save_hidden();
            }
            self.imports.closing.drop_machine(m);
            self.imports.older.remove(m);
        }
        self.sync_watches();
    }

    fn save_hidden(&self) {
        machine_prefs::set_import_hidden(&self.prefs, self.imports.hidden.as_map());
    }

    /// An imported or adopted row goes by itself (the host tab ended, the setting went off, the machine was removed):
    /// an attached one detaches (`remote.detach`), never closes; nothing is remembered as closed (rules 1293, 1294).
    fn drop_import_row(&mut self, id: &str) {
        let unattached = self.model.is_unattached(id);
        self.note_close(id);
        self.deps.host.leave(id);
        self.working.remove(id);
        self.model.remove_imported(id);
        self.term.tab_closed(id);
        self.forget_tab_surfaces(id);
        if unattached {
            self.forget_unattached(id);
        }
    }

    /// An unattached row never had a handle, so the hub's sync never sees it go: its marks and decor are let go here.
    fn forget_unattached(&mut self, id: &str) {
        self.imports.last.remove(id);
        self.hub.on_closed(id);
        self.decor.remove_tab(id);
        self.plugin_tab_gone(id);
        crate::dictation_ctl::tab_gone(id);
    }

    /// A watch link's answer (rule 1290's plan, applied). A machine no longer watched, or a round before the restore
    /// answered, changes nothing but the closing list.
    pub(crate) fn on_import(&mut self, a: WatchAnswer) {
        if !self.imports.watches.contains_key(&a.machine) {
            return;
        }
        let now = Instant::now();
        let answer = a.heard.answer();
        self.imports.closing.heard(&a.machine, &answer, now);
        // No round before the restore answered: a saved remote tab and its import never both appear.
        if !self.model.restore_done() {
            return;
        }
        let machine = a.machine.as_str();
        let shown: Vec<Shown> = self
            .model
            .state
            .tabs
            .iter()
            .filter(|t| t.remote.as_ref().is_some_and(|r| r.id == machine))
            .map(|t| Shown {
                id: t.id.clone(),
                host_tab: self.model.remote_live(&t.id).and_then(|l| l.host_tab()),
                how: self.model.import_how(&t.id),
                attached: !self.model.is_unattached(&t.id),
            })
            .collect();
        // An open there still waits for its host tab (its link has not ended): the round is skipped.
        let in_flight = shown.iter().any(|s| {
            s.host_tab.is_none()
                && self
                    .model
                    .remote_live(&s.id)
                    .is_some_and(|l| l.link() != LinkState::Ended)
        });
        let plan = import_plan(&Round {
            machine,
            answer,
            shown: &shown,
            hidden: &self.imports.hidden,
            closing: &self.imports.closing,
            in_flight,
            add_new: self.imports.on,
            now,
        });
        if plan.older_host && self.imports.older.insert(machine.to_string()) {
            log::info!("{machine} runs a Flare from before rule 1289: nothing is imported from it");
        }
        if !plan.hidden_gone.is_empty() && self.imports.hidden.forget(machine, &plan.hidden_gone) {
            self.save_hidden();
        }
        let mut changed = !plan.remove.is_empty() || !plan.add.is_empty();
        for id in &plan.remove {
            self.drop_import_row(id);
        }
        for (id, st) in plan.update {
            if self.imports.last.get(&id) == Some(&st) {
                continue;
            }
            if let Some(live) = self.model.remote_live(&id) {
                live.set_status(st.clone());
                self.imports.last.insert(id, st);
                changed = true;
            }
        }
        let tag = self.machine_tag(machine);
        for row in plan.add {
            let id = self.model.import_remote_tab(
                tag.clone(),
                &row.folder,
                &row.host_tab,
                ImportHow::Imported,
            );
            // The hub knows it as a remote tab from now, or no mark would show before it attaches.
            self.hub.on_spawned(&id, &row.folder, None);
            self.hub.on_remote_tab(&id);
            if let Some(live) = self.model.remote_live(&id) {
                live.set_status(row.status.clone());
            }
            self.imports.last.insert(id, row.status);
        }
        if changed {
            // The statuses set above raise the marks as a link's would (rule 294), then the strip is drawn again.
            self.host_wake();
            self.refresh();
        }
    }

    /// Rule 1292: the tab in front is an unattached row and no Ctrl+Tab walk is held: it attaches now. Called at the
    /// top of `refresh()`, so every way a row comes forward attaches it.
    pub(crate) fn attach_front_import(&mut self) {
        if self.imports.stepping {
            return;
        }
        if let Some(id) = self.model.state.active_id.clone()
            && self.model.is_unattached(&id)
        {
            self.attach_imported(&id);
        }
    }

    /// An unattached row attaches (rule 1292): its own live state (host tab set), never `remote.open` or a resume, and
    /// `ring: "both"` (rule 1295). A Claude tab the host's window sizes starts in the host-sizes state (rule 1193): no
    /// size until the first click or key here, and its fitted view may stand at once. Not `start_remote`: its second
    /// `on_spawned` would drop the session the poll gave, and it would add the host's folder to the recent folders.
    fn attach_imported(&mut self, id: &str) {
        let (Some(tag), Some(live)) = (
            self.model.remote_of(id).cloned(),
            self.model.remote_live(id).cloned(),
        ) else {
            return;
        };
        let folder = self
            .model
            .state
            .tabs
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.cwd.clone())
            .unwrap_or_default();
        let st = self.imports.last.remove(id);
        let session = st.as_ref().is_some_and(|s| s.session.is_some());
        let host_sized = st.as_ref().is_some_and(|s| s.host_sized);
        self.model.set_attached(id);
        crate::diag_ctl::crumb_often("import-attach", &[("id", crate::diag_ctl::txt(id))]);
        self.deps.host.spawn_remote(
            id,
            flare_term::remote_pty::RemoteSpec {
                machine: tag.id.clone(),
                folder,
                resume: None,
                live,
                host_sized_start: host_sized && session,
                ring_both: true,
            },
        );
        // The snapshot is not new work.
        self.hub.on_remote_tab(id);
        if st.is_some() {
            self.rconvo.heard(id, host_sized, session);
        }
        // An adopted row's machine may need its watch no more.
        self.sync_watches();
    }

    /// Close on a remote tab (rule 1293): its host tab, read before the row goes, stays out of the import while the host
    /// asks about the close (rule 48). An unattached row's close goes over its machine's watch link.
    pub(crate) fn closing_remote(&mut self, id: &str) {
        let (Some(machine), Some(host_tab)) = (
            self.model.remote_of(id).map(|t| t.id.clone()),
            self.model.remote_live(id).and_then(|l| l.host_tab()),
        ) else {
            return;
        };
        self.imports
            .closing
            .add(&machine, &host_tab, Instant::now());
        if self.model.is_unattached(id) {
            match self.imports.watches.get(&machine) {
                Some(w) => w.close(&host_tab),
                None => log::warn!("no watch link to {machine} to close an imported tab"),
            }
        }
    }

    /// The row went by a close or a hide: an unattached one is let go of in the hub too.
    pub(crate) fn closed_import(&mut self, id: &str, unattached: bool) {
        if unattached {
            self.forget_unattached(id);
        }
        self.sync_watches();
    }

    /// Hide here only on a remote tab (rule 1293): its host tab, read before the row goes, is hidden on its machine, for
    /// an imported tab and for one opened from here alike (once detached, its host lists it as its own).
    pub(crate) fn hiding_remote(&mut self, id: &str) {
        if let (Some(machine), Some(host_tab)) = (
            self.model.remote_of(id).map(|t| t.id.clone()),
            self.model.remote_live(id).and_then(|l| l.host_tab()),
        ) && self.imports.hidden.hide(&machine, &host_tab)
        {
            self.save_hidden();
        }
    }

    /// The Control release ended a Ctrl+Tab walk (or the window lost the keys): the row the keys landed on attaches.
    pub(crate) fn end_stepping(&mut self) {
        if std::mem::take(&mut self.imports.stepping) {
            self.refresh();
        }
    }

    /// Whether `id` is an unattached row (no shell here to any consumer, rule 1295).
    pub(crate) fn unattached(&self, id: &str) -> bool {
        self.model.is_unattached(id)
    }

    /// The tabs at work whose shell runs through this window: an unattached row is left out (rule 1295), its quit only
    /// detaches nothing.
    pub(crate) fn working_here(&self) -> usize {
        self.hub
            .indicator()
            .working_ids()
            .iter()
            .filter(|t| !self.model.is_unattached(t))
            .count()
    }

    /// The hidden host tabs of `machine` (tests).
    #[cfg(test)]
    pub(crate) fn hidden_of(&self, machine: &str) -> Vec<String> {
        self.imports.hidden.ids(machine).to_vec()
    }
}
