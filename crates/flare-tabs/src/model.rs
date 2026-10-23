//! TabModel: the window tab state (Task 18).
//!
//! Source: PT App.tsx (state, openTab, closeNow, the mru effect, commitWalk, next-tab, jump-tab,
//! settings cases, the tabsChanged effect). One object the UI thread owns; the glue calls it.

use crate::restore::{Restored, SavedTab, SavedTabs, Spawn, peek_state, settle_restore_from};
use crate::tab_mru::{Walk, start_walk, step_walk, sync_mru, touch_mru, walk_holds, walk_target};
use crate::tabs::{self, Pending, SETTINGS_ID, Tab, TabKind, TabState};
use flare_agent::kinds::AgentKind;
use flare_settings::tab_switch::TabSwitch;
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

    pub fn close(&mut self, id: &str) -> bool {
        if !self.state.tabs.iter().any(|t| t.id == id) {
            return false;
        }
        self.state = tabs::close_tab(&self.state, id);
        self.agents.remove(id);
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

    /// Shell tabs only; `active` is the index among them, 0 when Settings is in front.
    pub fn saved(&self) -> SavedTabs {
        let shells: Vec<&Tab> = tabs::shell_tabs(&self.state);
        let active = self
            .state
            .active_id
            .as_deref()
            .and_then(|a| shells.iter().position(|t| t.id == a))
            .unwrap_or(0);
        SavedTabs {
            tabs: shells
                .iter()
                .map(|t| SavedTab {
                    cwd: t.cwd.clone(),
                    agent: self.agent_kind(&t.id),
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
                    agent: None
                },
                SavedTab {
                    cwd: r"C:\b".into(),
                    agent: Some(AgentKind::Claude)
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
                },
                SavedTab {
                    cwd: r"C:\b".into(),
                    agent: None,
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
                },
                RestoredTab {
                    cwd: r"C:\b".into(),
                    resume: None,
                    from: Some(1),
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
