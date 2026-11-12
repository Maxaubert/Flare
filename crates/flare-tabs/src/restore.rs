//! SavedTabs, parse_tabs, plan_restore, peek_state, settle_restore, SessionLookup (Task 13).
//!
//! Sources: PT/src/main/{tabsStore,planRestore}.ts, PT/src/renderer/src/lib/restorePlan.ts.
//! The restore never types into a shell: a resume travels as the startup command (`Spawn::resume`).

use crate::tabs::{MachineTag, Pending, Tab, TabKind, TabState};
use flare_agent::kinds::{AGY_RESUME, AgentKind, CODEX_RESUME};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Most tabs a saved strip keeps: a sanity cap on a hand-edited or damaged file, far above what anyone
/// opens (nothing in the strip or the model limits the tabs, so a lower cap lost tabs 51 and up, with
/// their resumes, at the next launch: review #11).
pub const MAX_TABS: usize = 500;

/// The longest machine id or host tab id a saved remote tab keeps.
const REMOTE_ID_MAX: usize = 128;

/// One saved tab: its folder and the resumable agent it hosted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedTab {
    pub cwd: String,
    pub agent: Option<AgentKind>,
    /// Some for a remote tab (remote sessions PR 3): `cwd` is then the host's folder and `agent` is None (the host
    /// owns the session and its resume).
    pub remote: Option<SavedRemote>,
}

/// A remote tab as `tabs.json` keeps it: `remote: {machine, host_tab, last_seq}`.
#[derive(Clone, Debug, Eq)]
pub struct SavedRemote {
    /// The machine's id in `machines.json`.
    pub machine: String,
    /// The host's id of the tab, once the host opened it: the restore attaches to it again (rule 288). None: the host
    /// never answered, and the restore opens a new one in the folder.
    pub host_tab: Option<String>,
    /// The host's output number the tab had read.
    pub last_seq: u64,
}

/// `last_seq` is no reason to write `tabs.json` again (it moves with every poll): it is written whenever the file is.
impl PartialEq for SavedRemote {
    fn eq(&self, other: &Self) -> bool {
        self.machine == other.machine && self.host_tab == other.host_tab
    }
}

/// An id the file may hold: printable, short, no space or control character.
fn remote_id(v: Option<&Value>) -> Option<String> {
    let s = v?.as_str()?;
    let ok = !s.is_empty()
        && s.len() <= REMOTE_ID_MAX
        && s.chars()
            .all(|c| c.is_ascii_graphic() || (!c.is_ascii() && !c.is_control()));
    ok.then(|| s.to_string())
}

/// The `remote` object of a saved tab; anything not shaped right reads as None (rule 40), so the tab comes back as
/// a local one in that folder if the folder is here, and is dropped if it is not.
fn parse_remote(v: Option<&Value>) -> Option<SavedRemote> {
    let o = v?.as_object()?;
    let machine = remote_id(o.get("machine"))?;
    let host_tab = match o.get("host_tab") {
        None | Some(Value::Null) => None,
        Some(t) => Some(remote_id(Some(t))?),
    };
    let last_seq = match o.get("last_seq") {
        None | Some(Value::Null) => 0,
        Some(n) => n.as_u64()?,
    };
    Some(SavedRemote {
        machine,
        host_tab,
        last_seq,
    })
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
            let remote = parse_remote(e.get("remote"));
            // The host owns a remote tab's session: no agent of this machine resumes it.
            let agent = e
                .get("agent")
                .and_then(Value::as_str)
                .and_then(AgentKind::parse)
                .filter(|_| remote.is_none());
            Some(SavedTab {
                cwd: cwd.to_string(),
                agent,
                remote,
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

/// `{"tabs":[{"cwd":..,"agent":..}],"active":n}`; the agent key is left out for a plain shell. A remote tab is
/// `{"cwd":..,"remote":{"machine":..,"host_tab":..,"last_seq":n}}` (`host_tab` left out until the host answered).
pub fn tabs_json(s: &SavedTabs) -> String {
    // Written by hand: the key order (cwd, then agent; tabs, then active) is part of the file shape.
    let quote = |t: &str| Value::String(t.to_string()).to_string();
    let tabs: Vec<String> = s
        .tabs
        .iter()
        .map(|t| match (&t.remote, t.agent) {
            (Some(r), _) => {
                let host = r
                    .host_tab
                    .as_deref()
                    .map(|h| format!(r#","host_tab":{}"#, quote(h)))
                    .unwrap_or_default();
                format!(
                    r#"{{"cwd":{},"remote":{{"machine":{}{},"last_seq":{}}}}}"#,
                    quote(&t.cwd),
                    quote(&r.machine),
                    host,
                    r.last_seq
                )
            }
            (None, Some(a)) => format!(
                r#"{{"cwd":{},"agent":{}}}"#,
                quote(&t.cwd),
                quote(a.as_str())
            ),
            (None, None) => format!(r#"{{"cwd":{}}}"#, quote(&t.cwd)),
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
    /// Some for a remote tab: it attaches to its host session again (rule 288).
    pub remote: Option<SavedRemote>,
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
/// (`\\?\UNC\`, `\\.\UNC\`) count too. On macOS the same rule (`is_remote`) refuses `//` paths, the
/// autofs mount points (`/net/<host>`, `/Network`) and a `.` or `..` segment.
pub fn is_unsafe_restore_folder(cwd: &str) -> bool {
    flare_shells::cwd::is_remote(cwd) || flare_shells::cwd::has_control(cwd)
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
        // A remote tab's folder is the host's: it is never statted here, and a path that would reach this machine's
        // network (a UNC path) is never used here either, since nothing here opens it (rule 287).
        let here = t.remote.is_none();
        if t.cwd.is_empty() || has_control(&t.cwd) || (here && !local_folder_ok(&t.cwd, exists)) {
            continue;
        }
        if i == saved.active {
            active = tabs.len();
        }
        let mut resume = None;
        match t.agent.filter(|_| here) {
            Some(AgentKind::Codex) => resume = Some(CODEX_RESUME.to_string()),
            Some(AgentKind::Agy) => resume = Some(AGY_RESUME.to_string()),
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
            remote: t.remote.clone(),
        });
    }
    let active = active.min(tabs.len().saturating_sub(1));
    Restored { tabs, active }
}

fn has_control(cwd: &str) -> bool {
    flare_shells::cwd::has_control(cwd)
}

/// A local folder the restore may use: safe (no UNC, rule above) and there.
fn local_folder_ok(cwd: &str, exists: &dyn Fn(&str) -> bool) -> bool {
    !is_unsafe_restore_folder(cwd) && exists(cwd)
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
            remote: None,
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
        // A remote tab's folder is the host's: never statted here.
        if t.remote.is_some()
            || t.cwd.is_empty()
            || is_unsafe_restore_folder(&t.cwd)
            || !seen.insert(t.cwd.clone())
        {
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
            remote: t.remote.as_ref().map(|r| MachineTag::of_id(&r.machine)),
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
    /// Some for a remote tab: it attaches to its host session again and is not new work (rule 288).
    pub remote: Option<SavedRemote>,
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
                    remote: rt.remote.as_ref().map(|r| MachineTag::of_id(&r.machine)),
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
            remote: rt.remote.clone(),
        });
    }
    let remote_of: HashMap<&str, Option<MachineTag>> = spawn
        .iter()
        .map(|s| {
            (
                s.id.as_str(),
                s.remote.as_ref().map(|r| MachineTag::of_id(&r.machine)),
            )
        })
        .collect();
    let mut tabs: Vec<Tab> = Vec::new();
    for t in &s.tabs {
        if t.pending.is_none() {
            tabs.push(t.clone());
        } else if let Some(cwd) = claims.get(&t.id) {
            tabs.push(Tab {
                cwd: cwd.clone(),
                pending: None,
                remote: remote_of.get(t.id.as_str()).cloned().flatten(),
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
            remote: None,
        }
    }
    fn rt(cwd: &str, resume: Option<&str>, from: Option<usize>) -> RestoredTab {
        RestoredTab {
            cwd: cwd.into(),
            resume: resume.map(String::from),
            from,
            remote: None,
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
    fn agy_gets_its_own_marker_and_its_word_round_trips() {
        let saved = SavedTabs {
            tabs: vec![st("C:\\a", Some(AgentKind::Agy))],
            active: 0,
        };
        let r = plan_restore(&saved, &all, &|_| panic!("agy must not ask for sessions"));
        assert_eq!(r.tabs[0].resume.as_deref(), Some("agy:last"));
        let p = parse_tabs(r#"{"tabs":[{"cwd":"C:\\a","agent":"agy"}],"active":0}"#);
        assert_eq!(p.tabs, vec![st("C:\\a", Some(AgentKind::Agy))]);
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

    #[cfg(windows)]
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
                    resume: Some("sess-1".into()),
                    remote: None,
                },
                Spawn {
                    id: "t3".into(),
                    cwd: "C:\\c".into(),
                    resume: None,
                    remote: None,
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
                resume: None,
                remote: None,
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
            remote: None,
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
                remote: None,
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

    #[cfg(not(windows))]
    #[test]
    fn restore_never_spawns_in_a_remote_folder_on_macos() {
        let saved = SavedTabs {
            tabs: vec![
                st("//evil/share", None),
                st("/net/evil/share", None),
                st("/NET/evil", None),
                st("/Network/Servers/evil/x", None),
                st("/tmp/../net/evil", None),
                st("/Users/me/bad\u{7}dir", None),
                st("/Users/me/c1\u{85}dir", None),
                st("/Users/me/ok", None),
            ],
            active: 0,
        };
        let r = plan_restore(&saved, &all, &no_sessions);
        assert_eq!(r.tabs, vec![rt("/Users/me/ok", None, Some(7))]);
        let statted = Arc::new(Mutex::new(Vec::<String>::new()));
        let s2 = Arc::clone(&statted);
        let is_dir: Arc<dyn Fn(&str) -> bool + Send + Sync> = Arc::new(move |p| {
            s2.lock().unwrap().push(p.to_string());
            true
        });
        let r = resolve_restore(&saved, is_dir, Arc::new(NoSessions));
        assert_eq!(*statted.lock().unwrap(), vec!["/Users/me/ok"]);
        assert_eq!(r.tabs.len(), 1);
    }

    #[cfg(not(windows))]
    #[test]
    fn posix_folders_round_trip_through_the_file_and_restore_from_real_folders() {
        let home = tempfile::tempdir().unwrap();
        let spaced = home.path().join("My Files").join("caf\u{e9} (1)");
        std::fs::create_dir_all(&spaced).unwrap();
        let spaced = spaced.to_string_lossy().into_owned();
        let gone = format!("{}/gone", home.path().to_string_lossy());
        let saved = SavedTabs {
            tabs: vec![
                st(&spaced, Some(AgentKind::Claude)),
                st(&gone, None),
                st("/", None),
            ],
            active: 2,
        };
        let back = parse_tabs(&tabs_json(&saved));
        assert_eq!(back, saved, "the file keeps every POSIX folder as it was");
        let is_dir: Arc<dyn Fn(&str) -> bool + Send + Sync> =
            Arc::new(|p| std::path::Path::new(p).is_dir());
        let r = resolve_restore(&back, is_dir, Arc::new(NoSessions));
        let cwds: Vec<&str> = r.tabs.iter().map(|t| t.cwd.as_str()).collect();
        assert_eq!(cwds, vec![spaced.as_str(), "/"]);
        assert_eq!(r.active, 1, "the front tab still points at /");
    }
}

/// Remote sessions PR 3: a remote tab in `tabs.json` and through the restore.
#[cfg(test)]
mod remote_tests {
    use super::*;
    use crate::tabs::Os;

    fn remote_tab(cwd: &str, host_tab: Option<&str>, seq: u64) -> SavedTab {
        SavedTab {
            cwd: cwd.into(),
            agent: None,
            remote: Some(SavedRemote {
                machine: "mac".into(),
                host_tab: host_tab.map(str::to_string),
                last_seq: seq,
            }),
        }
    }

    fn local(cwd: &str) -> SavedTab {
        SavedTab {
            cwd: cwd.into(),
            agent: Some(AgentKind::Claude),
            remote: None,
        }
    }

    #[test]
    fn a_remote_tab_round_trips_through_tabs_json_beside_a_local_one() {
        let saved = SavedTabs {
            tabs: vec![
                local(r"C:\a"),
                remote_tab("/Users/max/proj", Some("t19-3"), 4096),
                remote_tab("/Users/max/new", None, 0),
            ],
            active: 1,
        };
        let json = tabs_json(&saved);
        assert!(
            json.contains(
                r#"{"cwd":"/Users/max/proj","remote":{"machine":"mac","host_tab":"t19-3","last_seq":4096}}"#
            ),
            "{json}"
        );
        assert!(
            json.contains(r#"{"cwd":"C:\\a","agent":"claude"}"#),
            "{json}"
        );
        let back = parse_tabs(&json);
        assert_eq!(back, saved);
        assert_eq!(back.tabs[1].remote.as_ref().unwrap().last_seq, 4096);
        assert_eq!(back.tabs[2].remote.as_ref().unwrap().host_tab, None);
    }

    /// Rule 40: a `remote` that is not shaped right reads as a local tab; an agent saved beside a remote never resumes.
    #[test]
    fn a_broken_remote_reads_as_local_and_a_remote_never_resumes_an_agent() {
        for bad in [
            r#"{"machine":""}"#,
            r#"{"machine":3}"#,
            r#"{"machine":"mac","host_tab":5}"#,
            r#"{"machine":"mac","last_seq":-1}"#,
            r#"{"machine":"a b"}"#,
            r#""mac""#,
        ] {
            let raw = format!(r#"{{"tabs":[{{"cwd":"C:\\x","remote":{bad}}}],"active":0}}"#);
            let s = parse_tabs(&raw);
            assert_eq!(s.tabs.len(), 1, "{bad}");
            assert_eq!(s.tabs[0].remote, None, "{bad}");
        }
        let raw = r#"{"tabs":[{"cwd":"/p","agent":"claude","remote":{"machine":"mac","host_tab":"t1"}}],"active":0}"#;
        let s = parse_tabs(raw);
        assert_eq!(s.tabs[0].agent, None);
        assert_eq!(s.tabs[0].remote.as_ref().unwrap().last_seq, 0);
        assert_eq!(Os::parse("linux"), None);
        assert_eq!(Os::parse("mac"), Some(Os::Mac));
    }

    /// `last_seq` moving is no reason to write the file again.
    #[test]
    fn only_the_machine_and_the_host_tab_make_a_saved_remote_differ() {
        let a = remote_tab("/p", Some("t1"), 10);
        let b = remote_tab("/p", Some("t1"), 99);
        assert_eq!(a, b);
        assert_ne!(a, remote_tab("/p", Some("t2"), 10));
    }

    /// Rules 287, 288: the restore never stats a remote tab's folder, keeps it whatever this machine has at that path,
    /// and hands it back with its host tab to attach to, not as a resume.
    #[test]
    fn the_restore_keeps_a_remote_tab_without_a_stat_and_attaches_it() {
        let saved = SavedTabs {
            tabs: vec![
                remote_tab("/Users/max/proj", Some("t7"), 5),
                local(r"C:\gone"),
            ],
            active: 0,
        };
        let statted = std::sync::Mutex::new(Vec::<String>::new());
        let r = plan_restore(
            &saved,
            &|p| {
                statted.lock().unwrap().push(p.to_string());
                false
            },
            &|_| vec!["5a1d0c2e-1111-4222-8333-94445555a666".into()],
        );
        assert_eq!(*statted.lock().unwrap(), vec![r"C:\gone".to_string()]);
        assert_eq!(r.tabs.len(), 1);
        assert_eq!(r.tabs[0].resume, None);
        assert_eq!(
            r.tabs[0].remote.as_ref().and_then(|x| x.host_tab.clone()),
            Some("t7".into())
        );
        let mut k = 0;
        let boot = peek_state(&saved, &mut || {
            k += 1;
            format!("s{k}")
        });
        let mut n = 0;
        let settled = settle_restore(&boot.state, &boot.slots, &r, &mut || {
            n += 1;
            format!("x{n}")
        });
        assert_eq!(settled.spawn.len(), 1);
        assert!(settled.spawn[0].remote.is_some());
        let tab = &settled.state.tabs[0];
        assert_eq!(tab.kind, TabKind::Shell);
        assert_eq!(tab.remote.as_ref().map(|m| m.id.as_str()), Some("mac"));
    }
}
