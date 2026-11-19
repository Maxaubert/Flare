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
//!
//! [`HostLists`] keeps each machine's newest answer for the agent's `open.remote.list` and adopt (rule 1296), and
//! [`list_machine`] makes the agent's view of it field by field.

use std::collections::{BTreeMap, HashMap, HashSet};
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

    /// Takes `tab` off the list (an agent's adopt, rule 1296). False when it was not hidden.
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

/// One machine's watch while it lives. Dropping it stops the polls (rule 1291: no watch while the setting is off); the
/// closes it was asked still go first, for up to [`WatchTiming::drain`].
pub trait Watch {
    /// Close on an unattached row (rule 1293): `remote.close {tab}` over this link, now or once it is up again, even
    /// when the watch is dropped right after.
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
    /// How long a dropped watch still tries to send the closes it was asked (rule 1293). No longer than
    /// [`CLOSING_FOR`]: past that the host tab may come back as an import anyway.
    pub drain: Duration,
}

impl Default for WatchTiming {
    fn default() -> Self {
        Self {
            every: Duration::from_secs(3),
            backoff_min: Duration::from_secs(3),
            backoff_max: Duration::from_secs(60),
            call: Duration::from_secs(15),
            drain: CLOSING_FOR,
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
    /// When the window dropped the watch.
    stopped: Option<Instant>,
    /// Host tabs to close (rule 1293), sent in order once the link is up.
    closes: Vec<String>,
}

struct WatchShared {
    state: Mutex<WatchState>,
    cv: Condvar,
    /// [`WatchTiming::drain`].
    drain: Duration,
}

impl WatchShared {
    fn new(drain: Duration) -> Self {
        Self {
            state: Mutex::new(WatchState::default()),
            cv: Condvar::new(),
            drain,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, WatchState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The window dropped the watch: no poll and no answer any more, only the closes still waiting.
    fn stopped(&self) -> bool {
        self.lock().stopped.is_some()
    }

    /// The worker ends: the watch was dropped and has no close left, or its closes waited [`WatchTiming::drain`].
    fn over_in(&self, st: &WatchState, now: Instant) -> bool {
        st.stopped.is_some_and(|at| {
            st.closes.is_empty() || now.saturating_duration_since(at) >= self.drain
        })
    }

    fn over(&self) -> bool {
        self.over_in(&self.lock(), Instant::now())
    }

    /// Waits `d`, or until the watch is over, or (`for_close`) a close is asked. A dropped watch with closes left
    /// waits no longer than its drain time. False once over.
    fn wait(&self, d: Duration, for_close: bool) -> bool {
        let end = Instant::now() + d;
        let mut st = self.lock();
        loop {
            let now = Instant::now();
            if self.over_in(&st, now) {
                return false;
            }
            let mut left = end.saturating_duration_since(now);
            if let Some(at) = st.stopped {
                left = left.min((at + self.drain).saturating_duration_since(now));
            }
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
    /// Never waits (the UI thread): the worker sends the closes still waiting, then ends by itself.
    fn drop(&mut self) {
        self.0.lock().stopped = Some(Instant::now());
        self.0.cv.notify_all();
    }
}

impl WatchPort for LinkWatchPort {
    fn watch(&self, machine: &str, post: WatchPost) -> Box<dyn Watch> {
        let shared = Arc::new(WatchShared::new(self.timing.drain));
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

    /// BLOCKING (ssh, the polls): the watch's own thread until the window drops it and its closes went (or waited
    /// [`WatchTiming::drain`]).
    fn run(self) {
        let mut backoff = self.t.backoff_min;
        while !self.shared.over() {
            match self.connector.connect(&self.machine) {
                Ok(mut link) => {
                    backoff = self.t.backoff_min;
                    self.polls(&mut *link);
                }
                Err(e) => log::info!("watch link to {}: {e}", self.machine),
            }
            // sync-ok: the watch link's own worker waits out its back-off
            if !self.shared.wait(backoff, false) {
                break;
            }
            backoff = next_backoff(backoff, self.t.backoff_max);
        }
        let left = self.shared.lock().closes.len();
        if left > 0 {
            log::warn!(
                "the watch link to {} stopped with {left} close(s) it could not send",
                self.machine
            );
        }
    }

    /// The polls of one link: the closes asked, then `remote.status`, every `every`, until the link breaks or the
    /// watch is over. A dropped watch sends only its closes.
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
            if self.shared.stopped() {
                return;
            }
            let Some(heard) = heard_of(link.call(Method::RemoteStatus, json!({}), self.t.call))
            else {
                return;
            };
            self.tell(heard);
            // sync-ok: the watch link's own worker waits for its next poll
            if !self.shared.wait(self.t.every, true) {
                return;
            }
        }
    }
}

// ---- the agent's list and adopt (rule 1296) ----

/// A machine's answer younger than this is used as it is by the agent's list; an older one is asked again, at most once
/// per machine in this time.
pub const LIST_FRESH: Duration = Duration::from_secs(5);

/// How long the agent's list waits for the machines it asks afresh, all at once. One that has not answered by then is
/// listed as not reachable: a link alone may wait up to `remote_link::HELLO_WAIT`, longer than any call's time.
pub const LIST_WAIT: Duration = Duration::from_secs(10);

/// The longest agent kind read from a peer (`claude`, `codex`, `agy`).
const AGENT_MAX: usize = 32;

/// Asks one machine's `remote.status` once (the agent's list, rule 1296). A test gives a fake.
pub trait ListAsk: Send + Sync {
    /// BLOCKING (ssh): what the ask heard; None when the link did not come up or broke.
    fn ask(&self, machine: &str) -> Option<Heard>;
}

/// The real ask: one short link from the watch links' connector (`--serve-remote --no-start`, so it never starts Flare
/// on the host), one `remote.status`, then the link is let go.
pub struct LinkListAsk {
    connector: Arc<dyn Connector>,
    call: Duration,
}

impl LinkListAsk {
    pub fn new(connector: Arc<dyn Connector>) -> Self {
        Self {
            connector,
            call: WatchTiming::default().call,
        }
    }
}

impl ListAsk for LinkListAsk {
    fn ask(&self, machine: &str) -> Option<Heard> {
        let mut link = match self.connector.connect(machine) {
            Ok(l) => l,
            Err(e) => {
                log::info!("the agent's list could not reach {machine}: {e}");
                return None;
            }
        };
        heard_of(link.call(Method::RemoteStatus, json!({}), self.call))
    }
}

/// What the agent's list knows of one machine.
#[derive(Default)]
struct Known {
    /// The newest answer and when it was heard or asked (its `Option` None: the link was down).
    heard: Option<(Instant, Option<Heard>)>,
    /// When the list last asked it afresh.
    asked: Option<Instant>,
    /// An ask of it still runs (a host stuck in its hello): no second one starts beside it.
    running: bool,
}

/// The newest answer of each machine for the agent's list and adopt (rule 1296): the watch links' answers as the window
/// hears them, and the list's own asks when those are older than [`LIST_FRESH`] (or there is no watch, the setting
/// being off). Shared by the window and the control channel's workers.
pub struct HostLists {
    asker: Arc<dyn ListAsk>,
    known: Mutex<HashMap<String, Known>>,
}

impl HostLists {
    pub fn new(asker: Arc<dyn ListAsk>) -> Self {
        Self {
            asker,
            known: Mutex::new(HashMap::new()),
        }
    }

    /// Lists whose asks never reach a machine (the tests' servers).
    #[cfg(test)]
    pub fn asking_none() -> Arc<Self> {
        struct NoAsk;
        impl ListAsk for NoAsk {
            fn ask(&self, _machine: &str) -> Option<Heard> {
                None
            }
        }
        Arc::new(Self::new(Arc::new(NoAsk)))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Known>> {
        self.known
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A watch link's answer, as the window hears it.
    pub fn heard(&self, machine: &str, heard: &Heard, now: Instant) {
        self.lock().entry(machine.to_string()).or_default().heard =
            Some((now, Some(heard.clone())));
    }

    /// BLOCKING (a control call's worker, rule 34): the newest answer of each of `machines`. One older than
    /// [`LIST_FRESH`] is asked again, at most once per machine in that time and never while an ask of it still runs,
    /// all of them at once, each on a thread of its own; the call waits at most `wait` for them. Only an answer younger
    /// than [`LIST_FRESH`] counts: a machine that answered nothing, not in time, or only long ago is None, so a stale
    /// listing never names a tab to adopt.
    pub fn answers(
        self: &Arc<Self>,
        machines: &[String],
        now: Instant,
        wait: Duration,
    ) -> HashMap<String, Option<Heard>> {
        let young = |at: Option<Instant>| {
            at.is_some_and(|at| now.saturating_duration_since(at) < LIST_FRESH)
        };
        // The ask is stamped before it starts, so two calls at once never both ask.
        let ask: Vec<String> = {
            let mut known = self.lock();
            machines
                .iter()
                .filter(|m| {
                    let k = known.entry((*m).clone()).or_default();
                    if young(k.heard.as_ref().map(|(at, _)| *at)) || young(k.asked) || k.running {
                        return false;
                    }
                    k.asked = Some(now);
                    k.running = true;
                    true
                })
                .cloned()
                .collect()
        };
        let (tx, rx) = std::sync::mpsc::channel();
        for name in &ask {
            let (me, m, tx) = (Arc::clone(self), name.clone(), tx.clone());
            let started = std::thread::Builder::new()
                .name("flare-remote-list".into())
                .spawn(move || {
                    let heard = me.asker.ask(&m);
                    {
                        let mut known = me.lock();
                        let k = known.entry(m.clone()).or_default();
                        // A watch answer heard after the ask started is fresher: the ask's own is dropped.
                        if k.heard.as_ref().is_none_or(|(at, _)| *at < now) {
                            k.heard = Some((now, heard));
                        }
                        k.running = false;
                    }
                    let _ = tx.send(m);
                });
            if let Err(e) = started {
                log::warn!("could not ask a machine for the agent's list: {e}");
                if let Some(k) = self.lock().get_mut(name) {
                    k.running = false;
                }
            }
        }
        drop(tx);
        let end = Instant::now() + wait;
        let mut late: HashSet<String> = ask.iter().cloned().collect();
        while !late.is_empty() {
            let left = end.saturating_duration_since(Instant::now());
            // sync-ok: a control call's worker waits for its own asks, never the UI thread
            match rx.recv_timeout(left) {
                Ok(m) => {
                    late.remove(&m);
                }
                Err(_) => break,
            }
        }
        let known = self.lock();
        machines
            .iter()
            .map(|m| {
                let heard = if late.contains(m) {
                    None
                } else {
                    known
                        .get(m)
                        .and_then(|k| k.heard.as_ref())
                        .filter(|(at, _)| young(Some(*at)))
                        .and_then(|(_, h)| h.clone())
                };
                (m.clone(), heard)
            })
            .collect()
    }
}

/// The entries of a listing that are the host's own tabs (`origin: here`), each id well formed, each once.
fn own_entries(heard: Option<&Heard>) -> Vec<(&str, &Value)> {
    let Some(Heard::Listed(v)) = heard else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    v["tabs"]
        .as_array()
        .map(|tabs| {
            tabs.iter()
                .filter_map(|t| Some((t["tab"].as_str().filter(|id| host_tab_ok(id))?, t)))
                .filter(|(id, t)| t["origin"].as_str() == Some(ORIGIN_HERE) && seen.insert(*id))
                .collect()
        })
        .unwrap_or_default()
}

/// A row of a host's own tab: its folder (`~` when the host names no safe one), title and status. None when the entry
/// has no state.
fn row_of(id: &str, t: &Value) -> Option<ImportRow> {
    let status = status_from(t)?;
    Some(ImportRow {
        host_tab: id.to_string(),
        folder: status.folder.clone().unwrap_or_else(|| HOME.into()),
        title: title_of(&t["title"]),
        status,
    })
}

/// Pure (rule 1296): the host tab `tab` as the machine's newest answer lists it, for an agent's adopt. None when the
/// answer does not list it as the host's own tab (`origin: here`), or there is no answer.
pub fn adoptable(heard: Option<&Heard>, tab: &str) -> Option<ImportRow> {
    own_entries(heard)
        .into_iter()
        .find(|(id, _)| *id == tab)
        .and_then(|(id, t)| row_of(id, t))
}

/// One machine of the agent's list, as the window and the machine's answer say it.
pub struct ListedMachine<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub heard: Option<&'a Heard>,
    /// The host tabs of this machine shown here as rows.
    pub shown: &'a HashSet<String>,
    /// The host tabs of this machine hidden here.
    pub hidden: &'a HashSet<String>,
}

/// Pure (rule 1296): one machine of the agent's list. Each tab is made field by field, never a copy of the host's
/// entry: `tab`, `folder`, `title`, `state` (one word), `agent` (a kind or null) and `here` (`shown`, `hidden` or
/// `none`). Never a session id, question words, a failure's words or screen text. `reachable` is false when the
/// machine gave no listing (no Flare runs there, a refusal, a link down or late).
pub fn list_machine(m: &ListedMachine) -> Value {
    let reachable = matches!(m.heard, Some(Heard::Listed(v)) if v["tabs"].is_array());
    let tabs: Vec<Value> = own_entries(m.heard)
        .into_iter()
        .filter_map(|(id, t)| row_of(id, t))
        .map(|r| {
            let here = if m.shown.contains(&r.host_tab) {
                "shown"
            } else if m.hidden.contains(&r.host_tab) {
                "hidden"
            } else {
                "none"
            };
            let state = flare_ctl::remote::TabState::parse(&r.status.state)
                .unwrap_or(flare_ctl::remote::TabState::Idle)
                .word();
            let agent = r.status.agent.filter(|a| {
                a.len() <= AGENT_MAX
                    && a.chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            });
            json!({
                "tab": r.host_tab,
                "folder": r.folder,
                "title": r.title,
                "state": state,
                "agent": agent,
                "here": here,
            })
        })
        .collect();
    json!({
        "machine": m.id,
        "name": m.name,
        "reachable": reachable,
        "tabs": tabs,
    })
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
            drain: Duration::from_millis(500),
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

    fn closes(m: &FakeMachine) -> Vec<Value> {
        m.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(c, _)| *c == Method::RemoteClose)
            .map(|(_, p)| p.clone())
            .collect()
    }

    /// Rule 1293: Close on an unattached row is `remote.close {tab}` over the machine's watch link; asked while the
    /// machine is down, it goes once the link is up again.
    #[test]
    fn close_on_an_unattached_imported_row_sends_remote_close_through_the_watch_link() {
        let m = Arc::new(FakeMachine::default());
        let (w, _heard) = watched(&m);
        wait_for("a poll", || statuses(&m) >= 1);
        w.close("t1-1");
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

    /// Rule 1293: a close asked just before the watch stops (the last adopted row closed while the setting is off, or
    /// the machine removed) still goes over the link that is up; then the watch ends with no poll after the stop.
    #[test]
    fn a_close_asked_as_the_watch_stops_is_still_sent_over_the_link_that_is_up() {
        let m = Arc::new(FakeMachine::default());
        let (w, heard) = watched(&m);
        wait_for("a poll", || statuses(&m) >= 1);
        w.close("t1-1");
        drop(w);
        wait_for("the close", || !closes(&m).is_empty());
        assert_eq!(closes(&m), [json!({"tab": "t1-1"})]);
        let told = heard.lock().unwrap().len();
        let (calls, connects) = (
            m.calls.lock().unwrap().len(),
            m.connects.load(Ordering::SeqCst),
        );
        std::thread::sleep(Duration::from_millis(100)); // sync-ok: test code only
        assert_eq!(
            m.calls.lock().unwrap().len(),
            calls,
            "no call after the close"
        );
        assert_eq!(
            m.connects.load(Ordering::SeqCst),
            connects,
            "no link after the close"
        );
        assert_eq!(
            heard.lock().unwrap().len(),
            told,
            "a stopped watch tells nothing"
        );
    }

    /// Rule 1293 and the spec's failure table: a close asked while the link is down is tried once when it comes back,
    /// even when the watch stopped in the meantime; it sends only the close, then the watch ends.
    #[test]
    fn a_close_asked_while_down_is_sent_when_the_link_comes_back_after_the_watch_stopped() {
        let m = Arc::new(FakeMachine::default());
        m.down.store(true, Ordering::SeqCst);
        let (w, heard) = watched(&m);
        wait_for("a try", || m.connects.load(Ordering::SeqCst) >= 1);
        w.close("t1-2");
        drop(w);
        std::thread::sleep(Duration::from_millis(40)); // sync-ok: test code only
        assert!(closes(&m).is_empty(), "nothing goes while it is down");
        m.down.store(false, Ordering::SeqCst);
        wait_for("the close once up", || !closes(&m).is_empty());
        assert_eq!(closes(&m), [json!({"tab": "t1-2"})]);
        let connects = m.connects.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(100)); // sync-ok: test code only
        assert_eq!(statuses(&m), 0, "a stopped watch sends only its closes");
        assert!(heard.lock().unwrap().is_empty());
        assert_eq!(m.connects.load(Ordering::SeqCst), connects, "then it ends");
    }

    /// Rule 1293: a dropped watch tries its closes for [`WatchTiming::drain`] at most (two minutes by default, never
    /// past [`CLOSING_FOR`]); a link that comes back later gets nothing, and the watch makes no link after.
    #[test]
    fn a_stopped_watch_gives_up_its_closes_after_the_drain_time() {
        assert!(WatchTiming::default().drain <= CLOSING_FOR);
        let m = Arc::new(FakeMachine::default());
        m.down.store(true, Ordering::SeqCst);
        let (w, _heard) = watched(&m);
        wait_for("a try", || m.connects.load(Ordering::SeqCst) >= 1);
        w.close("t1-3");
        drop(w);
        let tries = m.connects.load(Ordering::SeqCst);
        wait_for("tries while it drains", || {
            m.connects.load(Ordering::SeqCst) > tries
        });
        std::thread::sleep(quick().drain + Duration::from_millis(100)); // sync-ok: test code only
        let connects = m.connects.load(Ordering::SeqCst);
        m.down.store(false, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(150)); // sync-ok: test code only
        assert_eq!(
            m.connects.load(Ordering::SeqCst),
            connects,
            "no link after the drain time"
        );
        assert!(closes(&m).is_empty(), "the close is given up");
    }

    // ---- the agent's list (rule 1296) ----

    /// A list ask that counts its asks per machine and answers what the test set (None: the link is down).
    #[derive(Default)]
    struct FakeAsk {
        answers: Mutex<HashMap<String, Option<Heard>>>,
        asks: Mutex<Vec<String>>,
        /// Each ask waits this long first (a slow host).
        slow: Mutex<Duration>,
    }

    impl ListAsk for FakeAsk {
        fn ask(&self, machine: &str) -> Option<Heard> {
            self.asks.lock().unwrap().push(machine.to_string());
            let slow = *self.slow.lock().unwrap();
            std::thread::sleep(slow); // sync-ok: test code only
            self.answers.lock().unwrap().get(machine).cloned().flatten()
        }
    }

    fn asks_of(a: &FakeAsk, m: &str) -> usize {
        a.asks.lock().unwrap().iter().filter(|x| *x == m).count()
    }

    /// Rule 1296: the list asks a machine afresh only when its newest answer is older than five seconds, at most once
    /// per machine in that time, and a watch link's answer counts as fresh. A machine too slow to answer in time is
    /// none, and the call does not wait for it past its time.
    #[test]
    fn the_list_asks_each_machine_at_most_once_per_five_seconds() {
        let ask = Arc::new(FakeAsk::default());
        ask.answers
            .lock()
            .unwrap()
            .insert("mac".into(), Some(Heard::Listed(listing(&["t1-1"]))));
        let lists = Arc::new(HostLists::new(ask.clone()));
        let t0 = Instant::now();
        let both = ["mac".to_string(), "pc".to_string()];
        let a = lists.answers(&both, t0, LIST_WAIT);
        assert!(matches!(a["mac"], Some(Heard::Listed(_))));
        assert!(a["pc"].is_none(), "the pc's link is down");
        assert_eq!((asks_of(&ask, "mac"), asks_of(&ask, "pc")), (1, 1));
        lists.answers(&both, t0 + Duration::from_secs(4), LIST_WAIT);
        assert_eq!((asks_of(&ask, "mac"), asks_of(&ask, "pc")), (1, 1));
        lists.answers(&both, t0 + Duration::from_secs(6), LIST_WAIT);
        assert_eq!((asks_of(&ask, "mac"), asks_of(&ask, "pc")), (2, 2));
        // A watch link's answer is the newest: no ask while it is young.
        lists.heard(
            "pc",
            &Heard::Listed(listing(&["t9-1"])),
            t0 + Duration::from_secs(20),
        );
        let a = lists.answers(&both, t0 + Duration::from_secs(22), LIST_WAIT);
        assert!(matches!(a["pc"], Some(Heard::Listed(_))));
        assert_eq!((asks_of(&ask, "mac"), asks_of(&ask, "pc")), (3, 2));
        // An answer older than five seconds is never handed out as the newest.
        assert!(
            lists.answers(
                &["pc".to_string()],
                t0 + Duration::from_secs(26),
                Duration::ZERO
            )["pc"]
                .is_none(),
            "the ask is late and the watch's answer is old"
        );
        wait_for("the pc's ask", || asks_of(&ask, "pc") == 3);
        // A slow machine is none once the wait is over; the next call asks it no more while that ask runs, and gets no
        // old listing either.
        *ask.slow.lock().unwrap() = Duration::from_secs(3);
        let started = Instant::now();
        let a = lists.answers(
            &["mac".to_string()],
            t0 + Duration::from_secs(40),
            Duration::from_millis(50),
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(a["mac"].is_none(), "late");
        wait_for("the slow ask", || asks_of(&ask, "mac") == 4);
        let a = lists.answers(
            &["mac".to_string()],
            t0 + Duration::from_secs(47),
            Duration::ZERO,
        );
        assert!(a["mac"].is_none(), "no old listing while the slow ask runs");
        assert_eq!(
            asks_of(&ask, "mac"),
            4,
            "no second ask beside the running one"
        );
    }

    /// Rule 1296: an ask's answer never overwrites a watch answer heard after the ask started: the list hands out the
    /// fresher one, even when the ask found the link down.
    #[test]
    fn an_ask_never_overwrites_a_fresher_watch_answer() {
        /// An ask during which the watch hears the machine, then finds its own link down.
        struct WatchHearsMeanwhile {
            lists: Mutex<Option<std::sync::Weak<HostLists>>>,
            at: Instant,
        }
        impl ListAsk for WatchHearsMeanwhile {
            fn ask(&self, machine: &str) -> Option<Heard> {
                let lists = self.lists.lock().unwrap().clone().unwrap();
                if let Some(l) = lists.upgrade() {
                    l.heard(machine, &Heard::Listed(listing(&["t9-9"])), self.at);
                }
                None
            }
        }
        let t0 = Instant::now();
        let ask = Arc::new(WatchHearsMeanwhile {
            lists: Mutex::new(None),
            at: t0 + Duration::from_secs(1),
        });
        let lists = Arc::new(HostLists::new(ask.clone()));
        *ask.lists.lock().unwrap() = Some(Arc::downgrade(&lists));
        let a = lists.answers(&["mac".to_string()], t0, LIST_WAIT);
        let Some(Heard::Listed(v)) = &a["mac"] else {
            panic!("the watch's fresher answer is handed out");
        };
        assert_eq!(v["tabs"][0]["tab"], "t9-9");
    }

    /// Rule 1296: each listed tab is made field by field (no session id, question words, failure or anything else
    /// the host sent), only the host's own tabs, each once, with `here` from this window.
    #[test]
    fn the_list_names_only_the_hosts_own_tabs_and_carries_no_session_or_question() {
        let heard = Heard::Listed(json!({"tabs": [
            {"tab": "t1-1", "origin": "here", "state": "question", "kind": "claude",
             "session": "3f2a9c1e-77aa-4c0d-9b1e-0a1b2c3d4e5f", "question_text": "Allow rm -rf?",
             "failed": "x", "sized": "host", "folder": "/Users/max/p", "title": "p\u{7} claude",
             "screen": "secret", "extra": 1},
            {"tab": "t1-1", "origin": "here", "state": "idle", "folder": "/again"},
            {"tab": "t1-2", "origin": "here", "state": "a-new-word", "kind": "bad kind!", "folder": "\\\\srv\\x"},
            {"tab": "t1-3", "origin": "here", "state": "working", "kind": "codex", "folder": "/h"},
            {"tab": "t1-4", "origin": "viewer", "state": "idle", "folder": "/v"},
            {"tab": "../x", "origin": "here", "state": "idle", "folder": "/v"},
            {"tab": "t1-5", "origin": "here", "folder": "/no-state"},
        ]}));
        let shown: HashSet<String> = ["t1-1".to_string()].into();
        let hidden: HashSet<String> = ["t1-3".to_string()].into();
        let v = list_machine(&ListedMachine {
            id: "mac",
            name: "Mac mini",
            heard: Some(&heard),
            shown: &shown,
            hidden: &hidden,
        });
        assert_eq!(v["machine"], "mac");
        assert_eq!(v["name"], "Mac mini");
        assert_eq!(v["reachable"], true);
        assert_eq!(
            v["tabs"],
            json!([
                {"tab": "t1-1", "folder": "/Users/max/p", "title": "p claude", "state": "question",
                 "agent": "claude", "here": "shown"},
                {"tab": "t1-2", "folder": "~", "title": "", "state": "idle", "agent": null, "here": "none"},
                {"tab": "t1-3", "folder": "/h", "title": "", "state": "working", "agent": "codex",
                 "here": "hidden"},
            ])
        );
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["machine", "name", "reachable", "tabs"]);
        let text = v.to_string();
        for never in [
            "3f2a9c1e",
            "Allow",
            "secret",
            "sized",
            "session",
            "question_text",
        ] {
            assert!(!text.contains(never), "{never} in {text}");
        }
        // No listing: not reachable, no tabs. An older host (no origin) lists nothing.
        let none = HashSet::new();
        for heard in [
            None,
            Some(Heard::Unavailable),
            Some(Heard::Refused),
            Some(Heard::Listed(json!({"error": 1}))),
        ] {
            let v = list_machine(&ListedMachine {
                id: "pc",
                name: "PC",
                heard: heard.as_ref(),
                shown: &none,
                hidden: &none,
            });
            assert_eq!(v["reachable"], false);
            assert_eq!(v["tabs"], json!([]));
        }
        let older = Heard::Listed(json!({"tabs": [{"tab": "t1-1", "state": "idle"}]}));
        let v = list_machine(&ListedMachine {
            id: "pc",
            name: "PC",
            heard: Some(&older),
            shown: &none,
            hidden: &none,
        });
        assert_eq!(
            (v["reachable"].clone(), v["tabs"].clone()),
            (json!(true), json!([]))
        );
    }

    /// Rule 1296: an agent may adopt only a tab the machine lists as its own.
    #[test]
    fn adoptable_finds_only_a_listed_tab_of_the_hosts_own() {
        let heard = Heard::Listed(json!({"tabs": [
            entry("t1-1"),
            {"tab": "t1-2", "origin": "viewer", "state": "idle"},
        ]}));
        let r = adoptable(Some(&heard), "t1-1").unwrap();
        assert_eq!(
            (r.host_tab.as_str(), r.folder.as_str()),
            ("t1-1", "/Users/max/t1-1")
        );
        assert!(adoptable(Some(&heard), "t1-2").is_none());
        assert!(adoptable(Some(&heard), "t1-9").is_none());
        assert!(adoptable(Some(&Heard::Unavailable), "t1-1").is_none());
        assert!(adoptable(None, "t1-1").is_none());
    }
}
