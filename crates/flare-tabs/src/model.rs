//! TabModel: the window tab state (Task 18).
//!
//! Source: PT App.tsx (state, openTab, closeNow, the mru effect, commitWalk, next-tab, jump-tab,
//! settings cases, the tabsChanged effect). One object the UI thread owns; the glue calls it.

use crate::restore::{
    Restored, SavedRemote, SavedTab, SavedTabs, Spawn, peek_state, settle_restore_from,
};
use crate::tab_mru::{Walk, start_walk, step_walk, sync_mru, touch_mru, walk_holds, walk_target};
use crate::tabs::{self, ImportHow, MachineTag, Pending, SETTINGS_ID, Tab, TabKind, TabState};
use flare_agent::kinds::AgentKind;
use flare_settings::tab_switch::TabSwitch;
use flare_term::remote_pty::RemoteLive;
use std::collections::{HashMap, HashSet};

/// The window's tab list, the used list, the Ctrl walk, the restore flag and the agents.
pub struct TabModel {
    pub state: TabState,
    mru: Vec<String>,
    walk: Option<Walk>,
    slots: Vec<String>,
    /// The placeholder the boot put in front (`peek_state`): any other pending front was the owner's click.
    boot_front: Option<String>,
    seq: u64,
    clock_ms: u64,
    restore_done: bool,
    agents: HashMap<String, AgentKind>,
    /// Each remote tab's live state (remote sessions PR 3): the host's id of the tab and the output it read, which
    /// `tabs.json` keeps so a restart attaches to the same session (rule 288).
    remotes: HashMap<String, RemoteLive>,
    /// The remote tab rows that came by an import or an agent's adopt (rule 1290), and whether each has attached yet.
    /// Kept beside the tab, not in it, like `remotes`: these rows are never saved.
    imports: HashMap<String, ImportRow>,
}

/// An imported or adopted row's own state: how it came, and whether it has a session link yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ImportRow {
    how: ImportHow,
    attached: bool,
}

fn make_id(clock_ms: u64, seq: &mut u64) -> String {
    *seq += 1;
    format!("t{clock_ms:x}-{seq}")
}

impl TabModel {
    fn blank(clock_ms: u64) -> Self {
        TabModel {
            state: TabState::default(),
            mru: Vec::new(),
            walk: None,
            slots: Vec::new(),
            boot_front: None,
            seq: 0,
            clock_ms,
            restore_done: false,
            agents: HashMap::new(),
            remotes: HashMap::new(),
            imports: HashMap::new(),
        }
    }

    /// The saved strip drawn at once as placeholders; `restore_done` false.
    pub fn boot(saved: &SavedTabs, clock_ms: u64) -> Self {
        let mut m = Self::blank(clock_ms);
        let mut seq = 0;
        let boot = peek_state(saved, &mut || make_id(clock_ms, &mut seq));
        m.seq = seq;
        m.boot_front = boot.state.active_id.clone();
        m.state = boot.state;
        m.slots = boot.slots;
        m.after_change();
        m
    }

    /// No placeholders (tests, and a fresh start before `settle` with an empty plan).
    pub fn empty(clock_ms: u64) -> Self {
        Self::blank(clock_ms)
    }

    pub fn new_id(&mut self) -> String {
        make_id(self.clock_ms, &mut self.seq)
    }

    /// The used list, front first.
    pub fn mru(&self) -> &[String] {
        &self.mru
    }

    fn ids(&self) -> Vec<String> {
        self.state.tabs.iter().map(|t| t.id.clone()).collect()
    }

    /// The mru effect: follow the strip, drop a walk that no longer holds, touch the front tab.
    fn after_change(&mut self) {
        let ids = self.ids();
        let active = self.state.active_id.clone();
        self.mru = sync_mru(&self.mru, &ids, active.as_deref());
        if let Some(w) = &self.walk
            && !walk_holds(w, &ids, active.as_deref())
        {
            self.walk = None;
        }
        if self.walk.is_none()
            && let Some(a) = &active
        {
            self.mru = touch_mru(&self.mru, a);
        }
    }

    pub fn open_tab(&mut self, cwd: &str) -> String {
        let id = self.new_id();
        self.state = tabs::add_tab(&self.state, &id, cwd);
        self.after_change();
        id
    }

    /// A new remote tab on `machine` in the host's folder `cwd` (rule 287), its live state `live`.
    pub fn open_remote_tab(&mut self, machine: MachineTag, cwd: &str, live: RemoteLive) -> String {
        let id = self.new_id();
        self.state = tabs::add_remote_tab(&self.state, &id, cwd, machine);
        self.remotes.insert(id.clone(), live);
        self.after_change();
        id
    }

    /// A linked machine's own tab `host_tab` as an unattached remote tab row (rule 1290): appended at the end of the
    /// strip, never the front tab, and it joins the used list after the tabs already in it (`sync_mru`), so a "most
    /// recent" Ctrl+Tab still goes to the tab used before. Its live state has the host tab set and nothing read yet.
    /// Not saved in `tabs.json`. `add_tab` is not used: it always activates.
    pub fn import_remote_tab(
        &mut self,
        machine: MachineTag,
        cwd: &str,
        host_tab: &str,
        how: ImportHow,
    ) -> String {
        let id = self.new_id();
        self.state = tabs::append_remote_tab(&self.state, &id, cwd, machine);
        self.remotes
            .insert(id.clone(), RemoteLive::new(Some(host_tab.to_string()), 0));
        self.imports.insert(
            id.clone(),
            ImportRow {
                how,
                attached: false,
            },
        );
        self.after_change();
        id
    }

    /// How an imported or adopted row came here; None for every other tab.
    pub fn import_how(&self, id: &str) -> Option<ImportHow> {
        self.imports.get(id).map(|r| r.how)
    }

    /// An imported or adopted row with no session link yet: a tab with no shell to every consumer (rule 1290).
    pub fn is_unattached(&self, id: &str) -> bool {
        self.imports.get(id).is_some_and(|r| !r.attached)
    }

    /// The row attached (it came to the front or was clicked). False for a tab that is no such row.
    pub fn set_attached(&mut self, id: &str) -> bool {
        match self.imports.get_mut(id) {
            Some(r) => {
                r.attached = true;
                true
            }
            None => false,
        }
    }

    /// An imported or adopted row goes by itself (the host tab ended, the setting went off, the machine was removed).
    /// When it was in front, the most recently used other tab comes forward, not its neighbour as `close` picks. False
    /// for a tab that is no such row.
    pub fn remove_imported(&mut self, id: &str) -> bool {
        if !self.imports.contains_key(id) {
            return false;
        }
        let was_front = self.state.active_id.as_deref() == Some(id);
        let back = was_front
            .then(|| {
                self.mru
                    .iter()
                    .find(|t| t.as_str() != id && self.state.tabs.iter().any(|x| &x.id == *t))
                    .cloned()
            })
            .flatten();
        // Set the front before the used list follows, so the neighbour `close_tab` would pick is never touched.
        self.state = tabs::close_tab(&self.state, id);
        if was_front {
            self.state.active_id = back;
        }
        self.agents.remove(id);
        self.remotes.remove(id);
        self.imports.remove(id);
        self.after_change();
        true
    }

    /// The live state of a remote tab (a restored one gets its own here).
    pub fn set_remote_live(&mut self, id: &str, live: RemoteLive) {
        self.remotes.insert(id.to_string(), live);
    }

    pub fn remote_live(&self, id: &str) -> Option<&RemoteLive> {
        self.remotes.get(id)
    }

    /// Whether any tab is a remote tab.
    pub fn has_remote_tabs(&self) -> bool {
        !self.remotes.is_empty()
    }

    /// The machine a tab runs on, for a remote tab.
    pub fn remote_of(&self, id: &str) -> Option<&MachineTag> {
        self.state
            .tabs
            .iter()
            .find(|t| t.id == id)
            .and_then(|t| t.remote.as_ref())
    }

    pub fn close(&mut self, id: &str) -> bool {
        if !self.state.tabs.iter().any(|t| t.id == id) {
            return false;
        }
        self.state = tabs::close_tab(&self.state, id);
        self.agents.remove(id);
        self.remotes.remove(id);
        self.imports.remove(id);
        self.after_change();
        true
    }

    pub fn pick(&mut self, id: &str) {
        self.state = tabs::pick_tab(&self.state, id);
        self.after_change();
    }

    pub fn reorder(&mut self, id: &str, to: usize) {
        self.state.tabs = tabs::reorder_tabs(&self.state.tabs, id, to);
        self.after_change();
    }

    pub fn set_cwd(&mut self, id: &str, cwd: &str) -> bool {
        match tabs::set_cwd(&self.state, id, cwd) {
            Some(s) => {
                self.state = s;
                self.after_change();
                true
            }
            None => false,
        }
    }

    /// Ctrl+Tab. In order: step the strip. Most recent: walk a snapshot taken at the first press.
    pub fn step(&mut self, dir: i32, switch: TabSwitch) {
        match switch {
            TabSwitch::Order => {
                self.state = tabs::step_tab(&self.state, dir);
                self.after_change();
            }
            TabSwitch::Recent => {
                let walk = match self.walk.take() {
                    Some(w) => step_walk(&w, dir),
                    None => {
                        let ids = self.ids();
                        let active = self.state.active_id.clone();
                        let synced = sync_mru(&self.mru, &ids, active.as_deref());
                        let list = match &active {
                            Some(a) => touch_mru(&synced, a),
                            None => synced,
                        };
                        let Some(w) = start_walk(&list) else {
                            return;
                        };
                        step_walk(&w, dir)
                    }
                };
                let target = walk_target(&walk).to_string();
                self.walk = Some(walk);
                self.state = tabs::pick_tab(&self.state, &target);
                self.after_change();
            }
        }
    }

    /// Ctrl+1..9. False (the key is not taken) for 0 or past the last tab.
    pub fn jump(&mut self, n: usize) -> bool {
        if n == 0 || n > self.state.tabs.len() {
            return false;
        }
        let id = self.state.tabs[n - 1].id.clone();
        self.pick(&id);
        true
    }

    pub fn open_settings(&mut self) {
        self.state = tabs::open_settings(&self.state);
        self.after_change();
    }

    /// The cog: back to the tab used before, found in the used list.
    pub fn toggle_settings(&mut self) {
        let ids = self.ids();
        let back = self
            .mru
            .iter()
            .find(|id| id.as_str() != SETTINGS_ID && ids.contains(id))
            .cloned();
        self.state = tabs::toggle_settings(&self.state, back.as_deref());
        self.after_change();
    }

    /// The hold ended: the tab the walk landed on moves to the front of the used list.
    pub fn commit_walk(&mut self) {
        if let Some(w) = self.walk.take() {
            let target = walk_target(&w).to_string();
            if self.state.tabs.iter().any(|t| t.id == target) {
                self.mru = touch_mru(&self.mru, &target);
            }
        }
    }

    /// M6's hook.
    pub fn set_agent(&mut self, id: &str, kind: Option<AgentKind>) {
        match kind {
            Some(k) => {
                self.agents.insert(id.to_string(), k);
            }
            None => {
                self.agents.remove(id);
            }
        }
    }

    pub fn agent_kind(&self, id: &str) -> Option<AgentKind> {
        self.agents.get(id).copied()
    }

    pub fn has_agent(&self, id: &str) -> bool {
        self.agents.contains_key(id)
    }

    /// Each placeholder takes over its slot in place; returns the shells to start.
    pub fn settle(&mut self, r: &Restored) -> Vec<Spawn> {
        let clock = self.clock_ms;
        let mut seq = self.seq;
        let settled = settle_restore_from(
            &self.state,
            &self.slots,
            self.boot_front.as_deref(),
            r,
            &mut || make_id(clock, &mut seq),
        );
        self.seq = seq;
        self.state = settled.state;
        self.restore_done = true;
        self.after_change();
        settled.spawn
    }

    pub fn restore_done(&self) -> bool {
        self.restore_done
    }

    /// Rule 37: None until the restore answered, so an early strip never overwrites tabs.json.
    pub fn saved_if_restored(&self) -> Option<SavedTabs> {
        self.restore_done.then(|| self.saved())
    }

    /// Shell tabs only, never an imported or adopted row (rule 1290: the first poll after a start brings those back);
    /// `active` is the index among the kept tabs, 0 when Settings is in front. With an imported row in front it is
    /// the most recently used kept tab (0 when there is none).
    pub fn saved(&self) -> SavedTabs {
        let shells: Vec<&Tab> = tabs::shell_tabs(&self.state)
            .into_iter()
            .filter(|t| !self.imports.contains_key(&t.id))
            .collect();
        let at = |id: &str| shells.iter().position(|t| t.id == id);
        let active = match self.state.active_id.as_deref() {
            Some(a) if self.imports.contains_key(a) => {
                self.mru.iter().find_map(|id| at(id)).unwrap_or(0)
            }
            Some(a) => at(a).unwrap_or(0),
            None => 0,
        };
        SavedTabs {
            tabs: shells
                .iter()
                .map(|t| match &t.remote {
                    // The host owns a remote tab's session: no agent of this machine is saved for it.
                    Some(m) => {
                        let live = self.remotes.get(&t.id);
                        SavedTab {
                            cwd: t.cwd.clone(),
                            agent: None,
                            remote: Some(SavedRemote {
                                machine: m.id.clone(),
                                host_tab: live.and_then(RemoteLive::host_tab),
                                last_seq: live.map_or(0, RemoteLive::seq),
                            }),
                        }
                    }
                    None => SavedTab {
                        cwd: t.cwd.clone(),
                        agent: self.agent_kind(&t.id),
                        remote: None,
                    },
                })
                .collect(),
            active,
        }
    }

    pub fn labels(&self) -> Vec<String> {
        tabs::tab_labels(&self.state.tabs)
    }

    pub fn active(&self) -> Option<&Tab> {
        let id = self.state.active_id.as_deref()?;
        self.state.tabs.iter().find(|t| t.id == id)
    }

    /// None for Settings and for a placeholder.
    pub fn active_shell(&self) -> Option<&Tab> {
        self.active()
            .filter(|t| t.kind == TabKind::Shell && t.pending.is_none())
    }

    pub fn loading_ids(&self) -> HashSet<String> {
        self.state
            .tabs
            .iter()
            .filter(|t| t.pending == Some(Pending::Agent))
            .map(|t| t.id.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::restore::RestoredTab;

    fn abc() -> (TabModel, String, String, String) {
        let mut m = TabModel::empty(0);
        let a = m.open_tab(r"C:\a");
        let b = m.open_tab(r"C:\b");
        let c = m.open_tab(r"C:\c");
        (m, a, b, c)
    }

    fn front(m: &TabModel) -> Option<&str> {
        m.state.active_id.as_deref()
    }

    #[test]
    fn ctrl_tab_in_order_wraps() {
        let (mut m, a, _b, c) = abc();
        m.step(1, TabSwitch::Order);
        assert_eq!(front(&m), Some(a.as_str()));
        m.step(-1, TabSwitch::Order);
        assert_eq!(front(&m), Some(c.as_str()));
    }

    #[test]
    fn most_recent_walks_a_snapshot_and_commits_on_release() {
        let (mut m, a, b, c) = abc();
        m.pick(&a);
        m.pick(&c);
        m.pick(&b);
        assert_eq!(m.mru(), [b.clone(), c.clone(), a.clone()]);
        m.step(1, TabSwitch::Recent);
        assert_eq!(front(&m), Some(c.as_str()));
        m.step(1, TabSwitch::Recent);
        assert_eq!(front(&m), Some(a.as_str()));
        assert_eq!(m.mru(), [b.clone(), c.clone(), a.clone()]);
        m.commit_walk();
        assert_eq!(m.mru(), [a, b, c]);
    }

    #[test]
    fn a_click_during_a_hold_ends_the_walk() {
        let (mut m, a, b, c) = abc();
        m.pick(&a);
        m.pick(&c);
        m.pick(&b);
        m.step(1, TabSwitch::Recent);
        assert_eq!(front(&m), Some(c.as_str()));
        m.pick(&a);
        assert_eq!(m.mru()[0], a);
        m.commit_walk();
        assert_eq!(m.mru()[0], a);
    }

    #[test]
    fn jump_takes_one_to_nine_and_a_number_past_the_last_is_not_taken() {
        let (mut m, _a, b, _c) = abc();
        assert!(m.jump(2));
        assert_eq!(front(&m), Some(b.as_str()));
        assert!(!m.jump(4));
        assert_eq!(front(&m), Some(b.as_str()));
        assert!(!m.jump(0));
        assert_eq!(front(&m), Some(b.as_str()));
    }

    #[test]
    fn the_cog_toggles_and_goes_back_to_the_tab_used_before() {
        let mut m = TabModel::empty(0);
        let a = m.open_tab(r"C:\a");
        let _b = m.open_tab(r"C:\b");
        m.pick(&a);
        m.toggle_settings();
        assert_eq!(front(&m), Some("settings"));
        assert_eq!(m.labels().last().map(String::as_str), Some("Settings"));
        m.toggle_settings();
        assert!(!m.state.tabs.iter().any(|t| t.kind == TabKind::Settings));
        assert_eq!(front(&m), Some(a.as_str()));
    }

    #[test]
    fn closing_the_last_tab_leaves_no_front() {
        let mut m = TabModel::empty(0);
        let a = m.open_tab(r"C:\a");
        assert!(m.close(&a));
        assert!(m.active().is_none());
        assert!(!m.close("zzz"));
    }

    #[test]
    fn saved_skips_settings_and_keeps_a_resumable_agent() {
        let mut m = TabModel::empty(0);
        m.open_tab(r"C:\a");
        let b = m.open_tab(r"C:\b");
        m.set_agent(&b, Some(AgentKind::Claude));
        m.toggle_settings();
        assert_eq!(
            m.saved().tabs,
            vec![
                SavedTab {
                    cwd: r"C:\a".into(),
                    agent: None,
                    remote: None,
                },
                SavedTab {
                    cwd: r"C:\b".into(),
                    agent: Some(AgentKind::Claude),
                    remote: None,
                },
            ]
        );
    }

    #[test]
    fn saved_active_counts_shell_tabs_only() {
        let mut m = TabModel::empty(0);
        let _a = m.open_tab(r"C:\a");
        let b = m.open_tab(r"C:\b");
        m.pick(&b);
        m.open_settings();
        m.pick(&b);
        assert_eq!(m.saved().active, 1);
        m.open_settings();
        assert_eq!(front(&m), Some("settings"));
        assert_eq!(m.saved().active, 0);
    }

    #[test]
    fn set_agent_none_forgets_and_closing_forgets() {
        let mut m = TabModel::empty(0);
        let a = m.open_tab(r"C:\a");
        m.set_agent(&a, Some(AgentKind::Codex));
        m.set_agent(&a, None);
        assert!(!m.has_agent(&a));
        m.set_agent(&a, Some(AgentKind::Codex));
        m.close(&a);
        assert_eq!(m.agent_kind(&a), None);
    }

    #[test]
    fn boot_draws_placeholders_and_settle_takes_over_in_place() {
        let saved = SavedTabs {
            tabs: vec![
                SavedTab {
                    cwd: r"C:\a".into(),
                    agent: Some(AgentKind::Claude),
                    remote: None,
                },
                SavedTab {
                    cwd: r"C:\b".into(),
                    agent: None,
                    remote: None,
                },
            ],
            active: 1,
        };
        let mut m = TabModel::boot(&saved, 0);
        let ids = m.ids();
        assert_eq!(ids.len(), 2);
        assert!(m.state.tabs.iter().all(|t| t.pending.is_some()));
        assert_eq!(front(&m), Some(ids[1].as_str()));
        assert_eq!(m.loading_ids(), HashSet::from([ids[0].clone()]));
        let spawn = m.settle(&Restored {
            tabs: vec![
                RestoredTab {
                    cwd: r"C:\a".into(),
                    resume: Some("s1".into()),
                    from: Some(0),
                    remote: None,
                },
                RestoredTab {
                    cwd: r"C:\b".into(),
                    resume: None,
                    from: Some(1),
                    remote: None,
                },
            ],
            active: 1,
        });
        assert_eq!(spawn.len(), 2);
        assert_eq!(spawn[0].id, ids[0]);
        assert_eq!(spawn[0].resume.as_deref(), Some("s1"));
        assert_eq!(spawn[1].id, ids[1]);
        assert!(m.state.tabs.iter().all(|t| t.pending.is_none()));
        assert!(m.loading_ids().is_empty());
        assert!(m.restore_done());
    }

    #[test]
    fn saved_if_restored_is_none_until_the_restore_answered() {
        let saved = SavedTabs {
            tabs: vec![SavedTab {
                cwd: r"C:\a".into(),
                agent: None,
                remote: None,
            }],
            active: 0,
        };
        let mut m = TabModel::boot(&saved, 0);
        assert!(m.saved_if_restored().is_none());
        m.settle(&Restored {
            tabs: vec![RestoredTab {
                cwd: r"C:\a".into(),
                resume: None,
                from: Some(0),
                remote: None,
            }],
            active: 0,
        });
        assert!(m.saved_if_restored().is_some());
        let mut e = TabModel::empty(0);
        assert!(e.saved_if_restored().is_none());
        e.settle(&Restored::default());
        assert!(e.saved_if_restored().is_some());
    }

    #[test]
    fn new_ids_are_unique() {
        let mut m = TabModel::empty(7);
        let ids: HashSet<String> = (0..1000).map(|_| m.new_id()).collect();
        assert_eq!(ids.len(), 1000);
    }
}

/// Remote sessions PR 3: a remote tab in the model and in what it saves.
#[cfg(test)]
mod remote_tests {
    use super::*;

    /// Rule 288: a remote tab saves its machine, its host tab and the output it read (never an agent of this machine),
    /// and the save follows the live state the link writes.
    #[test]
    fn a_remote_tab_saves_its_host_tab_and_the_output_read() {
        let mut m = TabModel::empty(1);
        m.open_tab(r"C:\a");
        m.set_agent(&m.state.tabs[0].id.clone(), Some(AgentKind::Claude));
        let live = RemoteLive::new(None, 0);
        let id = m.open_remote_tab(MachineTag::of_id("mac"), "/Users/max/p", live.clone());
        m.set_agent(&id, Some(AgentKind::Claude));
        assert_eq!(m.remote_of(&id).map(|t| t.id.as_str()), Some("mac"));
        let s = m.saved();
        assert_eq!(s.tabs[1].agent, None);
        assert_eq!(s.tabs[1].remote.as_ref().unwrap().host_tab, None);
        live.set_host_tab("t9");
        live.set_seq(77);
        let s = m.saved();
        let r = s.tabs[1].remote.as_ref().unwrap();
        assert_eq!((r.host_tab.as_deref(), r.last_seq), (Some("t9"), 77));
        assert_eq!(s.tabs[0].agent, Some(AgentKind::Claude));
        assert_eq!(s.active, 1);
        assert!(m.close(&id));
        assert!(m.remote_live(&id).is_none());
    }

    fn mac() -> MachineTag {
        MachineTag::of_id("mac")
    }

    /// Rule 1290: an import is a remote tab row at the end of the strip, behind the tab in front, with the host tab set
    /// and no link yet. It is never saved, and closing it forgets all of it.
    #[test]
    fn an_imported_row_goes_behind_the_front_tab_and_is_not_saved() {
        let mut m = TabModel::empty(1);
        let a = m.open_tab(r"C:\a");
        let b = m.open_tab(r"C:\b");
        m.pick(&a);
        let r = m.import_remote_tab(mac(), "/Users/max/p", "t5-1", ImportHow::Imported);
        let d = m.import_remote_tab(mac(), "/Users/max/q", "t5-2", ImportHow::Adopted);
        assert_eq!(
            m.state.active_id.as_deref(),
            Some(a.as_str()),
            "the front stays"
        );
        let ids: Vec<&str> = m.state.tabs.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, [a.as_str(), b.as_str(), r.as_str(), d.as_str()]);
        assert_eq!(m.remote_of(&r).map(|t| t.id.as_str()), Some("mac"));
        let live = m.remote_live(&r).unwrap();
        assert_eq!((live.host_tab().as_deref(), live.seq()), (Some("t5-1"), 0));
        assert_eq!(m.import_how(&r), Some(ImportHow::Imported));
        assert_eq!(m.import_how(&d), Some(ImportHow::Adopted));
        assert_eq!(m.import_how(&a), None);
        assert!(m.is_unattached(&r) && m.is_unattached(&d));
        assert!(!m.is_unattached(&a));
        // A tab opened from here is saved; the imported and adopted rows are not, attached or not.
        m.open_remote_tab(
            mac(),
            "/Users/max/o",
            RemoteLive::new(Some("t5-9".into()), 0),
        );
        m.pick(&a);
        assert!(m.set_attached(&r));
        assert!(!m.is_unattached(&r));
        assert!(!m.set_attached(&a), "not an imported row");
        let s = m.saved();
        let cwds: Vec<&str> = s.tabs.iter().map(|t| t.cwd.as_str()).collect();
        assert_eq!(cwds, [r"C:\a", r"C:\b", "/Users/max/o"]);
        assert_eq!(s.active, 0);
        assert!(m.close(&r));
        assert_eq!(m.import_how(&r), None);
        assert!(!m.is_unattached(&r));
        assert!(m.remote_live(&r).is_none());
        // An import into an empty strip leaves no tab in front.
        let mut e = TabModel::empty(2);
        e.import_remote_tab(mac(), "/p", "t1-1", ImportHow::Imported);
        assert_eq!(e.state.active_id, None);
    }

    /// Rule 1290: an import joins the used list after the tabs already in it, so a "most recent" Ctrl+Tab goes to the
    /// tab used before, never to the newest import.
    #[test]
    fn an_imported_row_joins_the_end_of_the_used_list_so_ctrl_tab_goes_to_the_tab_used_before() {
        let mut m = TabModel::empty(1);
        let a = m.open_tab(r"C:\a");
        let b = m.open_tab(r"C:\b");
        m.pick(&a);
        m.pick(&b);
        let r = m.import_remote_tab(mac(), "/p", "t5-1", ImportHow::Imported);
        assert_eq!(m.mru(), [b.clone(), a.clone(), r.clone()]);
        m.step(1, TabSwitch::Recent);
        assert_eq!(m.state.active_id.as_deref(), Some(a.as_str()));
        m.commit_walk();
        assert_eq!(m.mru(), [a.clone(), b.clone(), r.clone()]);
    }

    /// Rule 1290: `saved()` counts `active` over the tabs it keeps; with an imported row in front it saves the most
    /// recently used kept tab, and an imported row in front with no kept tab saves 0.
    #[test]
    fn saved_counts_active_over_the_kept_tabs_and_an_imported_front_saves_the_last_used_kept_tab() {
        let mut m = TabModel::empty(1);
        m.open_tab(r"C:\a");
        let r = m.import_remote_tab(mac(), "/p", "t5-1", ImportHow::Imported);
        let b = m.open_tab(r"C:\b");
        let c = m.open_tab(r"C:\c");
        // Strip a, r, b, c: c is the third kept tab, not the fourth tab.
        assert_eq!(m.saved().active, 2);
        m.pick(&b);
        assert_eq!(m.saved().active, 1);
        m.pick(&r);
        assert_eq!(m.saved().active, 1, "b was used last");
        m.pick(&c);
        m.pick(&r);
        assert_eq!(m.saved().active, 2, "c was used last");
        m.open_settings();
        assert_eq!(m.saved().active, 0, "Settings in front saves 0 as before");
        let mut lone = TabModel::empty(2);
        let only = lone.import_remote_tab(mac(), "/p", "t1-1", ImportHow::Adopted);
        lone.pick(&only);
        let s = lone.saved();
        assert!(s.tabs.is_empty());
        assert_eq!(s.active, 0);
    }

    /// Rule 1290: a row that goes by itself while in front hands the front to the most recently used other tab, not to
    /// its neighbour, and the neighbour is not touched in the used list. Only imported or adopted rows go this way.
    #[test]
    fn remove_imported_fronts_the_most_recently_used_other_tab() {
        let mut m = TabModel::empty(1);
        let a = m.open_tab(r"C:\a");
        let r = m.import_remote_tab(mac(), "/p", "t5-1", ImportHow::Imported);
        let b = m.open_tab(r"C:\b");
        m.pick(&a);
        m.pick(&r);
        assert!(m.remove_imported(&r));
        assert_eq!(m.state.active_id.as_deref(), Some(a.as_str()));
        assert_eq!(m.mru(), [a.clone(), b.clone()]);
        assert!(m.remote_live(&r).is_none());
        assert!(!m.remove_imported(&r));
        assert!(
            !m.remove_imported(&b),
            "a tab opened here is closed, never removed this way"
        );
        // Behind the front: the front stays.
        let s = m.import_remote_tab(mac(), "/q", "t5-2", ImportHow::Adopted);
        assert!(m.remove_imported(&s));
        assert_eq!(m.state.active_id.as_deref(), Some(a.as_str()));
        // The last tab: nothing is in front after it.
        let mut lone = TabModel::empty(2);
        let only = lone.import_remote_tab(mac(), "/p", "t1-1", ImportHow::Imported);
        lone.pick(&only);
        assert!(lone.remove_imported(&only));
        assert_eq!(lone.state.active_id, None);
    }
}
