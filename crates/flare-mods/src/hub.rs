//! The mods hub (stage 8, the app part): everything the app knows about mods, behind one lock, with no
//! window and no browser. The Settings page, the control channel's dispatcher and the window all hold the
//! same `Arc<Hub>`.
//!
//! - **Discovery** (`scan`) reads the mods folder and the switch file on a worker; the answer replaces what
//!   the hub knows. A scan never turns a mod on: the switches are the user's.
//! - **Switches** (`set_enabled`) are applied in memory at once, on any thread: turning a mod ON mints its
//!   token, turning it OFF kills the token (a connection that presents it is refused on its next request) and
//!   fails every tool call waiting on the mod. The file is written by `persist`, on a worker (rule 34).
//! - **Tool routing** (`begin_call`, `respond`, `wait`) keeps the calls the agent made and a mod's page has
//!   not answered yet. A call can only be answered by the mod it was routed to; a mod that is disabled has no
//!   tools; a call never waits longer than [`CALL_TIMEOUT`].
//!
//! Tokens are secrets: nothing here returns one except [`Hub::grant_of_token`]'s grant and
//! [`Hub::grant_of_mod`]'s grant, and no `Debug` or `Snapshot` carries one.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use flare_ctl::{CtlError, Grant, Permission, code};
use serde_json::Value;

use crate::discover::{FolderError, Found, discover};
use crate::enabled::EnabledStore;
use crate::tokens::Tokens;
use crate::tools::{Collision, Route, TooLong, ToolTable};

/// How long the agent's tool call waits for the mod's page to answer.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Calls one mod may have waiting at once.
pub const MAX_PENDING_PER_MOD: usize = 8;
/// How often a waiting call looks at the stop flag.
const SLICE: Duration = Duration::from_millis(50);

/// What a permission lets a mod do, in the plain words Settings > Mods shows before the person turns the mod on.
/// `eval` is never granted to a mod; its words say so if it ever reaches a list.
pub fn permission_words(p: Permission) -> &'static str {
    match p {
        Permission::Panel => "Show its panel in a tab",
        Permission::View => "Show images, files and text",
        Permission::Browser => "Open and use its own web page",
        Permission::Convo => "Read the agent's tasks and changes",
        Permission::Tools => "Answer the agent's tool calls",
        Permission::Eval => "Run scripts in pages (never allowed to a mod)",
        Permission::Decorate => "Put badges and progress on tabs",
    }
}

/// Where mods live and where the switches are kept.
#[derive(Clone, Debug)]
pub struct Config {
    /// The folder holding one folder per mod (`%APPDATA%\Flare\mods`, or the profile's).
    pub dir: PathBuf,
    /// The switch file (`mods.json`).
    pub state_file: PathBuf,
}

/// One mod as Settings and the window see it. No secret.
#[derive(Clone, Debug, PartialEq)]
pub struct ModEntry {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub permissions: Vec<Permission>,
    /// The panel's title, when the mod has a panel.
    pub panel_title: Option<String>,
    pub tool_count: usize,
    pub enabled: bool,
    /// The mod was on and its manifest now asks for permissions the user never approved, so it was turned off.
    /// Switching it on again approves the new list.
    pub needs_approval: bool,
}

/// What the hub knows now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    /// A scan has finished at least once.
    pub scanned: bool,
    pub mods: Vec<ModEntry>,
    /// Folders that are not mods, with the reason.
    pub errors: Vec<FolderError>,
    /// Sentences about tools left out (a clash, a name too long) and a switch file that is read-only.
    pub notes: Vec<String>,
}

/// What a scan or a switch changed, for the window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Change {
    /// The set of tools the agent sees is not what it was.
    pub tools_changed: bool,
    /// Mods whose token was killed (turned off, gone from the disk, or their manifest changed): a page that
    /// shows one must close.
    pub revoked: Vec<String>,
}

struct Pending {
    mod_id: String,
    tx: Sender<Result<Value, CtlError>>,
}

#[derive(Default)]
struct Inner {
    scanned: bool,
    found: Vec<Found>,
    errors: Vec<FolderError>,
    on: BTreeSet<String>,
    /// What the user approved for each mod that is on (kept with the switch in `mods.json`).
    approved: BTreeMap<String, Vec<Permission>>,
    /// Mods turned off by a scan because their manifest asks for more than was approved.
    needs: BTreeSet<String>,
    /// Mods whose approved set was cut down by the last reconcile; the scan writes them.
    shrunk: Vec<String>,
    tokens: Tokens,
    table: ToolTable,
    collisions: Vec<Collision>,
    too_long: Vec<TooLong>,
    read_only: bool,
    pending: HashMap<String, Pending>,
}

pub struct Hub {
    cfg: Config,
    inner: Mutex<Inner>,
    /// The switch file; loaded by the first scan. Its own lock, so a slow disk never holds the hub's.
    store: Mutex<Option<EnabledStore>>,
}

/// A tool call routed to a mod, waiting for the page's answer.
pub struct Call {
    pub id: String,
    pub mod_id: String,
    pub tool: String,
    /// What the agent passed (always an object).
    pub input: Value,
    rx: Receiver<Result<Value, CtlError>>,
}

impl Hub {
    pub fn new(cfg: Config) -> Self {
        Self {
            cfg,
            inner: Mutex::new(Inner::default()),
            store: Mutex::new(None),
        }
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Reads the mods folder (and, the first time, the switch file). Runs on a worker. Replaces what the hub
    /// knew; mods that were turned on and are still valid keep their token, the others lose it.
    pub fn scan(&self) -> Change {
        let found = discover(&self.cfg.dir);
        // The first scan loads the switches; later ones keep the memory (a switch not yet written to disk is
        // not undone by a scan that reads the old file).
        let first_load = {
            let mut store = self.store.lock().unwrap_or_else(|e| e.into_inner());
            if store.is_none() {
                *store = Some(EnabledStore::load(self.cfg.state_file.clone())); // sync-ok: runs on the scan worker
                true
            } else {
                false
            }
        };
        let (store_ids, read_only) = {
            let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
            store
                .as_ref()
                .map(|s| {
                    let ids: Vec<(String, Vec<Permission>)> = s
                        .enabled_ids()
                        .into_iter()
                        .filter_map(|id| s.approved(&id).map(|a| (id, a)))
                        .collect();
                    (ids, s.is_read_only())
                })
                .unwrap_or_default()
        };
        let mut g = self.lock();
        if first_load {
            g.on = store_ids.iter().map(|(id, _)| id.clone()).collect();
            g.approved = store_ids.into_iter().collect();
        }
        g.read_only = read_only;
        let before_tools: Vec<Route> = g.table.tools().cloned().collect();
        g.found = found.mods;
        g.errors = found.errors;
        g.scanned = true;
        let revoked = Self::reconcile(&mut g);
        let after_tools: Vec<Route> = g.table.tools().cloned().collect();
        let shrunk = std::mem::take(&mut g.shrunk);
        drop(g);
        // A mod that now asks for less keeps the smaller set on disk too (this runs on the scan worker).
        for id in shrunk {
            let _ = self.persist(&id);
        }
        Change {
            tools_changed: before_tools != after_tools,
            revoked,
        }
    }

    /// Makes the tokens and the tool table agree with the switches and the mods found. Returns the ids
    /// whose token was killed.
    fn reconcile(g: &mut Inner) -> Vec<String> {
        let mut revoked = Vec::new();
        // The consent check: a mod stays on only while its manifest asks for nothing beyond what the user
        // approved when they switched it on. Asking for more turns it off (its token dies below); less keeps it
        // on with the smaller set. A mod on with no approved set at all is off (fail closed).
        let ids: Vec<String> = g.on.iter().cloned().collect();
        for id in ids {
            let Some(f) = g.found.iter().find(|f| f.manifest.id == id) else {
                continue;
            };
            let asked = f.manifest.permissions.clone();
            match g.approved.get(&id).cloned() {
                Some(ok) if asked.iter().all(|p| ok.contains(p)) => {
                    if ok.iter().any(|p| !asked.contains(p)) {
                        g.approved.insert(id.clone(), asked);
                        g.shrunk.push(id);
                    }
                }
                other => {
                    g.on.remove(&id);
                    if other.is_some() {
                        g.needs.insert(id);
                    }
                }
            }
        }
        let gone: Vec<String> = g
            .needs
            .iter()
            .filter(|id| !g.found.iter().any(|f| &f.manifest.id == *id))
            .cloned()
            .collect();
        for id in gone {
            g.needs.remove(&id);
        }
        let live: Vec<(String, Vec<Permission>)> = g
            .found
            .iter()
            .filter(|f| g.on.contains(&f.manifest.id))
            .map(|f| (f.manifest.id.clone(), f.manifest.permissions.clone()))
            .collect();
        // A token of a mod that is off, gone or invalid dies.
        let dead: Vec<String> = g
            .tokens
            .ids()
            .into_iter()
            .filter(|id| !live.iter().any(|(l, _)| l == id))
            .collect();
        for id in dead {
            g.tokens.revoke(&id);
            revoked.push(id);
        }
        // A mod that is on gets a token if it has none; one whose permissions changed gets a new one.
        for (id, perms) in &live {
            let stale = g
                .tokens
                .token_of(id)
                .and_then(|t| g.tokens.grant(t))
                .is_some_and(|gr| gr != crate::tokens::mod_grant(id, perms));
            if stale {
                revoked.push(id.clone());
            }
            if (stale || g.tokens.token_of(id).is_none()) && g.tokens.mint(id, perms).is_err() {
                log::warn!("could not make a token for mod {id}; it stays off");
            }
        }
        // The tool table: only mods that are on and hold a token.
        let manifests: Vec<&crate::manifest::Manifest> = g
            .found
            .iter()
            .filter(|f| g.tokens.token_of(&f.manifest.id).is_some())
            .map(|f| &f.manifest)
            .collect();
        let (table, collisions, too_long) = ToolTable::build(manifests);
        g.table = table;
        g.collisions = collisions;
        g.too_long = too_long;
        // Calls waiting on a mod that lost its token can never be answered.
        for id in &revoked {
            Self::fail_calls_locked(
                g,
                id,
                "the mod was turned off or changed while the call waited",
            );
        }
        revoked
    }

    /// Switches a mod on or off in memory, at once. On: needs a mod found by the last scan. Off: always allowed.
    /// The caller writes the file with [`Hub::persist`] on a worker.
    pub fn set_enabled(&self, id: &str, on: bool) -> Result<Change, String> {
        self.set_enabled_shown(id, on, None)
    }

    /// As [`Hub::set_enabled`], for a switch the person clicked on a row of Settings. `shown` is the permission
    /// words that row listed: turning a mod on approves THAT list, so when the manifest found now asks for anything
    /// else (an edit picked up between the row being drawn and the click) nothing changes and the person is told
    /// to look again.
    pub fn set_enabled_shown(
        &self,
        id: &str,
        on: bool,
        shown: Option<&[String]>,
    ) -> Result<Change, String> {
        let mut g = self.lock();
        if on && !g.found.iter().any(|f| f.manifest.id == id) {
            return Err(format!("there is no valid mod \"{id}\" to turn on"));
        }
        if on && let Some(shown) = shown {
            let now: Vec<String> = g
                .found
                .iter()
                .find(|f| f.manifest.id == id)
                .map(|f| {
                    f.manifest
                        .permissions
                        .iter()
                        .map(|p| permission_words(*p).to_string())
                        .collect()
                })
                .unwrap_or_default();
            if now != shown {
                return Err(format!(
                    "the mod \"{id}\" changed what it asks for while you were looking; read the list again before turning it on"
                ));
            }
        }
        let before_tools: Vec<Route> = g.table.tools().cloned().collect();
        g.needs.remove(id);
        if on {
            // Switching on is the approval: the manifest's list as it stands now.
            let asked = g
                .found
                .iter()
                .find(|f| f.manifest.id == id)
                .map(|f| f.manifest.permissions.clone())
                .unwrap_or_default();
            g.approved.insert(id.to_string(), asked);
            g.on.insert(id.to_string());
        } else {
            g.approved.remove(id);
            g.on.remove(id);
        }
        let revoked = Self::reconcile(&mut g);
        let after_tools: Vec<Route> = g.table.tools().cloned().collect();
        Ok(Change {
            tools_changed: before_tools != after_tools,
            revoked,
        })
    }

    /// Writes the mod's switch to the file, as it stands NOW. Runs on a worker. It reads the state under the
    /// file's lock, so two saves racing each other (a mod turned on and straight off again) both write the latest
    /// state, never a stale one. An error is a sentence for Settings; the switch keeps its effect for this session.
    pub fn persist(&self, id: &str) -> Result<(), String> {
        let mut store = self.store.lock().unwrap_or_else(|e| e.into_inner());
        let (on, approved) = {
            let g = self.lock();
            (
                g.on.contains(id),
                g.approved.get(id).cloned().unwrap_or_default(),
            )
        };
        let store = store.get_or_insert_with(|| EnabledStore::load(self.cfg.state_file.clone())); // sync-ok: runs on a worker
        store.set(id, on, &approved)
    }

    pub fn is_enabled(&self, id: &str) -> bool {
        self.lock().on.contains(id)
    }

    /// What Settings and the window show.
    pub fn snapshot(&self) -> Snapshot {
        let g = self.lock();
        let mods = g
            .found
            .iter()
            .map(|f| ModEntry {
                id: f.manifest.id.clone(),
                name: f.manifest.name.clone(),
                version: f.manifest.version.clone(),
                description: f.manifest.description.clone(),
                permissions: f.manifest.permissions.clone(),
                panel_title: f.manifest.panel.as_ref().map(|p| p.title.clone()),
                tool_count: f.manifest.tools.len(),
                enabled: g.on.contains(&f.manifest.id),
                needs_approval: g.needs.contains(&f.manifest.id),
            })
            .collect();
        let mut notes = Vec::new();
        for c in &g.collisions {
            let who: Vec<String> = c.claims.iter().map(|(m, t)| format!("{m} ({t})")).collect();
            notes.push(format!(
                "The tool name {} is claimed by {}; none of them is offered to the agent.",
                c.name,
                who.join(" and ")
            ));
        }
        for t in &g.too_long {
            notes.push(format!(
                "The tool {} of {} is left out: {} is longer than {} characters.",
                t.tool,
                t.mod_id,
                t.name,
                crate::tools::MAX_AGENT_NAME
            ));
        }
        if g.read_only {
            notes.push(
                "mods.json is a version this Flare does not read, so switches are not saved."
                    .into(),
            );
        }
        Snapshot {
            scanned: g.scanned,
            mods,
            errors: g.errors.clone(),
            notes,
        }
    }

    /// The found mod (a copy), for the window to serve and show.
    pub fn found(&self, id: &str) -> Option<Found> {
        self.lock()
            .found
            .iter()
            .find(|f| f.manifest.id == id)
            .cloned()
    }

    /// The grant a presented token holds; None for a token that is not live (a disabled mod's, or a guess).
    pub fn grant_of_token(&self, token: &str) -> Option<Grant> {
        self.lock().tokens.grant(token)
    }

    /// The grant of the mod's own token right now; None when the mod is off (its token is dead). The window
    /// uses this for the calls its page makes over the bridge: the page never holds the token.
    pub fn grant_of_mod(&self, id: &str) -> Option<Grant> {
        let g = self.lock();
        let token = g.tokens.token_of(id)?.to_string();
        g.tokens.grant(&token)
    }

    /// The tools the agent sees now: every tool of every mod that is on, sorted by name.
    pub fn tools(&self) -> Vec<Route> {
        self.lock().table.tools().cloned().collect()
    }

    /// Routes the agent's call of `name` to its mod. The page is told by the window; the answer comes back
    /// through [`Hub::respond`] and is awaited with [`Hub::wait`].
    pub fn begin_call(&self, name: &str, input: Value) -> Result<Call, CtlError> {
        if !input.is_object() {
            return Err(CtlError::new(code::BAD_PARAMS, "input must be an object"));
        }
        let mut g = self.lock();
        let Some(route) = g.table.route(name).cloned() else {
            return Err(CtlError::new(
                code::NOT_FOUND,
                format!("no mod offers a tool called {name}"),
            ));
        };
        let may = g
            .tokens
            .token_of(&route.mod_id)
            .and_then(|t| g.tokens.grant(t))
            .is_some_and(|gr| matches!(&gr, Grant::Mod { permissions, .. } if permissions.contains(&Permission::Tools)));
        if !may {
            return Err(CtlError::new(
                code::UNAVAILABLE,
                format!("the mod {} is not turned on", route.mod_id),
            ));
        }
        if g.pending
            .values()
            .filter(|p| p.mod_id == route.mod_id)
            .count()
            >= MAX_PENDING_PER_MOD
        {
            return Err(CtlError::new(
                code::RATE_LIMITED,
                format!("the mod {} has too many calls waiting", route.mod_id),
            ));
        }
        // A call id is 128 random bits: a page cannot guess the id of a call it was not handed.
        let id = format!(
            "c{}",
            crate::tokens::new_token()
                .map_err(|e| CtlError::new(code::INTERNAL, format!("no call id: {e}")))?
                .get(..32)
                .unwrap_or_default()
        );
        let (tx, rx) = mpsc::channel();
        g.pending.insert(
            id.clone(),
            Pending {
                mod_id: route.mod_id.clone(),
                tx,
            },
        );
        Ok(Call {
            id,
            mod_id: route.mod_id,
            tool: route.tool,
            input,
            rx,
        })
    }

    /// The mod's page answered a call. Only the mod the call was routed to may answer it, once.
    pub fn respond(
        &self,
        mod_id: &str,
        call_id: &str,
        outcome: Result<Value, String>,
    ) -> Result<(), CtlError> {
        let mut g = self.lock();
        match g.pending.get(call_id) {
            Some(p) if p.mod_id == mod_id => {}
            _ => {
                return Err(CtlError::new(
                    code::NOT_FOUND,
                    "no open call with that call_id (it was answered, timed out, or is another mod's)",
                ));
            }
        }
        let p = g.pending.remove(call_id).expect("checked above");
        let answer = outcome.map_err(|why| CtlError::new("tool_error", why));
        // A caller that stopped waiting is not an error for the mod.
        let _ = p.tx.send(answer);
        Ok(())
    }

    /// Ends a call that will not be answered (its page could not be made, or the caller gave up).
    pub fn cancel(&self, call_id: &str) {
        self.lock().pending.remove(call_id);
    }

    /// The ids of the calls waiting on the mod (tests).
    pub fn pending_ids(&self, mod_id: &str) -> Vec<String> {
        self.lock()
            .pending
            .iter()
            .filter(|(_, p)| p.mod_id == mod_id)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Is a call with this id still waiting for its answer (not answered, cancelled, failed or timed out)?
    pub fn is_pending(&self, call_id: &str) -> bool {
        self.lock().pending.contains_key(call_id)
    }

    /// Fails every call waiting on `mod_id` (its panel closed, its page died, it was turned off).
    pub fn fail_calls(&self, mod_id: &str, why: &str) -> usize {
        let mut g = self.lock();
        Self::fail_calls_locked(&mut g, mod_id, why)
    }

    fn fail_calls_locked(g: &mut Inner, mod_id: &str, why: &str) -> usize {
        let ids: Vec<String> = g
            .pending
            .iter()
            .filter(|(_, p)| p.mod_id == mod_id)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &ids {
            if let Some(p) = g.pending.remove(id) {
                let _ =
                    p.tx.send(Err(CtlError::new(code::UNAVAILABLE, why.to_string())));
            }
        }
        ids.len()
    }

    /// Fails every call waiting on any mod (the quit).
    pub fn fail_all_calls(&self, why: &str) {
        let mut g = self.lock();
        let ids: Vec<String> = g.pending.values().map(|p| p.mod_id.clone()).collect();
        for id in ids {
            Self::fail_calls_locked(&mut g, &id, why);
        }
    }

    /// How many calls are waiting on the mod.
    pub fn pending_of(&self, mod_id: &str) -> usize {
        self.lock()
            .pending
            .values()
            .filter(|p| p.mod_id == mod_id)
            .count()
    }

    /// Waits for the page's answer: at most `timeout`, looking at `stopping` every slice. A call that is not
    /// answered in time is ended and answers `timeout`.
    pub fn wait(
        &self,
        call: Call,
        timeout: Duration,
        stopping: &dyn Fn() -> bool,
    ) -> Result<Value, CtlError> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            // sync-ok: a worker thread of the control channel waits for the mod's page, never the UI thread
            match call.rx.recv_timeout(left.min(SLICE)) {
                Ok(answer) => return answer,
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(CtlError::new(
                        code::UNAVAILABLE,
                        "the call was dropped before the mod answered",
                    ));
                }
                Err(RecvTimeoutError::Timeout) => {
                    if stopping() {
                        self.cancel(&call.id);
                        return Err(CtlError::new(code::UNAVAILABLE, "Flare is shutting down"));
                    }
                    if left.is_zero() {
                        self.cancel(&call.id);
                        return Err(CtlError::new(
                            code::TIMEOUT,
                            format!(
                                "the mod {} did not answer {} within {} seconds",
                                call.mod_id,
                                call.tool,
                                timeout.as_secs()
                            ),
                        ));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn mod_json(id: &str, perms: &str, tools: &str) -> String {
        format!(
            r#"{{"id":"{id}","name":"Mod {id}","version":"1.0.0","protocol":1,"description":"d","permissions":{perms},"panel":{{"entry":"index.html","title":"P"}},"tools":{tools}}}"#
        )
    }

    fn tool(name: &str) -> String {
        format!(r#"{{"name":"{name}","description":"d","input_schema":{{"type":"object"}}}}"#)
    }

    struct World {
        _dir: tempfile::TempDir,
        hub: Hub,
        root: PathBuf,
    }

    fn world(mods: &[(&str, &str, &str)]) -> World {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("mods");
        for (id, perms, tools) in mods {
            let f = root.join(id);
            std::fs::create_dir_all(&f).unwrap(); // sync-ok: test code only
            std::fs::write(f.join("mod.json"), mod_json(id, perms, tools)).unwrap(); // sync-ok: test code only
            std::fs::write(f.join("index.html"), "<p>x</p>").unwrap(); // sync-ok: test code only
        }
        let hub = Hub::new(Config {
            dir: root.clone(),
            state_file: dir.path().join("mods.json"),
        });
        World {
            _dir: dir,
            hub,
            root,
        }
    }

    fn notes_world() -> World {
        let t = format!("[{}]", tool("append_note"));
        world(&[("notes", r#"["panel","tools"]"#, &t)])
    }

    #[test]
    fn every_permission_a_manifest_may_ask_has_plain_words() {
        for p in crate::manifest::ASKABLE {
            let w = permission_words(p);
            assert!(!w.is_empty() && !w.contains('\u{2014}'), "{p:?}");
            assert!(w.split_whitespace().count() <= 8, "{w}");
        }
    }

    #[test]
    fn a_scan_lists_mods_off_and_without_tokens_or_tools() {
        let w = notes_world();
        let c = w.hub.scan();
        assert_eq!(c, Change::default());
        let s = w.hub.snapshot();
        assert!(s.scanned);
        assert_eq!(s.mods.len(), 1);
        assert!(!s.mods[0].enabled, "off until the user turns it on");
        assert!(w.hub.tools().is_empty());
        assert!(w.hub.grant_of_mod("notes").is_none());
    }

    #[test]
    fn turning_on_mints_a_token_and_offers_the_tools_and_turning_off_kills_both() {
        let w = notes_world();
        w.hub.scan();
        let c = w.hub.set_enabled("notes", true).unwrap();
        assert!(c.tools_changed);
        assert!(c.revoked.is_empty());
        let g = w.hub.grant_of_mod("notes").expect("a token");
        assert_eq!(
            g,
            Grant::Mod {
                id: "notes".into(),
                permissions: vec![Permission::Panel, Permission::Tools]
            }
        );
        assert_eq!(w.hub.tools().len(), 1);
        assert_eq!(w.hub.tools()[0].name, "mod_notes_append_note");
        let c = w.hub.set_enabled("notes", false).unwrap();
        assert!(c.tools_changed);
        assert_eq!(c.revoked, vec!["notes".to_string()]);
        assert!(w.hub.grant_of_mod("notes").is_none());
        assert!(w.hub.tools().is_empty());
    }

    #[test]
    fn a_token_presented_after_the_mod_is_turned_off_is_dead() {
        let w = notes_world();
        w.hub.scan();
        w.hub.set_enabled("notes", true).unwrap();
        // The only way to get the token is from inside the hub, as the window does for its own page.
        let token = {
            let g = w.hub.lock();
            g.tokens.token_of("notes").unwrap().to_string()
        };
        assert!(w.hub.grant_of_token(&token).is_some());
        w.hub.set_enabled("notes", false).unwrap();
        assert!(w.hub.grant_of_token(&token).is_none());
        // A new token after turning it on again is a different one.
        w.hub.set_enabled("notes", true).unwrap();
        assert!(w.hub.grant_of_token(&token).is_none());
    }

    #[test]
    fn a_mod_that_is_not_found_cannot_be_turned_on() {
        let w = notes_world();
        w.hub.scan();
        assert!(w.hub.set_enabled("ghost", true).is_err());
        assert!(w.hub.set_enabled("ghost", false).is_ok());
    }

    #[test]
    fn the_switch_is_saved_and_a_restart_mints_new_tokens_for_the_mods_still_on() {
        let w = notes_world();
        w.hub.scan();
        w.hub.set_enabled("notes", true).unwrap();
        w.hub.persist("notes").unwrap();
        let old = {
            let g = w.hub.lock();
            g.tokens.token_of("notes").unwrap().to_string()
        };
        let again = Hub::new(w.hub.config().clone());
        let c = again.scan();
        assert!(c.tools_changed, "the tools come back");
        assert!(again.is_enabled("notes"));
        assert!(again.grant_of_mod("notes").is_some());
        assert!(
            again.grant_of_token(&old).is_none(),
            "a restart mints new tokens"
        );
        let text = std::fs::read_to_string(&w.hub.config().state_file).unwrap(); // sync-ok: test code only
        assert!(!text.contains(&old), "tokens are never written");
    }

    #[test]
    fn a_save_writes_the_state_as_it_stands_so_a_late_save_cannot_undo_a_later_switch() {
        let w = notes_world();
        w.hub.scan();
        w.hub.set_enabled("notes", true).unwrap();
        w.hub.set_enabled("notes", false).unwrap();
        // The first save to run is the one for "on", but the mod is off now: the file says off, and a second
        // save for the same mod writes the same.
        w.hub.persist("notes").unwrap();
        w.hub.persist("notes").unwrap();
        let text = std::fs::read_to_string(&w.hub.config().state_file).unwrap(); // sync-ok: test code only
        assert!(text.contains("\"notes\":{\"on\":false}"), "{text}");
        w.hub.set_enabled("notes", true).unwrap();
        w.hub.persist("notes").unwrap();
        let text = std::fs::read_to_string(&w.hub.config().state_file).unwrap(); // sync-ok: test code only
        assert!(text.contains("\"notes\":{\"on\":true"), "{text}");
    }

    #[test]
    fn a_rescan_keeps_a_switch_the_file_does_not_hold_yet() {
        let w = notes_world();
        w.hub.scan();
        w.hub.set_enabled("notes", true).unwrap(); // not persisted yet
        let c = w.hub.scan();
        assert!(!c.tools_changed);
        assert!(w.hub.is_enabled("notes"));
        assert!(w.hub.grant_of_mod("notes").is_some());
    }

    #[test]
    fn a_mod_that_vanishes_or_breaks_loses_its_token_at_the_next_scan() {
        let w = notes_world();
        w.hub.scan();
        w.hub.set_enabled("notes", true).unwrap();
        std::fs::write(w.root.join("notes").join("mod.json"), "{").unwrap(); // sync-ok: test code only
        let c = w.hub.scan();
        assert_eq!(c.revoked, vec!["notes".to_string()]);
        assert!(c.tools_changed);
        assert!(w.hub.grant_of_mod("notes").is_none());
        let s = w.hub.snapshot();
        assert!(s.mods.is_empty());
        assert_eq!(s.errors.len(), 1);
        assert_eq!(s.errors[0].folder, "notes");
    }

    fn rewrite(w: &World, perms: &str) {
        let t = if perms.contains("tools") {
            format!("[{}]", tool("append_note"))
        } else {
            "[]".to_string()
        };
        std::fs::write(
            w.root.join("notes").join("mod.json"),
            mod_json("notes", perms, &t),
        )
        .unwrap(); // sync-ok: test code only
    }

    #[test]
    fn a_mod_that_asks_for_more_is_turned_off_and_its_token_dies() {
        let w = notes_world();
        w.hub.scan();
        w.hub.set_enabled("notes", true).unwrap();
        w.hub.persist("notes").unwrap();
        let old = w.hub.lock().tokens.token_of("notes").unwrap().to_string();
        rewrite(&w, r#"["panel","tools","browser"]"#);
        let c = w.hub.scan();
        assert_eq!(c.revoked, vec!["notes".to_string()]);
        assert!(c.tools_changed);
        assert!(!w.hub.is_enabled("notes"));
        assert!(w.hub.grant_of_mod("notes").is_none());
        assert!(
            w.hub.grant_of_token(&old).is_none(),
            "live connections are refused"
        );
        assert!(w.hub.tools().is_empty());
        let s = w.hub.snapshot();
        assert!(!s.mods[0].enabled);
        assert!(s.mods[0].needs_approval);
        // A restart keeps it off: the file still holds the old approval.
        let again = Hub::new(w.hub.config().clone());
        again.scan();
        assert!(!again.is_enabled("notes"));
        assert!(again.snapshot().mods[0].needs_approval);
    }

    #[test]
    fn switching_it_on_again_approves_the_new_list() {
        let w = notes_world();
        w.hub.scan();
        w.hub.set_enabled("notes", true).unwrap();
        w.hub.persist("notes").unwrap();
        rewrite(&w, r#"["panel","tools","view"]"#);
        w.hub.scan();
        assert!(!w.hub.is_enabled("notes"));
        w.hub.set_enabled("notes", true).unwrap();
        w.hub.persist("notes").unwrap();
        assert!(!w.hub.snapshot().mods[0].needs_approval);
        match w.hub.grant_of_mod("notes").unwrap() {
            Grant::Mod { permissions, .. } => assert!(permissions.contains(&Permission::View)),
            other => panic!("{other:?}"),
        }
        w.hub.scan();
        assert!(
            w.hub.is_enabled("notes"),
            "the new list is the approved one"
        );
        let again = Hub::new(w.hub.config().clone());
        again.scan();
        assert!(again.is_enabled("notes"));
    }

    #[test]
    fn a_mod_that_asks_for_less_stays_on_with_the_smaller_set() {
        let w = notes_world();
        w.hub.scan();
        w.hub.set_enabled("notes", true).unwrap();
        w.hub.persist("notes").unwrap();
        rewrite(&w, r#"["panel"]"#);
        let c = w.hub.scan();
        assert!(w.hub.is_enabled("notes"));
        assert_eq!(
            c.revoked,
            vec!["notes".to_string()],
            "a new token with the smaller set"
        );
        match w.hub.grant_of_mod("notes").unwrap() {
            Grant::Mod { permissions, .. } => assert_eq!(permissions, vec![Permission::Panel]),
            other => panic!("{other:?}"),
        }
        assert!(!w.hub.snapshot().mods[0].needs_approval);
        // The smaller set is what is approved now: asking for the old one again needs the user.
        rewrite(&w, r#"["panel","tools"]"#);
        w.hub.scan();
        assert!(!w.hub.is_enabled("notes"));
        assert!(w.hub.snapshot().mods[0].needs_approval);
    }

    #[test]
    fn tools_added_in_a_new_version_keep_the_mod_on() {
        let w = notes_world();
        w.hub.scan();
        w.hub.set_enabled("notes", true).unwrap();
        let t = format!("[{},{}]", tool("append_note"), tool("read_note"));
        std::fs::write(
            w.root.join("notes").join("mod.json"),
            mod_json("notes", r#"["panel","tools"]"#, &t),
        )
        .unwrap(); // sync-ok: test code only
        let c = w.hub.scan();
        assert!(c.tools_changed);
        assert!(c.revoked.is_empty());
        assert!(w.hub.is_enabled("notes"));
        assert_eq!(w.hub.tools().len(), 2);
    }

    #[test]
    fn a_switch_approves_the_list_the_row_showed_and_nothing_else() {
        let w = notes_world();
        w.hub.scan();
        let shown: Vec<String> = w.hub.snapshot().mods[0]
            .permissions
            .iter()
            .map(|p| permission_words(*p).to_string())
            .collect();
        // The manifest asks for something else by the time the click is handled: nothing is approved.
        let mut stale = shown.clone();
        stale.pop();
        assert!(
            w.hub
                .set_enabled_shown("notes", true, Some(&stale))
                .is_err()
        );
        assert!(!w.hub.is_enabled("notes"));
        assert!(w.hub.grant_of_mod("notes").is_none());
        // The list that was shown, as it still stands, is approved.
        w.hub
            .set_enabled_shown("notes", true, Some(&shown))
            .unwrap();
        assert!(w.hub.is_enabled("notes"));
        // Turning off never needs the list.
        w.hub.set_enabled_shown("notes", false, Some(&[])).unwrap();
        assert!(!w.hub.is_enabled("notes"));
    }

    #[test]
    fn call_ids_are_long_and_never_repeat() {
        let w = notes_world();
        w.hub.scan();
        w.hub.set_enabled("notes", true).unwrap();
        let a = w
            .hub
            .begin_call("mod_notes_append_note", json!({}))
            .unwrap();
        let b = w
            .hub
            .begin_call("mod_notes_append_note", json!({}))
            .unwrap();
        assert_ne!(a.id, b.id);
        assert_ne!(a.id, "c1");
        assert_eq!(a.id.len(), 33, "c plus 128 bits in hex");
        assert!(a.id[1..].chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn an_old_format_store_fails_closed() {
        let w = notes_world();
        std::fs::write(
            &w.hub.config().state_file,
            r#"{"version":1,"mods":{"notes":true}}"#,
        )
        .unwrap(); // sync-ok: test code only
        w.hub.scan();
        assert!(!w.hub.is_enabled("notes"));
        assert!(w.hub.grant_of_mod("notes").is_none());
        assert!(w.hub.tools().is_empty());
        assert!(
            !w.hub.snapshot().mods[0].needs_approval,
            "nothing to re-approve: it is just off"
        );
    }

    #[test]
    fn a_call_routes_waits_and_is_answered_by_its_own_mod_once() {
        let w = notes_world();
        w.hub.scan();
        w.hub.set_enabled("notes", true).unwrap();
        let call = w
            .hub
            .begin_call("mod_notes_append_note", json!({"text": "hi"}))
            .unwrap();
        assert_eq!(call.input, json!({"text": "hi"}));
        assert_eq!(
            (call.mod_id.as_str(), call.tool.as_str()),
            ("notes", "append_note")
        );
        // Another mod cannot answer it, and an unknown id is not found.
        assert_eq!(
            w.hub
                .respond("other", &call.id, Ok(json!(1)))
                .unwrap_err()
                .code,
            code::NOT_FOUND
        );
        assert_eq!(
            w.hub
                .respond("notes", "c999", Ok(json!(1)))
                .unwrap_err()
                .code,
            code::NOT_FOUND
        );
        w.hub
            .respond("notes", &call.id, Ok(json!({"count": 1})))
            .unwrap();
        // Answered twice is not found.
        assert_eq!(
            w.hub
                .respond("notes", &call.id, Ok(json!(2)))
                .unwrap_err()
                .code,
            code::NOT_FOUND
        );
        let r = w.hub.wait(call, Duration::from_secs(1), &|| false).unwrap();
        assert_eq!(r, json!({"count": 1}));
        assert_eq!(w.hub.pending_of("notes"), 0);
    }

    #[test]
    fn an_error_answer_reaches_the_caller_with_the_mods_reason() {
        let w = notes_world();
        w.hub.scan();
        w.hub.set_enabled("notes", true).unwrap();
        let call = w
            .hub
            .begin_call("mod_notes_append_note", json!({}))
            .unwrap();
        w.hub
            .respond("notes", &call.id, Err("text is empty".into()))
            .unwrap();
        let e = w
            .hub
            .wait(call, Duration::from_secs(1), &|| false)
            .unwrap_err();
        assert_eq!(e.message, "text is empty");
    }

    #[test]
    fn a_call_for_an_unknown_or_disabled_tool_is_not_found_and_input_must_be_an_object() {
        let w = notes_world();
        w.hub.scan();
        assert_eq!(
            w.hub
                .begin_call("mod_notes_append_note", json!({}))
                .err()
                .unwrap()
                .code,
            code::NOT_FOUND,
            "off: no tool"
        );
        w.hub.set_enabled("notes", true).unwrap();
        assert_eq!(
            w.hub
                .begin_call("append_note", json!({}))
                .err()
                .unwrap()
                .code,
            code::NOT_FOUND
        );
        assert_eq!(
            w.hub
                .begin_call("mod_notes_append_note", json!("x"))
                .err()
                .unwrap()
                .code,
            code::BAD_PARAMS
        );
    }

    #[test]
    fn turning_the_mod_off_fails_the_calls_waiting_on_it_at_once() {
        let w = notes_world();
        w.hub.scan();
        w.hub.set_enabled("notes", true).unwrap();
        let call = w
            .hub
            .begin_call("mod_notes_append_note", json!({}))
            .unwrap();
        w.hub.set_enabled("notes", false).unwrap();
        let e = w
            .hub
            .wait(call, Duration::from_secs(1), &|| false)
            .unwrap_err();
        assert_eq!(e.code, code::UNAVAILABLE);
    }

    #[test]
    fn an_unanswered_call_times_out_and_leaves_nothing_behind() {
        let w = notes_world();
        w.hub.scan();
        w.hub.set_enabled("notes", true).unwrap();
        let call = w
            .hub
            .begin_call("mod_notes_append_note", json!({}))
            .unwrap();
        let id = call.id.clone();
        let e = w
            .hub
            .wait(call, Duration::from_millis(120), &|| false)
            .unwrap_err();
        assert_eq!(e.code, code::TIMEOUT);
        assert_eq!(w.hub.pending_of("notes"), 0);
        assert!(
            w.hub.respond("notes", &id, Ok(json!(1))).is_err(),
            "too late"
        );
    }

    #[test]
    fn a_stop_ends_the_wait_within_a_slice() {
        let w = notes_world();
        w.hub.scan();
        w.hub.set_enabled("notes", true).unwrap();
        let call = w
            .hub
            .begin_call("mod_notes_append_note", json!({}))
            .unwrap();
        let t = Instant::now();
        let e = w
            .hub
            .wait(call, Duration::from_secs(30), &|| true)
            .unwrap_err();
        assert_eq!(e.code, code::UNAVAILABLE);
        assert!(t.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_mod_may_have_only_a_few_calls_waiting() {
        let w = notes_world();
        w.hub.scan();
        w.hub.set_enabled("notes", true).unwrap();
        let mut held = Vec::new();
        for _ in 0..MAX_PENDING_PER_MOD {
            held.push(
                w.hub
                    .begin_call("mod_notes_append_note", json!({}))
                    .unwrap(),
            );
        }
        assert_eq!(
            w.hub
                .begin_call("mod_notes_append_note", json!({}))
                .err()
                .unwrap()
                .code,
            code::RATE_LIMITED
        );
        drop(held);
    }

    #[test]
    fn a_clash_between_two_mods_leaves_both_out_and_says_so() {
        let a = format!("[{}]", tool("d_e"));
        let b = format!("[{}]", tool("e"));
        let w = world(&[
            ("ab-c", r#"["panel","tools"]"#, &a),
            ("ab-c-d", r#"["panel","tools"]"#, &b),
        ]);
        w.hub.scan();
        w.hub.set_enabled("ab-c", true).unwrap();
        w.hub.set_enabled("ab-c-d", true).unwrap();
        assert!(w.hub.tools().is_empty());
        let s = w.hub.snapshot();
        assert!(
            s.notes.iter().any(|n| n.contains("mod_ab_c_d_e")),
            "{:?}",
            s.notes
        );
    }

    #[test]
    fn a_missing_mods_folder_is_simply_no_mods() {
        let dir = tempfile::tempdir().unwrap();
        let hub = Hub::new(Config {
            dir: dir.path().join("nope"),
            state_file: dir.path().join("mods.json"),
        });
        hub.scan();
        let s = hub.snapshot();
        assert!(s.scanned && s.mods.is_empty() && s.errors.is_empty());
    }

    #[test]
    fn the_snapshot_and_debug_never_carry_a_token() {
        let w = notes_world();
        w.hub.scan();
        w.hub.set_enabled("notes", true).unwrap();
        let token = {
            let g = w.hub.lock();
            g.tokens.token_of("notes").unwrap().to_string()
        };
        let text = format!("{:?}", w.hub.snapshot());
        assert!(!text.contains(&token));
    }
}
