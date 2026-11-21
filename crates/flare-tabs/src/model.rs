//! TabModel: the window tab state (Task 18).
//!
//! Source: PT App.tsx (state, openTab, closeNow, the mru effect, commitWalk, next-tab, jump-tab,
//! settings cases, the tabsChanged effect). One object the UI thread owns; the glue calls it.

use crate::restore::{
    Restored, SavedHidden, SavedRemote, SavedTab, SavedTabs, Spawn, peek_state, settle_restore_from,
};
use crate::tab_mru::{Walk, start_walk, step_walk, sync_mru, touch_mru, walk_holds, walk_target};
use crate::tabs::{self, MachineTag, Pending, SETTINGS_ID, Tab, TabKind, TabState};
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
    /// Each Claude tab's session as `tabs.json` saves it (rule 1286): the id its hooks named, or `SESSION_NEW`.
    sessions: HashMap<String, String>,
    /// Each remote tab's live state (remote sessions PR 3): the host's id of the tab and the output it read, which
    /// `tabs.json` keeps so a restart attaches to the same session (rule 288).
    remotes: HashMap<String, RemoteLive>,
    /// Sessions this machine runs for another machine's Flare (rule 1302): never in `state`, so the strip, the
    /// sidebar, the used list, Ctrl+Tab, Ctrl+1..9 and every front pick pass them by. Their agents and sessions live
    /// in `agents` and `sessions` as any tab's do.
    hidden: Vec<Hidden>,
    /// A random word of this run, in every hidden session's id (rule 1307): the window clock starts at 0 each run, so a
    /// plain id (`t0-6`) comes back after a restart naming whatever tab took that place, and an opener would follow it.
    run: u64,
}

/// Pure: a hidden session's id, `h<run>-<n>`: unique in this run and never one an earlier run gave (rule 1307).
fn hidden_id(run: u64, seq: &mut u64) -> String {
    *seq += 1;
    format!("h{run:x}-{seq}")
}

/// A random word for this run's hidden ids (no dependency: the standard hasher's seeds are random per process).
fn new_run() -> u64 {
    use std::hash::BuildHasher;
    std::collections::hash_map::RandomState::new().hash_one(0x6869_6464_656e_u64) & 0xffff_ffff_ffff
}

/// A hidden session (rule 1302): a local shell tab no strip shows, kept for the machine that opened it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hidden {
    /// Its id, folder and kind as any shell tab's; `pending` and `remote` are always None.
    pub tab: Tab,
    /// The opener's machine name, as its link named itself (updated at each attach).
    pub opener: String,
    /// The opener's `open_id` (rule 1210), kept for the session's life and saved with it.
    pub open_id: Option<String>,
    /// When it was opened (the window clock).
    pub since_ms: u64,
    /// The opener's last size: the session is born and restored at it, never at this window's size.
    pub size: Option<(u16, u16)>,
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
            sessions: HashMap::new(),
            remotes: HashMap::new(),
            hidden: Vec::new(),
            run: new_run(),
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
        self.sessions.remove(id);
        self.remotes.remove(id);
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

    /// A hidden session in `cwd` for the machine `opener` (rule 1302): never added to the strip, so nothing in front
    /// moves and the used list never holds it.
    pub fn open_hidden(
        &mut self,
        cwd: &str,
        opener: &str,
        open_id: Option<&str>,
        size: Option<(u16, u16)>,
        now_ms: u64,
    ) -> String {
        // Rule 1307: an id no earlier run of this Flare gave, so an opener's saved id never names another session.
        let id = hidden_id(self.run, &mut self.seq);
        self.hidden.push(Hidden {
            tab: Tab {
                id: id.clone(),
                kind: TabKind::Shell,
                cwd: cwd.to_string(),
                pending: None,
                remote: None,
            },
            // Rule 1306: the one check restore uses too, so what opens is what comes back.
            opener: crate::restore::valid_opener(opener)
                .unwrap_or(crate::restore::OPENER_UNNAMED)
                .to_string(),
            open_id: open_id.map(str::to_string),
            since_ms: now_ms,
            size,
        });
        id
    }

    /// The hidden sessions, oldest first.
    pub fn hidden(&self) -> &[Hidden] {
        &self.hidden
    }

    pub fn hidden_of(&self, id: &str) -> Option<&Hidden> {
        self.hidden.iter().find(|h| h.tab.id == id)
    }

    pub fn is_hidden(&self, id: &str) -> bool {
        self.hidden_of(id).is_some()
    }

    /// The hidden session goes, with its agent, session and remote state. False for any other id.
    pub fn close_hidden(&mut self, id: &str) -> bool {
        let before = self.hidden.len();
        self.hidden.retain(|h| h.tab.id != id);
        if self.hidden.len() == before {
            return false;
        }
        self.agents.remove(id);
        self.sessions.remove(id);
        self.remotes.remove(id);
        true
    }

    /// The opener's size of a hidden session (saved with it). False when it is no hidden session or nothing changed.
    pub fn set_hidden_size(&mut self, id: &str, cols: u16, rows: u16) -> bool {
        match self.hidden.iter_mut().find(|h| h.tab.id == id) {
            Some(h) if h.size != Some((cols, rows)) => {
                h.size = Some((cols, rows));
                true
            }
            _ => false,
        }
    }

    /// The opener named itself again (each attach): a renamed machine shows its new name. A name the check refuses
    /// (rule 1306) changes nothing.
    pub fn set_hidden_opener(&mut self, id: &str, opener: &str) -> bool {
        let Some(opener) = crate::restore::valid_opener(opener) else {
            return false;
        };
        match self.hidden.iter_mut().find(|h| h.tab.id == id) {
            Some(h) if h.opener != opener => {
                h.opener = opener.to_string();
                true
            }
            _ => false,
        }
    }

    /// The hidden sessions' labels, as the strip would name them among themselves.
    pub fn hidden_labels(&self) -> Vec<String> {
        let tabs: Vec<Tab> = self.hidden.iter().map(|h| h.tab.clone()).collect();
        tabs::tab_labels(&tabs)
    }

    pub fn set_cwd(&mut self, id: &str, cwd: &str) -> bool {
        // A hidden session follows its shell's folder too (OSC 7), but nothing in the strip moves.
        if let Some(h) = self.hidden.iter_mut().find(|h| h.tab.id == id) {
            if h.tab.cwd.to_lowercase() == cwd.to_lowercase() {
                return false;
            }
            h.tab.cwd = cwd.to_string();
            return true;
        }
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

    /// The session `tabs.json` saves for a Claude tab (rule 1286): a session id or `SESSION_NEW`; anything else, and
    /// None, forgets it. Fed beside `set_agent`.
    pub fn set_session(&mut self, id: &str, session: Option<&str>) {
        match session.and_then(crate::restore::saved_session) {
            Some(s) => {
                self.sessions.insert(id.to_string(), s);
            }
            None => {
                self.sessions.remove(id);
            }
        }
    }

    pub fn session(&self, id: &str) -> Option<&str> {
        self.sessions.get(id).map(String::as_str)
    }

    pub fn agent_kind(&self, id: &str) -> Option<AgentKind> {
        self.agents.get(id).copied()
    }

    pub fn has_agent(&self, id: &str) -> bool {
        self.agents.contains_key(id)
    }

    /// Each placeholder takes over its slot in place; returns the shells to start. The restored hidden sessions
    /// (rule 1302) join the hidden list with new ids and come last in the answer: [`TabModel::is_hidden`] tells them.
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
        let mut spawn = settled.spawn;
        for h in &r.hidden {
            if self.hidden.len() >= crate::restore::MAX_HIDDEN {
                break;
            }
            let id = self.open_hidden(&h.cwd, &h.opener, h.open_id.as_deref(), h.size, clock);
            spawn.push(Spawn {
                id,
                cwd: h.cwd.clone(),
                resume: h.resume.clone(),
                remote: None,
            });
        }
        spawn
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
                                open_id: live.and_then(RemoteLive::open_id),
                            }),
                            session: None,
                        }
                    }
                    None => {
                        let agent = self.agent_kind(&t.id);
                        // Only a Claude tab keeps its session (rule 1286).
                        let session = (agent == Some(AgentKind::Claude))
                            .then(|| self.session(&t.id).map(str::to_string))
                            .flatten();
                        SavedTab {
                            cwd: t.cwd.clone(),
                            agent,
                            remote: None,
                            session,
                        }
                    }
                })
                .collect(),
            active,
            // Rule 1302: after the strip, hidden as they are, with the opener's name, open id and size.
            hidden: self
                .hidden
                .iter()
                .map(|h| {
                    let agent = self.agent_kind(&h.tab.id);
                    SavedHidden {
                        cwd: h.tab.cwd.clone(),
                        agent,
                        session: (agent == Some(AgentKind::Claude))
                            .then(|| self.session(&h.tab.id).map(str::to_string))
                            .flatten(),
                        opener: h.opener.clone(),
                        open_id: h.open_id.clone(),
                        size: h.size,
                    }
                })
                .collect(),
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
                    session: None,
                },
                SavedTab {
                    cwd: r"C:\b".into(),
                    agent: Some(AgentKind::Claude),
                    remote: None,
                    session: None,
                },
            ]
        );
    }

    /// Rule 1286: a Claude tab's session (an id or `new`) is saved beside its agent; a tab that is not Claude, a bad
    /// value and a closed tab keep none.
    #[test]
    fn saved_keeps_a_claude_tabs_session_only() {
        const SID: &str = "3f2a9c1e-77aa-4c0d-9b1e-0a1b2c3d4e5f";
        let mut m = TabModel::empty(0);
        let a = m.open_tab(r"C:\a");
        let b = m.open_tab(r"C:\b");
        let c = m.open_tab(r"C:\c");
        m.set_agent(&a, Some(AgentKind::Claude));
        m.set_session(&a, Some(SID));
        m.set_agent(&b, Some(AgentKind::Claude));
        m.set_session(&b, Some(crate::restore::SESSION_NEW));
        m.set_agent(&c, Some(AgentKind::Codex));
        m.set_session(&c, Some(SID));
        let sessions: Vec<Option<String>> = m.saved().tabs.into_iter().map(|t| t.session).collect();
        assert_eq!(sessions, vec![Some(SID.into()), Some("new".into()), None]);
        m.set_session(&a, Some("x; rm"));
        assert_eq!(m.session(&a), None);
        m.set_session(&b, None);
        assert_eq!(m.session(&b), None);
        m.set_session(&a, Some(SID));
        assert!(m.close(&a));
        assert_eq!(m.session(&a), None);
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
                    session: None,
                },
                SavedTab {
                    cwd: r"C:\b".into(),
                    agent: None,
                    remote: None,
                    session: None,
                },
            ],
            active: 1,
            hidden: Vec::new(),
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
            hidden: Vec::new(),
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
                session: None,
            }],
            active: 0,
            hidden: Vec::new(),
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
            hidden: Vec::new(),
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
        let live = RemoteLive::new(None, 0, Some("o-1".into()));
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
        // Rule 1307: the open id is saved with the tab.
        assert_eq!(r.open_id.as_deref(), Some("o-1"));
        assert_eq!(s.tabs[0].agent, Some(AgentKind::Claude));
        assert_eq!(s.active, 1);
        assert!(m.close(&id));
        assert!(m.remote_live(&id).is_none());
    }
}

/// Rule 1302: a hidden session in the model.
#[cfg(test)]
mod hidden_tests {
    use super::*;
    use crate::restore::{RestoredHidden, RestoredTab, parse_tabs, tabs_json};

    const SID: &str = "3f2a9c1e-77aa-4c0d-9b1e-0a1b2c3d4e5f";

    /// Rule 1307: the window clock starts at 0 each run, so strip ids repeat across restarts (`t0-1`). A hidden
    /// session's id carries a random word of the run: the same place in a new run (a host restart) never gives the id
    /// an opener saved, so its attach hears `no_tab` and it finds its session by its open id instead of following
    /// whatever tab took that place.
    #[test]
    fn a_hidden_sessions_id_is_never_one_an_earlier_run_gave() {
        let mut ids = HashSet::new();
        for _ in 0..20 {
            let mut m = TabModel::empty(0);
            let t = m.open_tab(r"C:\a");
            assert_eq!(t, "t0-1", "strip ids are the clock's");
            let h = m.open_hidden(r"C:\h", "Mac Mini", Some("o-1"), None, 0);
            assert!(h.starts_with('h') && h.len() <= 64, "{h}");
            assert!(ids.insert(h.clone()), "{h} came back in another run");
            let h2 = m.open_hidden(r"C:\h2", "Mac Mini", None, None, 0);
            assert_ne!(h, h2);
            assert!(m.is_hidden(&h) && m.is_hidden(&h2));
        }
    }

    #[test]
    fn a_hidden_session_is_not_in_the_strip_the_walk_or_the_front_pick() {
        let mut m = TabModel::empty(0);
        let a = m.open_tab(r"C:\a");
        let b = m.open_tab(r"C:\b");
        let h = m.open_hidden(r"C:\h", "Mac Mini", Some("o-1"), Some((90, 30)), 5);
        assert_eq!(
            m.state.active_id.as_deref(),
            Some(b.as_str()),
            "nothing in front moved"
        );
        assert!(m.state.tabs.iter().all(|t| t.id != h), "never in the strip");
        assert!(!m.mru().contains(&h), "never in the used list");
        assert_eq!(m.labels().len(), 2);
        for _ in 0..4 {
            m.step(1, TabSwitch::Order);
            assert_ne!(m.state.active_id.as_deref(), Some(h.as_str()));
            m.step(1, TabSwitch::Recent);
            m.commit_walk();
            assert_ne!(m.state.active_id.as_deref(), Some(h.as_str()));
        }
        assert!(!m.jump(3), "Ctrl+3 has no third tab");
        m.pick(&h);
        assert_ne!(
            m.state.active_id.as_deref(),
            Some(h.as_str()),
            "a pick never fronts it"
        );
        assert!(m.is_hidden(&h) && !m.is_hidden(&a));
        let hd = m.hidden_of(&h).expect("hidden");
        assert_eq!(
            (
                hd.opener.as_str(),
                hd.open_id.as_deref(),
                hd.size,
                hd.since_ms
            ),
            ("Mac Mini", Some("o-1"), Some((90, 30)), 5)
        );
        assert!(m.set_cwd(&h, r"C:\h\sub"));
        assert_eq!(m.hidden()[0].tab.cwd, r"C:\h\sub");
        assert_eq!(m.hidden_labels(), ["sub"]);
        assert!(m.set_hidden_size(&h, 100, 40));
        assert!(
            !m.set_hidden_size(&h, 100, 40),
            "the same size is no change"
        );
        assert!(m.set_hidden_opener(&h, "Mac Studio"));
        assert!(!m.close(&h), "a strip close never takes it");
        m.set_agent(&h, Some(AgentKind::Claude));
        m.set_session(&h, Some(SID));
        assert!(m.close_hidden(&h));
        assert!(m.hidden().is_empty());
        assert_eq!(m.agent_kind(&h), None, "its agent goes with it");
        assert_eq!(m.session(&h), None);
        assert!(!m.close_hidden(&a), "a strip tab is no hidden session");
    }

    /// Rule 1306: opening, renaming and restoring check an opener's name the same way. A name restore would refuse is
    /// kept as "another computer" at open and changes nothing at a rename, so a saved session always comes back hidden
    /// with the name it showed.
    #[test]
    fn an_opener_name_is_checked_the_same_at_open_rename_and_restore() {
        use crate::restore::{MAX_OPENER_NAME, OPENER_UNNAMED};
        let mut m = TabModel::empty(0);
        let long = "m".repeat(MAX_OPENER_NAME + 1);
        let most = "m".repeat(MAX_OPENER_NAME);
        let h = m.open_hidden(r"C:\h", &long, None, None, 0);
        assert_eq!(m.hidden_of(&h).unwrap().opener, OPENER_UNNAMED);
        let h2 = m.open_hidden(r"C:\h2", "Mac\u{7}Mini", None, None, 0);
        assert_eq!(m.hidden_of(&h2).unwrap().opener, OPENER_UNNAMED);
        let h3 = m.open_hidden(r"C:\h3", &most, None, None, 0);
        assert_eq!(m.hidden_of(&h3).unwrap().opener, most);
        assert!(!m.set_hidden_opener(&h3, &long));
        assert!(!m.set_hidden_opener(&h3, "Mac\nMini"));
        assert_eq!(m.hidden_of(&h3).unwrap().opener, most);
        assert!(m.set_hidden_opener(&h, " Mac Studio "));
        assert_eq!(m.hidden_of(&h).unwrap().opener, "Mac Studio");
        // What was kept comes back, all hidden.
        let back = parse_tabs(&tabs_json(&m.saved()));
        assert!(back.tabs.is_empty());
        let names: Vec<&str> = back.hidden.iter().map(|h| h.opener.as_str()).collect();
        assert_eq!(names, ["Mac Studio", OPENER_UNNAMED, most.as_str()]);
    }

    #[test]
    fn closing_the_front_tab_never_fronts_a_hidden_session() {
        let mut m = TabModel::empty(0);
        let a = m.open_tab(r"C:\a");
        let h = m.open_hidden(r"C:\h", "PC", None, None, 0);
        assert!(m.close(&a));
        assert_eq!(m.state.active_id, None, "no tab left in front");
        assert!(m.is_hidden(&h), "the hidden session goes on");
        m.toggle_settings();
        m.toggle_settings();
        assert_ne!(m.state.active_id.as_deref(), Some(h.as_str()));
    }

    /// Rule 1302: a hidden session is saved after the strip with its opener, open id, size, agent and session; the
    /// front counts strip tabs only; the file reads back hidden; settle restores it hidden with a new id.
    #[test]
    fn a_hidden_session_saves_and_restores_hidden_with_its_opener_and_open_id() {
        let mut m = TabModel::empty(0);
        let a = m.open_tab(r"C:\a");
        let h = m.open_hidden(r"C:\h", "Mac Mini", Some("o-1"), Some((90, 30)), 0);
        m.set_agent(&h, Some(AgentKind::Claude));
        m.set_session(&h, Some(SID));
        m.pick(&a);
        let s = m.saved();
        assert_eq!(s.tabs.len(), 1);
        assert_eq!(s.active, 0);
        assert_eq!(
            s.hidden,
            vec![SavedHidden {
                cwd: r"C:\h".into(),
                agent: Some(AgentKind::Claude),
                session: Some(SID.into()),
                opener: "Mac Mini".into(),
                open_id: Some("o-1".into()),
                size: Some((90, 30)),
            }]
        );
        let back = parse_tabs(&tabs_json(&s));
        assert_eq!(back, s, "the file round-trips");
        let mut boot = TabModel::boot(&back, 1);
        assert_eq!(
            boot.state.tabs.len(),
            1,
            "no placeholder for a hidden session"
        );
        let spawn = boot.settle(&Restored {
            tabs: vec![RestoredTab {
                cwd: r"C:\a".into(),
                resume: None,
                from: Some(0),
                remote: None,
            }],
            active: 0,
            hidden: vec![RestoredHidden {
                cwd: r"C:\h".into(),
                resume: Some(SID.into()),
                opener: "Mac Mini".into(),
                open_id: Some("o-1".into()),
                size: Some((90, 30)),
            }],
        });
        assert_eq!(spawn.len(), 2);
        let hid = &spawn[1];
        assert!(boot.is_hidden(&hid.id), "restored hidden");
        assert_eq!(hid.resume.as_deref(), Some(SID));
        assert!(boot.state.tabs.iter().all(|t| t.id != hid.id));
        let hd = boot.hidden_of(&hid.id).unwrap();
        assert_eq!(
            (hd.open_id.as_deref(), hd.size),
            (Some("o-1"), Some((90, 30)))
        );
        assert_ne!(hid.id, h, "a new id, as every restored tab");
    }
}
