//! Import remote sessions (rule 1290): which tabs of a linked machine's own Flare show here as remote tab rows.
//!
//! The machine's watch link (PR 3) asks `remote.status` every few seconds and hands each answer to [`import_plan`],
//! which is pure: it says which rows to add, which unattached rows to feed a status, which to remove and which hidden
//! ids the host no longer lists. It reads only entries that say `origin: here` (an older host says nothing and is
//! never imported from), never adds a host tab already shown here or hidden or being closed, skips the whole round
//! while an open there still waits for its host tab, and removes rows only on an answer: a refusal changes nothing, and
//! `unavailable` (no Flare runs there) removes unattached rows but never prunes the hidden list.
//!
//! [`HiddenList`] is "Hide here only" per machine (kept in `flare.machines.importHidden`), and [`ClosingList`] keeps a
//! row closed here out while the host still asks about the close (rule 48), until the host drops the tab or for
//! [`CLOSING_FOR`].

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use flare_ctl::Method;
use flare_ctl::remote::ORIGIN_HERE;
use flare_settings::machine_prefs::IMPORT_HIDDEN_MAX;
use flare_tabs::tabs::ImportHow;
use flare_term::remote_pty::HostStatus;
use serde_json::{Value, json};

use crate::remote_link::{Connector, Link, LinkError, next_backoff};
use crate::remote_status::status_from;

/// How long a row closed here stays out of the import while the host has not dropped its tab (builder's call 12).
pub const CLOSING_FOR: Duration = Duration::from_secs(120);

/// The longest host tab id read from a peer; a longer one, or one with any character but a letter, digit, `-` or `_`,
/// names no tab here (host ids look like `t<clock>-<n>`).
pub const HOST_TAB_MAX: usize = 64;

/// The longest host title kept, in characters (the agent's list shows it, import remote sessions PR 4).
pub const TITLE_MAX: usize = 200;

/// The folder a row takes when the host's folder is not a safe path: the host's home, as a remote tab opened there.
const HOME: &str = "~";

/// Pure: whether a peer's host tab id may name a row here or go in the hidden list.
pub fn host_tab_ok(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= HOST_TAB_MAX
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// What one poll of a machine heard.
#[derive(Clone, Copy, Debug)]
pub enum Answer<'a> {
    /// The `remote.status` answer. One without a `tabs` list counts as a refusal.
    Listed(&'a Value),
    /// No Flare runs there (`--no-start` relays nothing): every tab is gone, but a starting or busy host may answer it
    /// in passing, so the hidden list is not pruned.
    Unavailable,
    /// Any other failure (busy, timeout, a broken link): nothing changes.
    Refused,
}

impl Answer<'_> {
    /// Every well formed host tab id a real listing names, whatever its origin. None for anything but a listing.
    fn listed(&self) -> Option<HashSet<&str>> {
        let Answer::Listed(v) = self else {
            return None;
        };
        Some(
            v["tabs"]
                .as_array()?
                .iter()
                .filter_map(|t| t["tab"].as_str().filter(|id| host_tab_ok(id)))
                .collect(),
        )
    }
}

/// A remote tab row of the polled machine already shown here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    /// This window's tab id.
    pub id: String,
    /// The host's id of the tab; None while its open still waits for the host's answer.
    pub host_tab: Option<String>,
    /// How an imported or adopted row came; None for a tab opened from here or restored.
    pub how: Option<ImportHow>,
    /// The row has its own session link (always true for a tab opened from here).
    pub attached: bool,
}

impl Shown {
    fn unattached(&self) -> bool {
        self.how.is_some() && !self.attached
    }
}

/// A host tab to add here as an unattached row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportRow {
    pub host_tab: String,
    /// The host's folder, or `~` when the host names no safe one (rule 1279).
    pub folder: String,
    /// The host's title, words only, for the agent's list (a `Tab` has no title: labels come from the folder).
    #[allow(dead_code)] // the agent's list reads it (import remote sessions PR 4)
    pub title: String,
    /// The marks the row shows before it attaches.
    pub status: HostStatus,
}

/// What one round does to one machine's rows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportPlan {
    /// New rows, in the host's order, each behind the front tab.
    pub add: Vec<ImportRow>,
    /// Unattached rows still listed, with the status their marks take (this window's tab id first).
    pub update: Vec<(String, HostStatus)>,
    /// Unattached rows (imported or adopted) whose host tab is gone: they leave without a `remote.close`.
    pub remove: Vec<String>,
    /// Hidden host tab ids a real listing no longer names: the hidden list forgets them.
    pub hidden_gone: Vec<String>,
    /// The host lists tabs but says no `origin` (a Flare from before rule 1289): nothing is imported from it.
    pub older_host: bool,
}

/// One round's inputs for one machine.
pub struct Round<'a> {
    pub machine: &'a str,
    pub answer: Answer<'a>,
    /// The machine's remote tab rows shown here, of every kind.
    pub shown: &'a [Shown],
    pub hidden: &'a HiddenList,
    pub closing: &'a ClosingList,
    /// A tab here on this machine waits for `remote.open` to answer (no host tab yet, its link not ended): the whole
    /// round is skipped, or its new host tab would be imported twice.
    pub in_flight: bool,
    /// The setting is on. Off, an agent's adopted row keeps its machine polled: rows are fed and removed, none added.
    pub add_new: bool,
    pub now: Instant,
}

/// Pure (rule 1290): what one poll of `round.machine` changes here.
pub fn import_plan(round: &Round) -> ImportPlan {
    let mut plan = ImportPlan::default();
    if round.in_flight {
        return plan;
    }
    let unattached = || round.shown.iter().filter(|s| s.unattached());
    let entries = match round.answer {
        Answer::Refused => return plan,
        Answer::Unavailable => {
            plan.remove = unattached().map(|s| s.id.clone()).collect();
            return plan;
        }
        Answer::Listed(v) => match v["tabs"].as_array() {
            Some(e) => e,
            None => return plan,
        },
    };
    let named: Vec<(&str, &Value)> = entries
        .iter()
        .filter_map(|t| Some((t["tab"].as_str().filter(|id| host_tab_ok(id))?, t)))
        .collect();
    if !named.is_empty() && named.iter().all(|(_, t)| t["origin"].as_str().is_none()) {
        plan.older_host = true;
        return plan;
    }
    let listed: HashSet<&str> = named.iter().map(|(id, _)| *id).collect();
    plan.hidden_gone = round
        .hidden
        .ids(round.machine)
        .iter()
        .filter(|id| !listed.contains(id.as_str()))
        .cloned()
        .collect();
    let mut own: HashSet<&str> = HashSet::new();
    for (id, t) in named {
        if t["origin"].as_str() != Some(ORIGIN_HERE) || !own.insert(id) {
            continue;
        }
        let shown = round
            .shown
            .iter()
            .find(|s| s.host_tab.as_deref() == Some(id));
        let Some(status) = status_from(t) else {
            continue;
        };
        match shown {
            Some(s) if s.unattached() => plan.update.push((s.id.clone(), status)),
            Some(_) => {}
            None if !round.add_new
                || round.hidden.hides(round.machine, id)
                || round.closing.holds(round.machine, id, round.now) => {}
            None => plan.add.push(ImportRow {
                host_tab: id.to_string(),
                folder: status.folder.clone().unwrap_or_else(|| HOME.into()),
                title: title_of(&t["title"]),
                status,
            }),
        }
    }
    plan.remove = unattached()
        .filter(|s| !s.host_tab.as_deref().is_some_and(|h| own.contains(h)))
        .map(|s| s.id.clone())
        .collect();
    plan
}

/// The host's title as words: no control or invisible character, at most [`TITLE_MAX`] characters.
fn title_of(v: &Value) -> String {
    v.as_str()
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control() && !flare_ctl::text::invisible(*c))
        .take(TITLE_MAX)
        .collect::<String>()
        .trim()
        .to_string()
}

/// "Hide here only" per machine: host tab ids, oldest first, at most [`IMPORT_HIDDEN_MAX`] each. Kept in
/// `flare.machines.importHidden` (`machine_prefs::import_hidden`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HiddenList(BTreeMap<String, Vec<String>>);

impl HiddenList {
    /// The stored lists, keeping only well formed ids.
    pub fn from_map(m: BTreeMap<String, Vec<String>>) -> Self {
        Self(
            m.into_iter()
                .map(|(k, ids)| (k, ids.into_iter().filter(|i| host_tab_ok(i)).collect()))
                .filter(|(_, ids): &(String, Vec<String>)| !ids.is_empty())
                .collect(),
        )
    }

    pub fn as_map(&self) -> &BTreeMap<String, Vec<String>> {
        &self.0
    }

    pub fn ids(&self, machine: &str) -> &[String] {
        self.0.get(machine).map_or(&[], Vec::as_slice)
    }

    pub fn hides(&self, machine: &str, tab: &str) -> bool {
        self.ids(machine).iter().any(|t| t == tab)
    }

    /// Hides `tab` (it becomes the newest; past the cap the oldest drops). False for an id that is not well formed.
    pub fn hide(&mut self, machine: &str, tab: &str) -> bool {
        if !host_tab_ok(tab) {
            return false;
        }
        let ids = self.0.entry(machine.to_string()).or_default();
        ids.retain(|t| t != tab);
        ids.push(tab.to_string());
        if ids.len() > IMPORT_HIDDEN_MAX {
            ids.drain(..ids.len() - IMPORT_HIDDEN_MAX);
        }
        true
    }

    /// Takes `tab` off the list (an agent's adopt, PR 4). False when it was not hidden.
    #[allow(dead_code)] // the agent's adopt calls it (import remote sessions PR 4)
    pub fn unhide(&mut self, machine: &str, tab: &str) -> bool {
        self.forget(machine, &[tab.to_string()])
    }

    /// Forgets the ids a real listing no longer names ([`ImportPlan::hidden_gone`]). True when any went.
    pub fn forget(&mut self, machine: &str, gone: &[String]) -> bool {
        let Some(ids) = self.0.get_mut(machine) else {
            return false;
        };
        let before = ids.len();
        ids.retain(|t| !gone.contains(t));
        let changed = ids.len() != before;
        if ids.is_empty() {
            self.0.remove(machine);
        }
        changed
    }

    /// A machine removed from Settings drops its list.
    pub fn drop_machine(&mut self, machine: &str) -> bool {
        self.0.remove(machine).is_some()
    }
}

/// Rows closed here whose host may still be asking about the close (rule 48), kept out of the import until the host
/// drops the tab or [`CLOSING_FOR`] passed. If the person on the host keeps the tab, it comes back after that.
#[derive(Clone, Debug, Default)]
pub struct ClosingList(Vec<Closing>);

#[derive(Clone, Debug)]
struct Closing {
    machine: String,
    tab: String,
    since: Instant,
}

impl ClosingList {
    /// Close on any remote tab of `machine` whose host tab is `tab` (its id read before the row went).
    pub fn add(&mut self, machine: &str, tab: &str, now: Instant) {
        self.0.retain(|c| !(c.machine == machine && c.tab == tab));
        self.0.push(Closing {
            machine: machine.to_string(),
            tab: tab.to_string(),
            since: now,
        });
    }

    /// Whether the import still keeps `tab` out.
    pub fn holds(&self, machine: &str, tab: &str, now: Instant) -> bool {
        self.0.iter().any(|c| {
            c.machine == machine && c.tab == tab && now.duration_since(c.since) < CLOSING_FOR
        })
    }

    /// A poll of `machine` answered: entries past [`CLOSING_FOR`] lapse, and on a real listing so do the machine's
    /// entries it no longer names. `unavailable` or a refusal drops nothing early.
    pub fn heard(&mut self, machine: &str, answer: &Answer, now: Instant) {
        let listed = answer.listed();
        self.0.retain(|c| {
            now.duration_since(c.since) < CLOSING_FOR
                && (c.machine != machine
                    || listed.as_ref().is_none_or(|l| l.contains(c.tab.as_str())))
        });
    }

    /// A machine removed from Settings drops its entries.
    pub fn drop_machine(&mut self, machine: &str) {
        self.0.retain(|c| c.machine != machine);
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

// ---- the watch links (rule 1291) ----

/// What one poll of a machine heard, owned: the window's message.
#[derive(Clone)]
pub enum Heard {
    Listed(Value),
    Unavailable,
    Refused,
}

impl Heard {
    pub fn answer(&self) -> Answer<'_> {
        match self {
            Heard::Listed(v) => Answer::Listed(v),
            Heard::Unavailable => Answer::Unavailable,
            Heard::Refused => Answer::Refused,
        }
    }
}

/// A watch link's answer for the window: the machine and what its poll heard.
pub struct WatchAnswer {
    pub machine: String,
    pub heard: Heard,
}

impl std::fmt::Debug for WatchAnswer {
    /// Its kind only: a listing carries the host's titles, question words and session ids.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WatchAnswer")
    }
}

/// Where a watch link hands its answers (the window's post, a test's list).
pub type WatchPost = Arc<dyn Fn(WatchAnswer) + Send + Sync>;

/// One machine's watch while it lives. Dropping it stops the link (rule 1291: no watch while the setting is off).
pub trait Watch {
    /// Close on an unattached row (rule 1293): `remote.close {tab}` over this link, now or once it is up again.
    fn close(&self, host_tab: &str);
}

/// Starts watch links (the window's; a test gives a fake).
pub trait WatchPort {
    fn watch(&self, machine: &str, post: WatchPost) -> Box<dyn Watch>;
}

/// The waits of a watch link (rule 1291).
#[derive(Clone, Copy, Debug)]
pub struct WatchTiming {
    /// Between two `remote.status` polls.
    pub every: Duration,
    /// The first wait after the link failed; each next one doubles, up to `backoff_max`.
    pub backoff_min: Duration,
    pub backoff_max: Duration,
    /// One call's time.
    pub call: Duration,
}

impl Default for WatchTiming {
    fn default() -> Self {
        Self {
            every: Duration::from_secs(3),
            backoff_min: Duration::from_secs(3),
            backoff_max: Duration::from_secs(60),
            call: Duration::from_secs(15),
        }
    }
}

/// The watch links over a connector whose links run `--serve-remote --no-start` (`SshConnector::watching`).
pub struct LinkWatchPort {
    connector: Arc<dyn Connector>,
    timing: WatchTiming,
}

impl LinkWatchPort {
    pub fn new(connector: Arc<dyn Connector>, timing: WatchTiming) -> Self {
        Self { connector, timing }
    }
}

#[derive(Default)]
struct WatchState {
    stop: bool,
    /// Host tabs to close (rule 1293), sent in order once the link is up.
    closes: Vec<String>,
}

#[derive(Default)]
struct WatchShared {
    state: Mutex<WatchState>,
    cv: Condvar,
}

impl WatchShared {
    fn lock(&self) -> std::sync::MutexGuard<'_, WatchState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn stopped(&self) -> bool {
        self.lock().stop
    }

    /// Waits `d`, or until the watch stops, or (`for_close`) a close is asked. False once stopped.
    fn wait(&self, d: Duration, for_close: bool) -> bool {
        let end = Instant::now() + d;
        let mut st = self.lock();
        loop {
            if st.stop {
                return false;
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() || (for_close && !st.closes.is_empty()) {
                return true;
            }
            st = self
                .cv
                .wait_timeout(st, left) // sync-ok: a watch link's worker waits for its next poll, never the UI thread
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
    }
}

struct LinkWatch(Arc<WatchShared>);

impl Watch for LinkWatch {
    fn close(&self, host_tab: &str) {
        self.0.lock().closes.push(host_tab.to_string());
        self.0.cv.notify_all();
    }
}

impl Drop for LinkWatch {
    fn drop(&mut self) {
        self.0.lock().stop = true;
        self.0.cv.notify_all();
    }
}

impl WatchPort for LinkWatchPort {
    fn watch(&self, machine: &str, post: WatchPost) -> Box<dyn Watch> {
        let shared = Arc::new(WatchShared::default());
        let worker = WatchWorker {
            machine: machine.to_string(),
            connector: Arc::clone(&self.connector),
            t: self.timing,
            shared: Arc::clone(&shared),
            post,
        };
        if let Err(e) = std::thread::Builder::new()
            .name("flare-watch-link".into())
            .spawn(move || worker.run())
        {
            log::warn!("could not start the watch link of {machine}: {e}");
        }
        Box::new(LinkWatch(shared))
    }
}

/// Pure (rule 1291): what a watch link's `remote.status` call came to. `unavailable` is a host with no Flare running
/// (`--no-start`); any other refusal is passing. None for a broken link.
pub fn heard_of(r: Result<Value, LinkError>) -> Option<Heard> {
    match r {
        Ok(v) => Some(Heard::Listed(v)),
        Err(LinkError::Refused(e)) if e.code == flare_ctl::code::UNAVAILABLE => {
            Some(Heard::Unavailable)
        }
        Err(LinkError::Refused(e)) => {
            log::info!("remote.status refused on the watch link: {}", e.code);
            Some(Heard::Refused)
        }
        Err(LinkError::Down(_)) => None,
    }
}

struct WatchWorker {
    machine: String,
    connector: Arc<dyn Connector>,
    t: WatchTiming,
    shared: Arc<WatchShared>,
    post: WatchPost,
}

impl WatchWorker {
    fn tell(&self, heard: Heard) {
        if !self.shared.stopped() {
            (self.post)(WatchAnswer {
                machine: self.machine.clone(),
                heard,
            });
        }
    }

    /// BLOCKING (ssh, the polls): the watch's own thread until the window drops it.
    fn run(self) {
        let mut backoff = self.t.backoff_min;
        while !self.shared.stopped() {
            match self.connector.connect(&self.machine) {
                Ok(mut link) => {
                    backoff = self.t.backoff_min;
                    self.polls(&mut *link);
                }
                Err(e) => log::info!("watch link to {}: {e}", self.machine),
            }
            if !self.shared.wait(backoff, false) {
                break;
            }
            backoff = next_backoff(backoff, self.t.backoff_max);
        }
    }

    /// The polls of one link: the closes asked, then `remote.status`, every `every`, until the link breaks or stops.
    fn polls(&self, link: &mut dyn Link) {
        loop {
            let closes = std::mem::take(&mut self.shared.lock().closes);
            for (n, tab) in closes.iter().enumerate() {
                if let Err(LinkError::Down(_)) =
                    link.call(Method::RemoteClose, json!({"tab": tab}), self.t.call)
                {
                    // Tried again once the link is back (the spec's `close_without_a_link`), before any asked since.
                    let mut st = self.shared.lock();
                    let mut again = closes[n..].to_vec();
                    again.append(&mut st.closes);
                    st.closes = again;
                    return;
                }
            }
            let Some(heard) = heard_of(link.call(Method::RemoteStatus, json!({}), self.t.call))
            else {
                return;
            };
            self.tell(heard);
            if !self.shared.wait(self.t.every, true) {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(tab: &str) -> Value {
        json!({"tab": tab, "state": "idle", "folder": format!("/Users/max/{tab}"), "title": tab,
               "origin": ORIGIN_HERE})
    }

    fn listing(tabs: &[&str]) -> Value {
        json!({"tabs": tabs.iter().map(|t| entry(t)).collect::<Vec<_>>()})
    }

    fn row(id: &str, host_tab: Option<&str>, how: Option<ImportHow>, attached: bool) -> Shown {
        Shown {
            id: id.into(),
            host_tab: host_tab.map(str::to_string),
            how,
            attached,
        }
    }

    struct Fx {
        hidden: HiddenList,
        closing: ClosingList,
        now: Instant,
    }

    impl Fx {
        fn new() -> Self {
            Self {
                hidden: HiddenList::default(),
                closing: ClosingList::default(),
                now: Instant::now(),
            }
        }

        fn plan(&self, machine: &str, answer: Answer, shown: &[Shown]) -> ImportPlan {
            import_plan(&Round {
                machine,
                answer,
                shown,
                hidden: &self.hidden,
                closing: &self.closing,
                in_flight: false,
                add_new: true,
                now: self.now,
            })
        }
    }

    fn added(p: &ImportPlan) -> Vec<&str> {
        p.add.iter().map(|r| r.host_tab.as_str()).collect()
    }

    /// Rule 1290: only entries that say `origin: here` are imported; a host that names no origin at all is an older
    /// Flare, flagged, and nothing comes from it.
    #[test]
    fn import_plan_keeps_origin_here_only_and_skips_an_older_host() {
        let fx = Fx::new();
        let mixed = json!({"tabs": [
            entry("t1-1"),
            {"tab": "t1-2", "state": "idle", "folder": "/a", "origin": "viewer"},
            {"tab": "t1-3", "state": "idle", "folder": "/b"},
        ]});
        let p = fx.plan("mac", Answer::Listed(&mixed), &[]);
        assert_eq!(added(&p), ["t1-1"]);
        assert!(!p.older_host);
        let older = json!({"tabs": [
            {"tab": "t1-1", "state": "idle", "folder": "/a"},
            {"tab": "t1-2", "state": "working", "folder": "/b"},
        ]});
        let p = fx.plan(
            "mac",
            Answer::Listed(&older),
            &[row("x", Some("t1-9"), Some(ImportHow::Adopted), false)],
        );
        assert_eq!(
            p,
            ImportPlan {
                older_host: true,
                ..ImportPlan::default()
            },
            "an older host: nothing imported, nothing removed"
        );
        let empty = json!({"tabs": []});
        assert!(!fx.plan("mac", Answer::Listed(&empty), &[]).older_host);
    }

    /// Rule 1290: one row per (machine, host tab). A host tab shown here in any way (opened from here, restored,
    /// imported or adopted) is never added again, and one listed twice is added once.
    #[test]
    fn import_plan_makes_one_row_per_machine_and_host_tab() {
        let fx = Fx::new();
        let answer = json!({"tabs": [entry("t1-1"), entry("t1-2"), entry("t1-3"), entry("t1-4"),
                                      entry("t1-5"), entry("t1-6"), entry("t1-6")]});
        let shown = [
            row("opened", Some("t1-1"), None, true),
            row("restored", Some("t1-2"), None, true),
            row("imported", Some("t1-3"), Some(ImportHow::Imported), false),
            row("adopted", Some("t1-4"), Some(ImportHow::Adopted), true),
        ];
        let p = fx.plan("mac", Answer::Listed(&answer), &shown);
        assert_eq!(added(&p), ["t1-5", "t1-6"]);
        let r = &p.add[0];
        assert_eq!(r.folder, "/Users/max/t1-5");
        assert_eq!(r.title, "t1-5");
        assert_eq!(r.status.state, "idle");
        // The unattached row is fed its status; attached and opened rows feed themselves through their own link.
        let fed: Vec<&str> = p.update.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(fed, ["imported"]);
        assert!(p.remove.is_empty());
    }

    /// Rule 1290: a hidden host tab and one closed here while the host still asks are not added.
    #[test]
    fn import_plan_drops_hidden_and_closing_tabs() {
        let mut fx = Fx::new();
        assert!(fx.hidden.hide("mac", "t1-1"));
        fx.closing.add("mac", "t1-2", fx.now);
        let answer = listing(&["t1-1", "t1-2", "t1-3"]);
        let p = fx.plan("mac", Answer::Listed(&answer), &[]);
        assert_eq!(added(&p), ["t1-3"]);
        // Hidden or closing on another machine keeps nothing out here.
        let p = fx.plan("pc", Answer::Listed(&answer), &[]);
        assert_eq!(added(&p), ["t1-1", "t1-2", "t1-3"]);
        // Past the closing time the closed tab the host kept comes back.
        fx.now += CLOSING_FOR;
        let p = fx.plan("mac", Answer::Listed(&answer), &[]);
        assert_eq!(added(&p), ["t1-2", "t1-3"]);
    }

    /// Rule 1290: hidden ids go only when a real listing no longer names them; `unavailable`, a refusal and an answer
    /// with no list prune nothing.
    #[test]
    fn import_plan_prunes_hidden_ids_only_on_a_real_listing_never_on_unavailable() {
        let mut fx = Fx::new();
        fx.hidden.hide("mac", "t1-1");
        fx.hidden.hide("mac", "t1-2");
        let answer = listing(&["t1-2"]);
        let p = fx.plan("mac", Answer::Listed(&answer), &[]);
        assert_eq!(p.hidden_gone, ["t1-1"]);
        // A tab the host still lists under another origin is not pruned either: only one it no longer names.
        let other =
            json!({"tabs": [entry("t1-2"), {"tab": "t1-1", "state": "idle", "origin": "viewer"}]});
        assert!(
            fx.plan("mac", Answer::Listed(&other), &[])
                .hidden_gone
                .is_empty()
        );
        assert!(
            fx.plan("mac", Answer::Unavailable, &[])
                .hidden_gone
                .is_empty()
        );
        assert!(fx.plan("mac", Answer::Refused, &[]).hidden_gone.is_empty());
        assert!(
            fx.plan("mac", Answer::Listed(&json!({"error": "x"})), &[])
                .hidden_gone
                .is_empty()
        );
        let empty = json!({"tabs": []});
        assert_eq!(
            fx.plan("mac", Answer::Listed(&empty), &[]).hidden_gone,
            ["t1-1", "t1-2"]
        );
        assert!(fx.hidden.forget("mac", &p.hidden_gone));
        assert_eq!(fx.hidden.ids("mac"), ["t1-2"]);
        assert!(!fx.hidden.forget("mac", &p.hidden_gone), "already gone");
    }

    /// Rule 1290: while a tab here on the machine waits for its open's host tab, the whole round is skipped: its new
    /// host tab would be listed but not yet known here, so it would be imported twice.
    #[test]
    fn import_plan_skips_a_machine_while_an_open_there_waits_for_its_host_tab() {
        let fx = Fx::new();
        let answer = listing(&["t1-1"]);
        let shown = [
            row("opening", None, None, true),
            row("gone", Some("t1-7"), Some(ImportHow::Imported), false),
        ];
        let round = |in_flight| Round {
            machine: "mac",
            answer: Answer::Listed(&answer),
            shown: &shown,
            hidden: &fx.hidden,
            closing: &fx.closing,
            in_flight,
            add_new: true,
            now: fx.now,
        };
        assert_eq!(import_plan(&round(true)), ImportPlan::default());
        let p = import_plan(&round(false));
        assert_eq!(added(&p), ["t1-1"]);
        assert_eq!(p.remove, ["gone"]);
    }

    /// Rule 1290: a round removes only unattached imported or adopted rows whose host tab the answer no longer lists;
    /// `unavailable` removes every unattached row; a refusal or an answer with no list removes nothing. Attached rows
    /// and tabs opened from here end through their own link.
    #[test]
    fn import_plan_removes_only_unattached_imported_rows_and_never_on_a_refusal() {
        let fx = Fx::new();
        let shown = [
            row("opened", Some("t1-1"), None, true),
            row("attached", Some("t1-2"), Some(ImportHow::Imported), true),
            row("idle", Some("t1-3"), Some(ImportHow::Imported), false),
            row("adopted", Some("t1-4"), Some(ImportHow::Adopted), false),
            row("kept", Some("t1-5"), Some(ImportHow::Imported), false),
        ];
        let answer = listing(&["t1-5"]);
        let p = fx.plan("mac", Answer::Listed(&answer), &shown);
        assert_eq!(p.remove, ["idle", "adopted"]);
        assert!(p.add.is_empty());
        let p = fx.plan("mac", Answer::Unavailable, &shown);
        assert_eq!(p.remove, ["idle", "adopted", "kept"]);
        assert!(p.add.is_empty() && p.update.is_empty());
        assert_eq!(
            fx.plan("mac", Answer::Refused, &shown),
            ImportPlan::default()
        );
        for odd in [json!({}), json!({"tabs": "t1-5"}), json!(null)] {
            assert_eq!(
                fx.plan("mac", Answer::Listed(&odd), &shown),
                ImportPlan::default(),
                "an answer with no list is a refusal: {odd}"
            );
        }
        // An entry that names the tab under another origin no longer counts as the host's own: the row goes.
        let moved =
            json!({"tabs": [{"tab": "t1-5", "state": "idle", "origin": "viewer"}, entry("t1-9")]});
        let p = fx.plan("mac", Answer::Listed(&moved), &shown);
        assert_eq!(p.remove, ["idle", "adopted", "kept"]);
    }

    /// Rule 1290: each machine's round reads only its own rows, hidden ids and closing entries, even when two hosts
    /// use the same tab id.
    #[test]
    fn import_plan_keeps_machines_apart() {
        let mut fx = Fx::new();
        fx.hidden.hide("pc", "t1-1");
        fx.closing.add("mini", "t1-1", fx.now);
        let answer = listing(&["t1-1"]);
        let mac_rows = [row("m", Some("t1-1"), Some(ImportHow::Imported), false)];
        // The mac shows t1-1 already; the pc hides its own t1-1; the mini closes its own t1-1.
        let p = fx.plan("mac", Answer::Listed(&answer), &mac_rows);
        assert!(p.add.is_empty());
        assert_eq!(p.update.len(), 1);
        let p = fx.plan("pc", Answer::Listed(&answer), &[]);
        assert!(p.add.is_empty());
        let p = fx.plan("mini", Answer::Listed(&answer), &[]);
        assert!(p.add.is_empty());
        // Each machine's t1-1 is new to the others.
        let mut other = Fx::new();
        other.hidden = fx.hidden.clone();
        for m in ["mac", "mini"] {
            assert_eq!(
                added(&other.plan(m, Answer::Listed(&answer), &[])),
                ["t1-1"]
            );
        }
        // Unavailable on one machine removes only that machine's rows (the caller hands each machine its own).
        assert_eq!(
            fx.plan("pc", Answer::Unavailable, &[]).remove,
            Vec::<String>::new()
        );
        assert!(
            fx.plan("pc", Answer::Listed(&json!({"tabs": []})), &[])
                .hidden_gone
                == ["t1-1"]
        );
        assert!(
            fx.plan("mac", Answer::Listed(&json!({"tabs": []})), &[])
                .hidden_gone
                .is_empty()
        );
    }

    /// Rule 1290: setting off, a machine stays polled only for an agent's adopted row: the row is fed and removed as
    /// usual, and nothing new is added.
    #[test]
    fn import_plan_adds_nothing_while_the_setting_is_off_but_still_feeds_and_removes() {
        let fx = Fx::new();
        let answer = listing(&["t1-1", "t1-2"]);
        let shown = [
            row("a", Some("t1-1"), Some(ImportHow::Adopted), false),
            row("b", Some("t1-3"), Some(ImportHow::Adopted), false),
        ];
        let p = import_plan(&Round {
            machine: "mac",
            answer: Answer::Listed(&answer),
            shown: &shown,
            hidden: &fx.hidden,
            closing: &fx.closing,
            in_flight: false,
            add_new: false,
            now: fx.now,
        });
        assert!(p.add.is_empty());
        assert_eq!(p.update.len(), 1);
        assert_eq!(p.update[0].0, "a");
        assert_eq!(p.remove, ["b"]);
    }

    /// Rule 1290: a peer's entry is read as words: a tab id that is not a plain word names nothing, an unsafe folder
    /// becomes the host's home (rule 1279), a title loses control characters and is capped, an entry with no state
    /// word is not added, and a session that is not a UUID is dropped.
    #[test]
    fn import_plan_reads_each_entry_as_words() {
        let fx = Fx::new();
        let long = "x".repeat(TITLE_MAX + 50);
        let answer = json!({"tabs": [
            {"tab": "../etc", "state": "idle", "origin": ORIGIN_HERE},
            {"tab": "t".repeat(HOST_TAB_MAX + 1), "state": "idle", "origin": ORIGIN_HERE},
            {"tab": "t1-1", "state": "question", "folder": r"\\evil\share", "title": "a\u{1b}]0;b\u{7}c\u{202e}",
             "kind": "claude", "session": "../../x", "question_text": "Go?", "sized": "host",
             "origin": ORIGIN_HERE},
            {"tab": "t1-2", "origin": ORIGIN_HERE},
            {"tab": "t1-3", "state": "idle", "title": long, "folder": "/p", "origin": ORIGIN_HERE},
            {"tab": 7, "state": "idle", "origin": ORIGIN_HERE},
        ]});
        let p = fx.plan("mac", Answer::Listed(&answer), &[]);
        assert_eq!(added(&p), ["t1-1", "t1-3"]);
        let r = &p.add[0];
        assert_eq!(r.folder, "~");
        assert_eq!(r.title, "a]0;bc");
        assert_eq!(r.status.question.as_deref(), Some("Go?"));
        assert_eq!(r.status.agent.as_deref(), Some("claude"));
        assert_eq!(r.status.session, None);
        assert!(r.status.host_sized);
        assert_eq!(p.add[1].title.chars().count(), TITLE_MAX);
        assert!(host_tab_ok("t19a3f-12") && host_tab_ok("a_b"));
        assert!(
            !host_tab_ok("") && !host_tab_ok("t 1") && !host_tab_ok("t/1") && !host_tab_ok("té")
        );
    }

    /// Builder's call 12: a closing entry lapses when a real listing of its machine no longer names the tab, or after
    /// two minutes; `unavailable`, a refusal and another machine's listing drop nothing early.
    #[test]
    fn a_closing_entry_lapses_when_the_host_drops_it_or_after_two_minutes() {
        let t0 = Instant::now();
        let mut c = ClosingList::default();
        assert!(c.is_empty());
        c.add("mac", "t1-1", t0);
        c.add("mac", "t1-2", t0);
        c.add("pc", "t1-1", t0);
        assert!(c.holds("mac", "t1-1", t0) && c.holds("pc", "t1-1", t0));
        assert!(!c.holds("mini", "t1-1", t0));
        let still = listing(&["t1-1", "t1-2"]);
        c.heard("mac", &Answer::Listed(&still), t0);
        c.heard("mac", &Answer::Unavailable, t0);
        c.heard("mac", &Answer::Refused, t0);
        c.heard("mac", &Answer::Listed(&json!({})), t0);
        assert!(c.holds("mac", "t1-1", t0) && c.holds("mac", "t1-2", t0));
        // The host answered the ask and the tab went: only that entry lapses, and only on its machine.
        let one = listing(&["t1-2"]);
        c.heard("mac", &Answer::Listed(&one), t0 + Duration::from_secs(1));
        assert!(!c.holds("mac", "t1-1", t0));
        assert!(c.holds("mac", "t1-2", t0) && c.holds("pc", "t1-1", t0));
        // The host kept the tab: after two minutes it is not held, and the next round drops it.
        let late = t0 + CLOSING_FOR;
        assert!(!c.holds("mac", "t1-2", late));
        c.heard("mac", &Answer::Refused, late);
        assert!(c.is_empty(), "every entry lapsed by time");
        // Closing again restarts the clock; a removed machine drops its entries.
        c.add("pc", "t1-1", late);
        c.add("pc", "t1-1", late);
        c.add("mac", "t1-3", late);
        c.drop_machine("pc");
        assert!(!c.holds("pc", "t1-1", late) && c.holds("mac", "t1-3", late));
    }

    /// Rule 1290: the hidden list keeps well formed ids only, moves a hidden-again id to the newest, keeps the newest
    /// 200 per machine, and a removed machine drops its list.
    #[test]
    fn the_hidden_list_keeps_the_newest_ids_per_machine_and_drops_a_removed_machine() {
        let mut stored = BTreeMap::new();
        stored.insert(
            "mac".to_string(),
            vec!["t1-1".to_string(), "../x".to_string()],
        );
        stored.insert("bad".to_string(), vec!["a b".to_string()]);
        let mut h = HiddenList::from_map(stored);
        assert_eq!(h.as_map().len(), 1);
        assert_eq!(h.ids("mac"), ["t1-1"]);
        assert!(!h.hide("mac", "no good"));
        assert!(h.hide("mac", "t1-2"));
        assert!(h.hide("mac", "t1-1"));
        assert_eq!(h.ids("mac"), ["t1-2", "t1-1"], "hidden again is the newest");
        for n in 0..IMPORT_HIDDEN_MAX {
            h.hide("pc", &format!("t2-{n}"));
        }
        h.hide("pc", "t2-new");
        assert_eq!(h.ids("pc").len(), IMPORT_HIDDEN_MAX);
        assert!(!h.hides("pc", "t2-0") && h.hides("pc", "t2-new"));
        assert!(h.unhide("mac", "t1-2"));
        assert!(!h.unhide("mac", "t1-2"));
        assert!(!h.unhide("mini", "t1-2"));
        assert!(h.unhide("mac", "t1-1"));
        assert!(h.ids("mac").is_empty() && !h.as_map().contains_key("mac"));
        assert!(h.drop_machine("pc"));
        assert!(!h.drop_machine("pc"));
        assert!(h.as_map().is_empty());
    }

    // ---- the watch links (rule 1291) ----

    use flare_ctl::{CtlError, code};
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    /// A machine for the watch: whether it answers at all, what `remote.status` answers, and every call it got.
    #[derive(Default)]
    struct FakeMachine {
        down: AtomicBool,
        /// Some: the code `remote.status` is refused with (`unavailable`: no Flare runs there).
        refuse: Mutex<Option<&'static str>>,
        connects: AtomicU32,
        calls: Mutex<Vec<(Method, Value)>>,
    }

    struct FakeLink(Arc<FakeMachine>);

    impl Link for FakeLink {
        fn call(&mut self, m: Method, p: Value, _t: Duration) -> Result<Value, LinkError> {
            if self.0.down.load(Ordering::SeqCst) {
                return Err(LinkError::Down("the link broke".into()));
            }
            self.0.calls.lock().unwrap().push((m, p));
            if let Some(c) = *self.0.refuse.lock().unwrap() {
                return Err(LinkError::Refused(CtlError::new(c, "not now")));
            }
            Ok(listing(&["t1-1"]))
        }
    }

    struct FakeConnector(Arc<FakeMachine>);

    impl Connector for FakeConnector {
        fn connect(&self, _machine: &str) -> Result<Box<dyn Link>, LinkError> {
            self.0.connects.fetch_add(1, Ordering::SeqCst);
            if self.0.down.load(Ordering::SeqCst) {
                return Err(LinkError::Down("no route to host".into()));
            }
            Ok(Box::new(FakeLink(Arc::clone(&self.0))))
        }
    }

    fn quick() -> WatchTiming {
        WatchTiming {
            every: Duration::from_millis(10),
            backoff_min: Duration::from_millis(10),
            backoff_max: Duration::from_millis(40),
            call: Duration::from_secs(1),
        }
    }

    fn wait_for(what: &str, f: impl Fn() -> bool) {
        let end = Instant::now() + Duration::from_secs(5);
        while !f() {
            assert!(Instant::now() < end, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5)); // sync-ok: test code only
        }
    }

    type Heards = Arc<Mutex<Vec<(String, &'static str)>>>;

    fn watched(m: &Arc<FakeMachine>) -> (Box<dyn Watch>, Heards) {
        let heard: Heards = Arc::default();
        let h2 = Arc::clone(&heard);
        let port = LinkWatchPort::new(Arc::new(FakeConnector(Arc::clone(m))), quick());
        let w = port.watch(
            "mac",
            Arc::new(move |a: WatchAnswer| {
                let kind = match a.heard {
                    Heard::Listed(_) => "listed",
                    Heard::Unavailable => "unavailable",
                    Heard::Refused => "refused",
                };
                h2.lock().unwrap().push((a.machine, kind));
            }),
        );
        (w, heard)
    }

    fn statuses(m: &FakeMachine) -> usize {
        m.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(c, _)| *c == Method::RemoteStatus)
            .count()
    }

    /// Rule 1291: the default watch polls every 3 s and, while the machine does not answer, waits 3, 6, 12, 24, 48 s and
    /// then 60 s at most between tries.
    #[test]
    fn the_watch_backs_off_from_three_seconds_to_a_minute() {
        let t = WatchTiming::default();
        assert_eq!(t.every, Duration::from_secs(3));
        let mut d = t.backoff_min;
        let mut seen = vec![d.as_secs()];
        for _ in 0..6 {
            d = next_backoff(d, t.backoff_max);
            seen.push(d.as_secs());
        }
        assert_eq!(seen, [3, 6, 12, 24, 48, 60, 60]);
    }

    /// Rule 1291: the watch asks `remote.status` again and again and hands each answer on; a machine that is down is
    /// tried again; `unavailable` (no Flare runs there) and a passing refusal are handed on as such; once the window drops
    /// the watch (the setting off), no call is made any more.
    #[test]
    fn the_watcher_polls_status_backs_off_when_down_and_stops_when_off() {
        let m = Arc::new(FakeMachine::default());
        let (w, heard) = watched(&m);
        wait_for("three polls", || statuses(&m) >= 3);
        assert!(
            heard
                .lock()
                .unwrap()
                .iter()
                .all(|(mm, k)| mm == "mac" && *k == "listed")
        );
        m.down.store(true, Ordering::SeqCst);
        let connects = m.connects.load(Ordering::SeqCst);
        wait_for("tries while down", || {
            m.connects.load(Ordering::SeqCst) >= connects + 3
        });
        let n = heard.lock().unwrap().len();
        std::thread::sleep(Duration::from_millis(60)); // sync-ok: test code only
        assert_eq!(
            heard.lock().unwrap().len(),
            n,
            "a machine that is down tells nothing"
        );
        m.down.store(false, Ordering::SeqCst);
        *m.refuse.lock().unwrap() = Some(code::UNAVAILABLE);
        wait_for("unavailable", || {
            heard
                .lock()
                .unwrap()
                .iter()
                .any(|(_, k)| *k == "unavailable")
        });
        *m.refuse.lock().unwrap() = Some(code::BUSY);
        wait_for("a refusal", || {
            heard.lock().unwrap().iter().any(|(_, k)| *k == "refused")
        });
        drop(w);
        std::thread::sleep(Duration::from_millis(30)); // sync-ok: test code only
        let (calls, connects) = (statuses(&m), m.connects.load(Ordering::SeqCst));
        std::thread::sleep(Duration::from_millis(100)); // sync-ok: test code only
        assert_eq!(statuses(&m), calls, "no poll once stopped");
        assert_eq!(
            m.connects.load(Ordering::SeqCst),
            connects,
            "no link once stopped"
        );
        assert!(
            m.calls
                .lock()
                .unwrap()
                .iter()
                .all(|(c, _)| *c == Method::RemoteStatus),
            "a watch only polls"
        );
    }

    /// Rule 1293: Close on an unattached row is `remote.close {tab}` over the machine's watch link; asked while the
    /// machine is down, it goes once the link is up again.
    #[test]
    fn close_on_an_unattached_imported_row_sends_remote_close_through_the_watch_link() {
        let m = Arc::new(FakeMachine::default());
        let (w, _heard) = watched(&m);
        wait_for("a poll", || statuses(&m) >= 1);
        w.close("t1-1");
        let closes = |m: &FakeMachine| -> Vec<Value> {
            m.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(c, _)| *c == Method::RemoteClose)
                .map(|(_, p)| p.clone())
                .collect()
        };
        wait_for("the close", || !closes(&m).is_empty());
        assert_eq!(closes(&m), [json!({"tab": "t1-1"})]);
        m.down.store(true, Ordering::SeqCst);
        let tries = m.connects.load(Ordering::SeqCst);
        wait_for("the link down", || {
            m.connects.load(Ordering::SeqCst) > tries
        });
        w.close("t1-2");
        std::thread::sleep(Duration::from_millis(40)); // sync-ok: test code only
        assert_eq!(closes(&m).len(), 1, "nothing goes while it is down");
        m.down.store(false, Ordering::SeqCst);
        wait_for("the close once up", || closes(&m).len() == 2);
        assert_eq!(closes(&m)[1], json!({"tab": "t1-2"}));
    }
}
