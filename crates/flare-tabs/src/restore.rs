//! SavedTabs, parse_tabs, plan_restore, peek_state, settle_restore, SessionLookup (Task 13).
//!
//! Sources: PT/src/main/{tabsStore,planRestore}.ts, PT/src/renderer/src/lib/restorePlan.ts.
//! The restore never types into a shell: a resume travels as the startup command (`Spawn::resume`).

use crate::tabs::{Pending, Tab, TabKind, TabState};
use flare_agent::kinds::{AgentKind, CODEX_RESUME};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Most tabs a saved strip keeps: a sanity cap on a hand-edited or damaged file, far above what anyone
/// opens (nothing in the strip or the model limits the tabs, so a lower cap lost tabs 51 and up, with
/// their resumes, at the next launch: review #11).
pub const MAX_TABS: usize = 500;

/// One saved tab: its folder and the resumable agent it hosted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedTab {
    pub cwd: String,
    pub agent: Option<AgentKind>,
}

/// The strip as `tabs.json` holds it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SavedTabs {
    pub tabs: Vec<SavedTab>,
    pub active: usize,
}

/// Defensive reader: anything not shaped right is an empty strip, never a crash.
pub fn parse_tabs(raw: &str) -> SavedTabs {
    let Ok(v) = serde_json::from_str::<Value>(raw) else {
        return SavedTabs::default();
    };
    let Some(list) = v.get("tabs").and_then(Value::as_array) else {
        return SavedTabs::default();
    };
    let tabs: Vec<SavedTab> = list
        .iter()
        .filter_map(|e| {
            let cwd = e.get("cwd")?.as_str()?;
            if cwd.is_empty() {
                return None;
            }
            let agent = e
                .get("agent")
                .and_then(Value::as_str)
                .and_then(AgentKind::parse);
            Some(SavedTab {
                cwd: cwd.to_string(),
                agent,
            })
        })
        .take(MAX_TABS)
        .collect();
    let active = v
        .get("active")
        .and_then(Value::as_f64)
        .filter(|a| a.fract() == 0.0 && *a >= 0.0 && *a < tabs.len() as f64)
        .map_or(0, |a| a as usize);
    SavedTabs { tabs, active }
}

/// `{"tabs":[{"cwd":..,"agent":..}],"active":n}`; the agent key is left out for a plain shell.
pub fn tabs_json(s: &SavedTabs) -> String {
    // Written by hand: the key order (cwd, then agent; tabs, then active) is part of the file shape.
    let quote = |t: &str| Value::String(t.to_string()).to_string();
    let tabs: Vec<String> = s
        .tabs
        .iter()
        .map(|t| match t.agent {
            Some(a) => format!(
                r#"{{"cwd":{},"agent":{}}}"#,
                quote(&t.cwd),
                quote(a.as_str())
            ),
            None => format!(r#"{{"cwd":{}}}"#, quote(&t.cwd)),
        })
        .collect();
    format!(r#"{{"tabs":[{}],"active":{}}}"#, tabs.join(","), s.active)
}

/// A tab that comes back: where, what to resume, and the saved tab it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoredTab {
    pub cwd: String,
    pub resume: Option<String>,
    pub from: Option<usize>,
}

/// What the restore decided.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Restored {
    pub tabs: Vec<RestoredTab>,
    pub active: usize,
}

/// The HOOK POINT for agent resume (M6): the Claude session ids of a folder, newest first.
/// M3 ships `NoSessions`; M6 swaps in the reader of Claude's transcript store.
pub trait SessionLookup: Send + Sync {
    fn sessions(&self, cwd: &str) -> Vec<String>;
}

/// The M3 lookup: no sessions, so every Claude tab comes back as a plain shell.
pub struct NoSessions;

impl SessionLookup for NoSessions {
    fn sessions(&self, _cwd: &str) -> Vec<String> {
        Vec::new()
    }
}

/// Security: a folder that is a UNC path or holds a control character is never statted or spawned
/// in on restore (Windows would connect over SMB and could leak the user's NTLM hash). An explicit
/// user action may take a tab there; a restore never does.
/// The test is flare-shells' own (`is_unc`, `has_control`, the rule behind `folder_after_report`),
/// so mixed separators (`\/host/share`, `/\host\share`) and the device forms that reach SMB
/// (`\\?\UNC\`, `\\.\UNC\`) count too.
pub fn is_unsafe_restore_folder(cwd: &str) -> bool {
    flare_shells::cwd::is_unc(cwd) || flare_shells::cwd::has_control(cwd)
}

/// Port of planRestore: drop gone folders, keep the active tab pointing right, hand Claude tabs of
/// one folder their sessions newest first.
pub fn plan_restore(
    saved: &SavedTabs,
    exists: &dyn Fn(&str) -> bool,
    sessions: &dyn Fn(&str) -> Vec<String>,
) -> Restored {
    let mut tabs: Vec<RestoredTab> = Vec::new();
    let mut active = 0usize;
    let mut taken: HashMap<String, usize> = HashMap::new();
    for (i, t) in saved.tabs.iter().enumerate() {
        if t.cwd.is_empty() || is_unsafe_restore_folder(&t.cwd) || !exists(&t.cwd) {
            continue;
        }
        if i == saved.active {
            active = tabs.len();
        }
        let mut resume = None;
        match t.agent {
            Some(AgentKind::Codex) => resume = Some(CODEX_RESUME.to_string()),
            Some(AgentKind::Claude) => {
                let n = taken.entry(t.cwd.to_lowercase()).or_insert(0);
                if let Some(id) = sessions(&t.cwd).get(*n) {
                    resume = Some(id.clone());
                    *n += 1;
                }
            }
            None => {}
        }
        tabs.push(RestoredTab {
            cwd: t.cwd.clone(),
            resume,
            from: Some(i),
        });
    }
    let active = active.min(tabs.len().saturating_sub(1));
    Restored { tabs, active }
}

/// Folders handed over by the launch join AFTER the restored tabs; the last is then in front.
pub fn with_launch_folders(plan: &mut Restored, folders: &[String]) {
    if folders.is_empty() {
        return;
    }
    for f in folders {
        plan.tabs.push(RestoredTab {
            cwd: f.clone(),
            resume: None,
            from: None,
        });
    }
    plan.active = plan.tabs.len() - 1;
}

/// How long a folder may take to answer "is it there": past this the restore stops WAITING for it, so
/// one slow folder (an offline mapped drive that Windows spends a minute reconnecting, a USB disk
/// spinning up) cannot hold every tab and every queued hand-off. The tab is KEPT (stored-state rules
/// 36-41: only a folder that answered "gone" drops its tab), and the next save still writes it.
pub const STAT_DEADLINE: Duration = Duration::from_secs(3);

/// BLOCKING (a worker thread): stats every saved folder in parallel, asks `lookup` only for Claude
/// tabs in a folder that exists, then `plan_restore`. A reported UNC folder is never statted, and a
/// folder that does not answer within `STAT_DEADLINE` is kept, not dropped.
pub fn resolve_restore(
    saved: &SavedTabs,
    is_dir: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    lookup: Arc<dyn SessionLookup>,
) -> Restored {
    resolve_restore_within(saved, is_dir, lookup, STAT_DEADLINE)
}

/// `resolve_restore` with the stat deadline made explicit.
pub fn resolve_restore_within(
    saved: &SavedTabs,
    is_dir: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    lookup: Arc<dyn SessionLookup>,
    deadline: Duration,
) -> Restored {
    let (tx, rx) = std::sync::mpsc::channel::<(String, bool)>();
    let mut asked = 0usize;
    let mut seen = std::collections::HashSet::new();
    for t in &saved.tabs {
        if t.cwd.is_empty() || is_unsafe_restore_folder(&t.cwd) || !seen.insert(t.cwd.clone()) {
            continue;
        }
        let (is_dir, tx, cwd) = (Arc::clone(&is_dir), tx.clone(), t.cwd.clone());
        // Detached: a stat that never returns is left behind, not waited for.
        let spawned = std::thread::Builder::new()
            .name("flare-restore-stat".into())
            .spawn(move || {
                let ok = is_dir(&cwd);
                let _ = tx.send((cwd, ok));
            });
        if spawned.is_ok() {
            asked += 1;
        }
    }
    drop(tx);
    let end = Instant::now() + deadline;
    let mut present: HashMap<String, bool> = HashMap::new();
    while present.len() < asked {
        let left = end.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            // sync-ok: the restore worker, bounded by the deadline
            Ok((cwd, ok)) => {
                present.insert(cwd, ok);
            }
            Err(_) => break,
        }
    }
    plan_restore(saved, &|p| present.get(p).copied().unwrap_or(true), &|p| {
        lookup.sessions(p)
    })
}

/// The first frame: the placeholders and the id of each saved tab's slot.
pub struct Boot {
    pub state: TabState,
    pub slots: Vec<String>,
}

/// TABS FROM THE FIRST FRAME: every saved tab a placeholder, the saved one in front.
pub fn peek_state(saved: &SavedTabs, new_id: &mut dyn FnMut() -> String) -> Boot {
    let mut tabs = Vec::new();
    let mut slots = Vec::new();
    for t in &saved.tabs {
        let id = new_id();
        slots.push(id.clone());
        tabs.push(Tab {
            id,
            kind: TabKind::Shell,
            cwd: t.cwd.clone(),
            pending: Some(if t.agent.is_some() {
                Pending::Agent
            } else {
                Pending::Shell
            }),
        });
    }
    let active_id = if slots.is_empty() {
        None
    } else {
        Some(slots[saved.active.min(slots.len() - 1)].clone())
    };
    Boot {
        state: TabState { tabs, active_id },
        slots,
    }
}

/// A shell to start for a restored tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spawn {
    pub id: String,
    pub cwd: String,
    pub resume: Option<String>,
}

/// The settled strip and the shells to start.
pub struct Settled {
    pub state: TabState,
    pub spawn: Vec<Spawn>,
}

/// Port of settleRestore: each restored tab takes over its placeholder in place; a tab the user
/// closed meanwhile stays closed and spawns nothing; a tab the user opened meanwhile is kept.
pub fn settle_restore(
    s: &TabState,
    slots: &[String],
    r: &Restored,
    new_id: &mut dyn FnMut() -> String,
) -> Settled {
    settle_restore_from(s, slots, None, r, new_id)
}

/// `settle_restore`, told which placeholder the boot put in front (`peek_state`): a placeholder in front
/// that is NOT that one was clicked by the owner during the skeleton phase, and the restore keeps it
/// in front (review #66). `None` is unknown: a pending front is then never taken for a pick.
pub fn settle_restore_from(
    s: &TabState,
    slots: &[String],
    boot_front: Option<&str>,
    r: &Restored,
    new_id: &mut dyn FnMut() -> String,
) -> Settled {
    let mut spawn = Vec::new();
    // The id each answered tab ended up with (None: closed meanwhile), in the answer's own order.
    let mut by_at: Vec<Option<String>> = Vec::with_capacity(r.tabs.len());
    // The tab at `r.active` in the answer's own order; a tab closed meanwhile leaves no id, so counting
    // the spawned ones would shift the front onto its neighbour (audit 2026-10-20, L18).
    let mut front_id: Option<String> = None;
    let mut claims: HashMap<String, String> = HashMap::new(); // slot id -> restored cwd
    let mut extras: Vec<Tab> = Vec::new();
    for (at, rt) in r.tabs.iter().enumerate() {
        let id = match rt.from {
            Some(f) => {
                let Some(slot_id) = slots.get(f) else {
                    by_at.push(None);
                    continue;
                };
                let still_pending = s
                    .tabs
                    .iter()
                    .any(|t| &t.id == slot_id && t.pending.is_some());
                if !still_pending {
                    by_at.push(None);
                    continue;
                }
                claims.insert(slot_id.clone(), rt.cwd.clone());
                slot_id.clone()
            }
            None => {
                let id = new_id();
                extras.push(Tab {
                    id: id.clone(),
                    kind: TabKind::Shell,
                    cwd: rt.cwd.clone(),
                    pending: None,
                });
                id
            }
        };
        if at == r.active {
            front_id = Some(id.clone());
        }
        by_at.push(Some(id.clone()));
        spawn.push(Spawn {
            id,
            cwd: rt.cwd.clone(),
            resume: rt.resume.clone(),
        });
    }
    let mut tabs: Vec<Tab> = Vec::new();
    for t in &s.tabs {
        if t.pending.is_none() {
            tabs.push(t.clone());
        } else if let Some(cwd) = claims.get(&t.id) {
            tabs.push(Tab {
                cwd: cwd.clone(),
                pending: None,
                ..t.clone()
            });
        }
    }
    tabs.extend(extras);
    // The front the owner chose: a tab of their own, or a placeholder they clicked while the strip was
    // still a skeleton (a pending front other than the one the boot picked).
    let picked = s.active_id.as_ref().filter(|a| {
        tabs.iter().any(|t| &t.id == *a)
            && s.tabs.iter().any(|t| {
                &t.id == *a && (t.pending.is_none() || boot_front.is_some_and(|b| b != a.as_str()))
            })
    });
    // Else the tab at the answer's `active`; if the owner closed it meanwhile, the next kept tab after
    // it, then the nearest before, never just the last of the strip.
    let near_front = || {
        by_at
            .iter()
            .skip(r.active + 1)
            .chain(by_at.iter().take(r.active).rev())
            .find_map(|x| x.clone())
    };
    let active_id = picked
        .cloned()
        .or(front_id)
        .or_else(near_front)
        .or_else(|| tabs.last().map(|t| t.id.clone()));
    Settled {
        state: TabState { tabs, active_id },
        spawn,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    fn ids() -> impl FnMut() -> String {
        let mut n = 0;
        move || {
            n += 1;
            format!("t{n}")
        }
    }
    fn all(_: &str) -> bool {
        true
    }
    fn st(cwd: &str, agent: Option<AgentKind>) -> SavedTab {
        SavedTab {
            cwd: cwd.into(),
            agent,
        }
    }
    fn rt(cwd: &str, resume: Option<&str>, from: Option<usize>) -> RestoredTab {
        RestoredTab {
            cwd: cwd.into(),
            resume: resume.map(String::from),
            from,
        }
    }
    fn no_sessions(_: &str) -> Vec<String> {
        vec![]
    }

    #[test]
    fn parse_keeps_only_well_shaped_tabs() {
        let raw = r#"{"tabs":[{"cwd":"C:\\a","agent":"claude"},{"x":1},{"cwd":"C:\\b","agent":"vim"}],"active":9}"#;
        let p = parse_tabs(raw);
        assert_eq!(
            p.tabs,
            vec![st("C:\\a", Some(AgentKind::Claude)), st("C:\\b", None)]
        );
        assert_eq!(p.active, 0);
        assert_eq!(parse_tabs("not json"), SavedTabs::default());
    }

    #[test]
    fn parse_reads_foreign_files_as_empty() {
        for raw in [
            "null",
            "[]",
            "{}",
            r#"{"tabs":"no"}"#,
            r#"{"tabs":[null,1,"a",{"cwd":5},{"cwd":""}]}"#,
        ] {
            assert_eq!(parse_tabs(raw), SavedTabs::default(), "{raw}");
        }
    }

    #[test]
    fn parse_keeps_the_active_index_only_when_it_names_a_tab() {
        let mk = |a: &str| {
            parse_tabs(&format!(
                r#"{{"tabs":[{{"cwd":"C:\\a"}},{{"cwd":"C:\\b"}}],"active":{a}}}"#
            ))
            .active
        };
        assert_eq!(mk("1"), 1);
        assert_eq!(mk("2"), 0);
        assert_eq!(mk("-1"), 0);
        assert_eq!(mk("0.5"), 0);
        assert_eq!(mk(r#""1""#), 0);
        assert_eq!(mk("1.0"), 1);
    }

    #[test]
    fn parse_caps_a_runaway_list_at_the_sanity_cap() {
        let items: Vec<String> = (0..MAX_TABS + 30)
            .map(|i| format!(r#"{{"cwd":"C:\\d{i}"}}"#))
            .collect();
        let raw = format!(r#"{{"tabs":[{}],"active":0}}"#, items.join(","));
        assert_eq!(parse_tabs(&raw).tabs.len(), MAX_TABS);
    }

    #[test]
    fn the_file_shape_round_trips() {
        let s = SavedTabs {
            tabs: vec![st("C:\\a", Some(AgentKind::Claude)), st("C:\\b", None)],
            active: 1,
        };
        let json = tabs_json(&s);
        assert_eq!(
            json,
            r#"{"tabs":[{"cwd":"C:\\a","agent":"claude"},{"cwd":"C:\\b"}],"active":1}"#
        );
        assert_eq!(parse_tabs(&json), s);
    }

    #[test]
    fn drops_a_gone_folder_and_keeps_the_active_tab_pointing_right() {
        let saved = SavedTabs {
            tabs: vec![st("C:\\gone", None), st("C:\\a", None), st("C:\\b", None)],
            active: 2,
        };
        let r = plan_restore(&saved, &|p| p != "C:\\gone", &no_sessions);
        assert_eq!(
            r.tabs,
            vec![rt("C:\\a", None, Some(1)), rt("C:\\b", None, Some(2))]
        );
        assert_eq!(r.active, 1);
    }

    #[test]
    fn falls_back_to_the_first_tab_when_the_active_one_went() {
        let saved = SavedTabs {
            tabs: vec![st("C:\\a", None), st("C:\\gone", None)],
            active: 1,
        };
        let r = plan_restore(&saved, &|p| p != "C:\\gone", &no_sessions);
        assert_eq!(r.tabs, vec![rt("C:\\a", None, Some(0))]);
        assert_eq!(r.active, 0);
    }

    #[test]
    fn an_empty_save_or_all_gone_is_an_empty_strip() {
        let empty = Restored::default();
        assert_eq!(
            plan_restore(&SavedTabs::default(), &all, &no_sessions),
            empty
        );
        let gone = SavedTabs {
            tabs: vec![st("C:\\gone", None)],
            active: 0,
        };
        assert_eq!(plan_restore(&gone, &|_| false, &no_sessions), empty);
    }

    #[test]
    fn two_claude_tabs_in_one_folder_take_newest_and_second_newest() {
        let saved = SavedTabs {
            tabs: vec![
                st("C:\\a", Some(AgentKind::Claude)),
                st("c:\\A", Some(AgentKind::Claude)),
            ],
            active: 0,
        };
        let r = plan_restore(&saved, &all, &|_| vec!["new-id".into(), "old-id".into()]);
        let got: Vec<_> = r.tabs.iter().map(|t| t.resume.clone()).collect();
        assert_eq!(
            got,
            vec![Some("new-id".to_string()), Some("old-id".to_string())]
        );
    }

    #[test]
    fn sessions_are_looked_up_by_the_tabs_own_folder() {
        let saved = SavedTabs {
            tabs: vec![
                st("C:\\a", Some(AgentKind::Claude)),
                st("C:\\a\\sub", Some(AgentKind::Claude)),
            ],
            active: 0,
        };
        let asked = Mutex::new(Vec::<String>::new());
        plan_restore(&saved, &all, &|p| {
            asked.lock().unwrap().push(p.to_string());
            vec![]
        });
        assert_eq!(*asked.lock().unwrap(), vec!["C:\\a", "C:\\a\\sub"]);
    }

    #[test]
    fn no_session_means_a_plain_shell() {
        let saved = SavedTabs {
            tabs: vec![st("C:\\a", Some(AgentKind::Claude))],
            active: 0,
        };
        let r = plan_restore(&saved, &all, &no_sessions);
        assert_eq!(r.tabs, vec![rt("C:\\a", None, Some(0))]);
    }

    #[test]
    fn a_tab_without_an_agent_never_resumes() {
        let saved = SavedTabs {
            tabs: vec![st("C:\\a", None)],
            active: 0,
        };
        let r = plan_restore(&saved, &all, &|_| vec!["new-id".into()]);
        assert_eq!(r.tabs, vec![rt("C:\\a", None, Some(0))]);
    }

    #[test]
    fn codex_gets_its_own_marker() {
        let saved = SavedTabs {
            tabs: vec![st("C:\\a", Some(AgentKind::Codex))],
            active: 0,
        };
        let r = plan_restore(&saved, &all, &|_| panic!("codex must not ask for sessions"));
        assert_eq!(r.tabs[0].resume.as_deref(), Some("codex:last"));
    }

    #[test]
    fn launch_folders_join_after_the_restored_tabs_and_the_last_is_in_front() {
        let mut plan = Restored {
            tabs: vec![rt("C:\\a", None, Some(0)), rt("C:\\b", None, Some(1))],
            active: 0,
        };
        let unchanged = plan.clone();
        with_launch_folders(&mut plan, &[]);
        assert_eq!(plan, unchanged);
        with_launch_folders(&mut plan, &["D:\\x".into(), "D:\\y".into()]);
        assert_eq!(plan.tabs.len(), 4);
        assert_eq!(plan.tabs[2], rt("D:\\x", None, None));
        assert_eq!(plan.tabs[3], rt("D:\\y", None, None));
        assert_eq!(plan.active, 3);
    }

    struct Asked(Mutex<Vec<String>>);
    impl SessionLookup for Asked {
        fn sessions(&self, cwd: &str) -> Vec<String> {
            self.0.lock().unwrap().push(cwd.to_string());
            vec!["sess".into()]
        }
    }

    #[test]
    fn resolve_restore_stats_in_parallel_and_asks_only_for_claude_tabs_in_present_folders() {
        let saved = SavedTabs {
            tabs: vec![
                st("C:\\a", Some(AgentKind::Claude)),
                st("C:\\gone", Some(AgentKind::Claude)),
                st("C:\\b", None),
            ],
            active: 0,
        };
        let now = Arc::new(AtomicUsize::new(0));
        let max = Arc::new(AtomicUsize::new(0));
        let (n2, m2) = (Arc::clone(&now), Arc::clone(&max));
        let is_dir: Arc<dyn Fn(&str) -> bool + Send + Sync> = Arc::new(move |p| {
            let c = n2.fetch_add(1, Ordering::SeqCst) + 1;
            m2.fetch_max(c, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(100));
            n2.fetch_sub(1, Ordering::SeqCst);
            p != "C:\\gone"
        });
        let lookup = Arc::new(Asked(Mutex::new(vec![])));
        let r = resolve_restore(&saved, is_dir, lookup.clone());
        assert!(max.load(Ordering::SeqCst) >= 2);
        assert_eq!(*lookup.0.lock().unwrap(), vec!["C:\\a"]);
        assert_eq!(
            r.tabs,
            vec![
                rt("C:\\a", Some("sess"), Some(0)),
                rt("C:\\b", None, Some(2))
            ]
        );
    }

    #[test]
    fn one_folder_that_never_answers_does_not_hold_the_rest_and_keeps_its_tab() {
        let saved = SavedTabs {
            tabs: vec![st("Z:\\nas", None), st("C:\\a", None), st("C:\\b", None)],
            active: 0,
        };
        let release = Arc::new(AtomicBool::new(false));
        let r2 = Arc::clone(&release);
        let is_dir: Arc<dyn Fn(&str) -> bool + Send + Sync> = Arc::new(move |p| {
            if p == "Z:\\nas" {
                while !r2.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            true
        });
        let t = Instant::now();
        let r = resolve_restore_within(
            &saved,
            is_dir,
            Arc::new(NoSessions),
            Duration::from_millis(300),
        );
        let took = t.elapsed();
        release.store(true, Ordering::SeqCst);
        assert!(took < Duration::from_secs(2), "held for {took:?}");
        // The slow folder keeps its tab (and so its place in tabs.json); only a "gone" answer drops one.
        assert_eq!(
            r.tabs,
            vec![
                rt("Z:\\nas", None, Some(0)),
                rt("C:\\a", None, Some(1)),
                rt("C:\\b", None, Some(2))
            ]
        );
    }

    #[test]
    fn restore_never_spawns_in_a_reported_unc_folder() {
        let saved = SavedTabs {
            tabs: vec![
                st("\\\\evil\\share", None),
                st("//evil/share", None),
                st("C:\\bad\u{7}dir", None),
                st("\\/evil/share", None),
                st("/\\evil\\share", None),
                st("\\\\?\\UNC\\evil\\share", None),
                st("\\\\.\\C:\\..\\UNC\\h\\s", None),
                st("//./C:/../UNC/h/s", None),
                st("C:\\c1\u{85}dir", None),
                st("C:\\ok", None),
            ],
            active: 0,
        };
        let r = plan_restore(&saved, &all, &no_sessions);
        assert_eq!(r.tabs, vec![rt("C:\\ok", None, Some(9))]);
        // A device path naming a local drive is a local folder.
        assert!(!is_unsafe_restore_folder("\\\\?\\C:\\work"));
        // The parallel resolver never even stats them (a stat over SMB leaks the hash).
        let statted = Arc::new(Mutex::new(Vec::<String>::new()));
        let s2 = Arc::clone(&statted);
        let is_dir: Arc<dyn Fn(&str) -> bool + Send + Sync> = Arc::new(move |p| {
            s2.lock().unwrap().push(p.to_string());
            true
        });
        let r = resolve_restore(&saved, is_dir, Arc::new(NoSessions));
        assert_eq!(*statted.lock().unwrap(), vec!["C:\\ok"]);
        assert_eq!(r.tabs.len(), 1);
    }

    fn boot3() -> Boot {
        let saved = SavedTabs {
            tabs: vec![
                st("C:\\a", Some(AgentKind::Claude)),
                st("C:\\b", None),
                st("C:\\c", Some(AgentKind::Codex)),
            ],
            active: 2,
        };
        peek_state(&saved, &mut ids())
    }

    #[test]
    fn peek_draws_every_saved_tab_as_a_placeholder() {
        let b = boot3();
        let got: Vec<_> = b
            .state
            .tabs
            .iter()
            .map(|t| (t.id.as_str(), t.cwd.as_str(), t.pending))
            .collect();
        assert_eq!(
            got,
            vec![
                ("t1", "C:\\a", Some(Pending::Agent)),
                ("t2", "C:\\b", Some(Pending::Shell)),
                ("t3", "C:\\c", Some(Pending::Agent)),
            ]
        );
        assert_eq!(b.state.active_id.as_deref(), Some("t3"));
        assert_eq!(b.slots, vec!["t1", "t2", "t3"]);
    }

    #[test]
    fn peek_of_an_empty_save_is_empty() {
        let b = peek_state(&SavedTabs::default(), &mut ids());
        assert_eq!(
            b.state,
            TabState {
                tabs: vec![],
                active_id: None
            }
        );
        assert!(b.slots.is_empty());
    }

    fn boot_abc() -> Boot {
        let saved = SavedTabs {
            tabs: vec![
                st("C:\\a", Some(AgentKind::Claude)),
                st("C:\\gone", None),
                st("C:\\c", None),
            ],
            active: 0,
        };
        peek_state(&saved, &mut ids())
    }

    fn tab_ids(s: &TabState) -> Vec<&str> {
        s.tabs.iter().map(|t| t.id.as_str()).collect()
    }

    #[test]
    fn settle_takes_over_placeholders_in_place() {
        let b = boot_abc();
        let r = Restored {
            tabs: vec![
                rt("C:\\a", Some("sess-1"), Some(0)),
                rt("C:\\c", None, Some(2)),
            ],
            active: 0,
        };
        let mut n = ids();
        let s = settle_restore(&b.state, &b.slots, &r, &mut n);
        assert_eq!(tab_ids(&s.state), vec!["t1", "t3"]);
        assert!(s.state.tabs.iter().all(|t| t.pending.is_none()));
        assert_eq!(s.state.tabs[0].cwd, "C:\\a");
        assert_eq!(s.state.active_id.as_deref(), Some("t1"));
        assert_eq!(
            s.spawn,
            vec![
                Spawn {
                    id: "t1".into(),
                    cwd: "C:\\a".into(),
                    resume: Some("sess-1".into())
                },
                Spawn {
                    id: "t3".into(),
                    cwd: "C:\\c".into(),
                    resume: None
                },
            ]
        );
    }

    #[test]
    fn a_handed_folder_comes_after_and_can_be_in_front() {
        let b = boot_abc();
        let r = Restored {
            tabs: vec![
                rt("C:\\a", None, Some(0)),
                rt("C:\\c", None, Some(2)),
                rt("D:\\handed", None, None),
            ],
            active: 2,
        };
        let mut n = || "new".to_string();
        let s = settle_restore(&b.state, &b.slots, &r, &mut n);
        assert_eq!(tab_ids(&s.state), vec!["t1", "t3", "new"]);
        assert_eq!(s.state.active_id.as_deref(), Some("new"));
        assert_eq!(
            s.spawn.last().unwrap(),
            &Spawn {
                id: "new".into(),
                cwd: "D:\\handed".into(),
                resume: None
            }
        );
    }

    #[test]
    fn a_tab_opened_meanwhile_is_kept_and_its_front_wins() {
        let b = boot_abc();
        let mut state = b.state.clone();
        state.tabs.push(Tab {
            id: "mine".into(),
            kind: TabKind::Shell,
            cwd: "C:\\x".into(),
            pending: None,
        });
        state.active_id = Some("mine".into());
        let r = Restored {
            tabs: vec![rt("C:\\a", None, Some(0))],
            active: 0,
        };
        let s = settle_restore(&state, &b.slots, &r, &mut ids());
        assert_eq!(tab_ids(&s.state), vec!["t1", "mine"]);
        assert_eq!(s.state.active_id.as_deref(), Some("mine"));
    }

    #[test]
    fn a_placeholder_closed_before_the_answer_stays_closed() {
        let b = boot_abc();
        let mut state = b.state.clone();
        state.tabs.retain(|t| t.id != "t1");
        state.active_id = Some("t3".into());
        let r = Restored {
            tabs: vec![rt("C:\\a", Some("x"), Some(0)), rt("C:\\c", None, Some(2))],
            active: 0,
        };
        let s = settle_restore(&state, &b.slots, &r, &mut ids());
        assert_eq!(tab_ids(&s.state), vec!["t3"]);
        let spawned: Vec<_> = s.spawn.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(spawned, vec!["t3"]);
    }

    #[test]
    fn the_front_tab_is_the_one_at_active_even_when_an_earlier_placeholder_was_closed() {
        let b = boot_abc();
        let mut state = b.state.clone();
        state.tabs.retain(|t| t.id != "t1");
        state.active_id = None;
        let r = Restored {
            tabs: vec![
                rt("C:\\a", None, Some(0)),
                rt("C:\\c", None, Some(2)),
                rt("D:\\handed", None, None),
            ],
            active: 1,
        };
        let mut n = || "new".to_string();
        let s = settle_restore(&state, &b.slots, &r, &mut n);
        assert_eq!(tab_ids(&s.state), vec!["t3", "new"]);
        assert_eq!(s.state.active_id.as_deref(), Some("t3"));
    }

    fn boot_all_three() -> (Boot, Restored) {
        let b = boot_abc();
        let r = Restored {
            tabs: vec![
                rt(r"C:\a", None, Some(0)),
                rt(r"C:\b", None, Some(1)),
                rt(r"C:\c", None, Some(2)),
            ],
            active: 0,
        };
        (b, r)
    }

    #[test]
    fn a_placeholder_the_owner_clicked_during_the_skeleton_stays_in_front() {
        let (b, r) = boot_all_three();
        let mut state = b.state.clone();
        // The boot fronted t1; the owner clicked the placeholder t2 before the answer.
        state.active_id = Some("t2".into());
        let s = settle_restore_from(&state, &b.slots, Some("t1"), &r, &mut ids());
        assert_eq!(s.state.active_id.as_deref(), Some("t2"));
        // Left alone, the boot front is simply the front the answer names.
        let s = settle_restore_from(&b.state, &b.slots, Some("t1"), &r, &mut ids());
        assert_eq!(s.state.active_id.as_deref(), Some("t1"));
    }

    #[test]
    fn a_launch_folder_still_fronts_over_the_untouched_boot_placeholder() {
        let (b, mut r) = boot_all_three();
        r.tabs.push(rt(r"D:\handed", None, None));
        r.active = 3;
        let mut n = || "new".to_string();
        let s = settle_restore_from(&b.state, &b.slots, Some("t1"), &r, &mut n);
        assert_eq!(s.state.active_id.as_deref(), Some("new"));
    }

    #[test]
    fn a_front_closed_before_the_answer_falls_to_the_next_kept_tab_not_the_last() {
        let (b, r) = boot_all_three();
        let mut state = b.state.clone();
        state.tabs.retain(|t| t.id != "t1");
        state.active_id = None;
        let s = settle_restore_from(&state, &b.slots, Some("t1"), &r, &mut ids());
        assert_eq!(tab_ids(&s.state), vec!["t2", "t3"]);
        assert_eq!(s.state.active_id.as_deref(), Some("t2"));
    }

    #[test]
    fn a_strip_of_more_than_fifty_tabs_is_saved_and_read_whole() {
        let tabs: Vec<SavedTab> = (0..120)
            .map(|i| SavedTab {
                cwd: format!(r"C:\w{i}"),
                agent: None,
            })
            .collect();
        let json = tabs_json(&SavedTabs { tabs, active: 119 });
        let back = parse_tabs(&json);
        assert_eq!(
            back.tabs.len(),
            120,
            "tabs 51 and up must survive a restart"
        );
        assert_eq!(back.active, 119);
    }

    #[test]
    fn every_folder_gone_leaves_nothing() {
        let b = boot_abc();
        let s = settle_restore(&b.state, &b.slots, &Restored::default(), &mut ids());
        assert!(s.state.tabs.is_empty());
        assert_eq!(s.state.active_id, None);
        assert!(s.spawn.is_empty());
    }
}
