//! The mods hub (stage 8, the app part): everything the app knows about mods and plugins, behind one lock, with
//! no window and no browser. The Settings page, the control channel's dispatcher and the window all hold the
//! same `Arc<Hub>`.
//!
//! - **Discovery** (`scan`) reads the plugins folder and the switch file on a worker (the first scan moves the old
//!   mods folder's folders into the plugins folder before it reads); the answer replaces what the hub knows. A scan
//!   never turns a mod on: the switches are the user's. A protocol 2 plugin that is on stays on only while its
//!   manifest asks for nothing new AND names the author and homepage that were approved (design section 13).
//! - **Switches** (`set_enabled`) are applied in memory at once, on any thread: turning a mod ON mints its
//!   token, turning it OFF kills the token (a connection that presents it is refused on its next request) and
//!   fails every tool call waiting on the mod. The file is written by `persist`, on a worker (rule 34).
//! - **Tool routing** (`begin_call`, `respond`, `wait`) keeps the calls the agent made and a mod's page has
//!   not answered yet. A call can only be answered by the mod it was routed to; a mod that is disabled has no
//!   tools; a call never waits longer than [`CALL_TIMEOUT`].
//!
//! Tokens are secrets: nothing here returns one except [`Hub::grant_of_token`]'s grant and
//! [`Hub::grant_of_mod`]'s grant, and no `Debug` or `Snapshot` carries one.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use flare_ctl::{CtlError, Grant, Permission, code};
use serde_json::Value;

use crate::discover::{FolderError, Found, discover};
use crate::enabled::{EnabledStore, Identity};
use crate::grants::Choices;
use crate::limit::Limits;
use crate::tokens::Tokens;
use crate::tools::{Collision, Route, TooLong, ToolTable};

/// How long the agent's tool call waits for the mod's page to answer.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Calls one mod may have waiting at once.
pub const MAX_PENDING_PER_MOD: usize = 8;
/// Plugins with a background page that may be on at once (design 10).
pub const MAX_BACKGROUND: usize = 6;
/// How often a waiting call looks at the stop flag.
const SLICE: Duration = Duration::from_millis(50);

/// What a permission lets a mod do, in the plain words Settings > Plugins shows before the person turns the mod on.
/// `eval` is never granted to a mod; its words say so if it ever reaches a list. The two full-control
/// permissions say so in their words, and [`permission_warnings`] says it again.
pub fn permission_words(p: Permission) -> &'static str {
    match p {
        Permission::Panel => "Show its panel in a tab",
        Permission::View => "Show images, files and text",
        Permission::Browser => "Open and use its own web page",
        Permission::Convo => "Read the agent's changes",
        Permission::Tools => "Answer the agent's tool calls",
        Permission::Eval => "Run scripts in pages (never allowed to a mod)",
        Permission::Decorate => "Put badges and progress on tabs",
        Permission::Tabs => "See all your tabs and what their agents are doing",
        Permission::Sidebar => "Add a section to the sidebar",
        Permission::Actions => "Add buttons to tabs and menus",
        Permission::Bar => "Show a bar under the terminal",
        Permission::Sessions => "Read what you and Claude say in sessions",
        Permission::Input => "Type and run commands in your terminals (full control)",
        Permission::Network => "Use the internet",
        Permission::Process => "Run its own program on this PC (full access)",
        Permission::AgentLooks => "Change how agent icons move while they work",
        Permission::Fill => "Fill logins into your web panel",
    }
}

/// The sentence for `process`: a program that runs as the person, with everything that means.
pub const WARN_PROCESS: &str = "Full control: its program runs as you with full access to this PC, and can leave things behind outside Flare.";
/// The sentence for `input`, in the same tier as `process`.
pub const WARN_INPUT: &str =
    "Full control: it can type and run commands in your terminals, as if you typed them.";
/// `input` and `sessions` together.
pub const WARN_INPUT_SESSIONS: &str =
    "With both, it can read what Claude says and steer Claude by typing to it.";
/// `network` and `sessions` together.
pub const WARN_NETWORK_SESSIONS: &str = "It can send what you and Claude say to the internet.";
/// `network` and `tabs` together.
pub const WARN_NETWORK_TABS: &str =
    "It can send what your tabs show, their names and folders, to the internet.";
/// `network` and `fill` together: what the person types into Flare's secret field goes to the plugin's pages.
pub const WARN_NETWORK_FILL: &str = "It can send the passwords you type for it to the internet.";
/// `network` alone.
pub const WARN_NETWORK: &str = "It can change what it does from the internet.";
/// `fill`: the card says what it can do, and that Flare asks each time.
pub const WARN_FILL: &str = "Can type passwords into pages, after you approve each one.";

/// The warnings Settings shows with a plugin's permissions, most serious first: the full-control ones for `process`
/// and `input`, then the combinations that reach further than either permission alone, then `network`'s own, then
/// `fill`'s.
pub fn permission_warnings(perms: &[Permission]) -> Vec<&'static str> {
    let has = |p: Permission| perms.contains(&p);
    let mut out = Vec::new();
    if has(Permission::Process) {
        out.push(WARN_PROCESS);
    }
    if has(Permission::Input) {
        out.push(WARN_INPUT);
    }
    if has(Permission::Input) && has(Permission::Sessions) {
        out.push(WARN_INPUT_SESSIONS);
    }
    if has(Permission::Network) && has(Permission::Sessions) {
        out.push(WARN_NETWORK_SESSIONS);
    }
    if has(Permission::Network) && has(Permission::Tabs) {
        out.push(WARN_NETWORK_TABS);
    }
    if has(Permission::Network) && has(Permission::Fill) {
        out.push(WARN_NETWORK_FILL);
    }
    if has(Permission::Network) {
        out.push(WARN_NETWORK);
    }
    if has(Permission::Fill) {
        out.push(WARN_FILL);
    }
    out
}

/// Whether the list holds a permission that hands over full control (`process`, `input`).
pub fn is_full_control(perms: &[Permission]) -> bool {
    perms
        .iter()
        .any(|p| matches!(p, Permission::Process | Permission::Input))
}

/// What the approval of `f` keeps about who made it: where Settings installed it from (`installed`), and for a plugin
/// also the manifest's author and homepage (the hash comes later, from `persist`). A protocol 1 mod dropped in the
/// folder by hand keeps nothing, as before.
fn identity_of(f: &Found, installed: Option<&Identity>) -> Identity {
    let source = installed.and_then(|i| i.source.clone());
    if !f.manifest.is_plugin() {
        return Identity {
            source,
            ..Identity::default()
        };
    }
    Identity {
        author: f.manifest.author.clone(),
        homepage: f.manifest.homepage.clone(),
        source,
        ..Identity::default()
    }
}

/// What a mod's card in Settings showed when it was drawn. Turning the mod on approves exactly this.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Shown {
    /// The permission words the row listed, in order.
    pub permissions: Vec<String>,
    /// Who made it and its address as the card showed them; empty when the manifest names none.
    pub author: String,
    pub homepage: String,
}

/// Where mods live and where the switches are kept.
#[derive(Clone, Debug)]
pub struct Config {
    /// The plugins folder, holding one folder per mod or plugin, both protocols (`%APPDATA%\Flare\plugins`, or the
    /// profile's).
    pub dir: PathBuf,
    /// The old mods folder, `mods` beside it: the first scan moves its folders into `dir` (`migrate`), and it is
    /// never read. `None` moves nothing.
    pub old_mods_dir: Option<PathBuf>,
    /// The switch file (`mods.json`), for mods and plugins alike.
    pub state_file: PathBuf,
}

/// One mod as Settings and the window see it. No secret.
#[derive(Clone, Debug, PartialEq)]
pub struct ModEntry {
    pub id: String,
    /// 1 for a `mod.json`, 2 for a `plugin.json`.
    pub protocol: u64,
    pub name: String,
    pub version: String,
    pub description: String,
    pub permissions: Vec<Permission>,
    /// The panel's title, when the mod has a panel.
    pub panel_title: Option<String>,
    pub tool_count: usize,
    pub enabled: bool,
    /// The mod was on and its manifest now asks for permissions the user never approved (or, for a plugin, names
    /// another author or homepage), so it was turned off. Switching it on again approves the new list.
    pub needs_approval: bool,
    /// Protocol 2: who made it and where it lives, as the manifest says.
    pub author: Option<String>,
    pub homepage: Option<String>,
    /// Protocol 2: it has a background page.
    pub background: bool,
    /// Its background page was stopped (it failed too often, or more are running than may), or its helper program
    /// was (it ended too often, or did not start): the sentence for Settings.
    pub stopped: Option<String>,
    /// Where Settings installed it from (an `https` address or `a file`), when it did (plugins phase 7).
    pub source: Option<String>,
    /// Protocol 2: its colour and logo (the card, its details sheet, its questions and menu item).
    pub brand: Option<crate::manifest::Brand>,
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
    /// Why every plugin is off this run (safe mode), if it is.
    pub safe_mode: Option<String>,
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
    /// The tab the agent called from, when the app said ([`Hub::set_call_tab`]): while the call waits, the plugin may
    /// fill a login into that tab's page (`browser.fill`), as the person's click would let it act there.
    tab: Option<String>,
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
    /// Who each plugin that is on said it was when it was approved (empty for a protocol 1 mod).
    identity: BTreeMap<String, Identity>,
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
    /// Bumped for a mod each time its token dies (turned off, changed, gone): what was started for an older
    /// generation is never applied to the newer one.
    generations: BTreeMap<String, u64>,
    /// Plugins whose background page was stopped by the window (it failed too often), with the sentence Settings
    /// shows. Cleared when the plugin is switched.
    stopped: BTreeMap<String, String>,
    /// Why this run keeps every plugin off (safe mode), if it does.
    safe: Option<String>,
    /// The tabs each plugin may act on because the person clicked one of its actions or sidebar items there
    /// (design 13, "click scope"), oldest click first. Gone when the tab closes or the plugin's token dies.
    clicked: BTreeMap<String, VecDeque<String>>,
    /// Where Settings installed each plugin from, and the hash of what it installed (plugins phase 7), as the switch
    /// file keeps it.
    installed: BTreeMap<String, Identity>,
    /// Plugins being replaced by an update: none of their views or helper may start until it is done.
    paused: BTreeSet<String>,
    /// Plugins whose helper program was stopped (it ended too often, or did not start), with the sentence. Cleared
    /// when the plugin is switched.
    helper_stopped: BTreeMap<String, String>,
    /// What the person held back of each plugin's permissions (the details sheet), kept with the switch, on or off.
    choices: BTreeMap<String, Choices>,
}

/// The tabs one plugin may hold click scope on at once; a click on one more forgets the oldest.
pub const MAX_CLICKED_TABS: usize = 64;

/// What a plugin that is on adds to Flare's own surfaces (plugins phase 4): its sidebar sections when it holds
/// `sidebar`, its actions when it holds `actions`, and its own icons.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Surfaces {
    pub id: String,
    pub name: String,
    pub sidebar: Vec<crate::manifest::SidebarDef>,
    pub actions: Vec<crate::manifest::ActionDef>,
    pub icons: Vec<crate::manifest::IconDef>,
}

pub struct Hub {
    cfg: Config,
    inner: Mutex<Inner>,
    /// The switch file; loaded by the first scan. Its own lock, so a slow disk never holds the hub's.
    store: Mutex<Option<EnabledStore>>,
    /// The folders the move left in the old mods folder, each with its sentence; set once, by the first scan.
    kept: OnceLock<Vec<crate::migrate::Kept>>,
    /// The budget of every protocol 2 plugin that is on (design 13), shared with the window and the browser glue.
    limits: Arc<Limits>,
    /// Each plugin's own store (plugins phase 5), beside the switch file in `plugins-storage`.
    storage: crate::storage::Storage,
    /// Held by a scan, an install, an update and a removal while they touch the plugins folder: they run one at a
    /// time, whichever worker they are on (design 13, "one worker queue").
    disk: Mutex<()>,
    /// Where a removed or replaced plugin folder goes (the Recycle Bin; the tests delete it).
    recycle: Mutex<Recycle>,
    /// The browser profile folders of a plugin, for a removal to delete (set by the app; none in the tests).
    profiles: Mutex<Option<ProfileDirs>>,
    /// The asks of the `fill` permission, in memory only (a restart forgets a "Deny for this session").
    asks: Mutex<crate::fill::Asks>,
}

/// What takes a removed plugin folder.
pub type Recycle = Arc<dyn Fn(&Path) -> Result<(), String> + Send + Sync>;
/// The browser profile folders of a plugin, by id.
pub type ProfileDirs = Arc<dyn Fn(&str) -> Vec<PathBuf> + Send + Sync>;

mod lifecycle;
pub use lifecycle::{Installed, StagedInstall, logs_dir};

/// Where the plugins' stores are kept: `plugins-storage` beside the switch file (the profile's folder).
pub fn storage_dir(cfg: &Config) -> PathBuf {
    cfg.state_file
        .parent()
        .map_or_else(|| cfg.dir.join(".storage"), Path::to_path_buf)
        .join("plugins-storage")
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
            inner: Mutex::new(Inner::default()),
            store: Mutex::new(None),
            kept: OnceLock::new(),
            limits: Arc::new(Limits::default()),
            storage: crate::storage::Storage::new(storage_dir(&cfg)),
            disk: Mutex::new(()),
            recycle: Mutex::new(Arc::new(crate::install::recycle)),
            profiles: Mutex::new(None),
            asks: Mutex::new(crate::fill::Asks::default()),
            cfg,
        }
    }

    /// Every plugin's own store (`storage.get`, `storage.set`). Its calls read and write the disk: a worker's.
    pub fn storage(&self) -> &crate::storage::Storage {
        &self.storage
    }

    /// Whether `id` is a protocol 2 plugin the hub found (a protocol 1 mod keeps exactly what it had: no store).
    pub fn is_plugin(&self, id: &str) -> bool {
        self.lock()
            .found
            .iter()
            .any(|f| f.manifest.id == id && f.manifest.is_plugin())
    }

    /// The plugins' budgets: the window spends a page's messages from it, the browser glue looks at it before it
    /// copies one, the dispatcher spends a pipe connection's.
    pub fn limits(&self) -> Arc<Limits> {
        Arc::clone(&self.limits)
    }

    /// Keeps the budgets in step with the plugins that are on (protocol 1 mods have none).
    fn sync_limits(&self) {
        let ids: Vec<String> = {
            let g = self.lock();
            g.found
                .iter()
                .filter(|f| f.manifest.is_plugin() && g.tokens.token_of(&f.manifest.id).is_some())
                .map(|f| f.manifest.id.clone())
                .collect()
        };
        self.limits.keep_only(&ids, Instant::now());
    }

    /// This run keeps every plugin off (safe mode, design 13): set before the first scan, whose switches are then
    /// read but not applied (and not written, so the next run has them back). `why` is the sentence Settings shows.
    pub fn set_safe_mode(&self, why: &str) {
        self.lock().safe = Some(why.to_string());
    }

    /// Why this run keeps the plugins off, if it does.
    pub fn safe_mode(&self) -> Option<String> {
        self.lock().safe.clone()
    }

    /// The mod's generation: it changes each time the mod's token dies (turned off, changed or gone).
    pub fn generation(&self, id: &str) -> u64 {
        self.lock().generations.get(id).copied().unwrap_or(0)
    }

    /// The background page of a protocol 2 plugin that is on: its folder and entry. None for a mod, a plugin with
    /// no background page, or one that is off.
    pub fn background_of(&self, id: &str) -> Option<(PathBuf, String)> {
        let g = self.lock();
        g.tokens.token_of(id)?;
        let f = g.found.iter().find(|f| f.manifest.id == id)?;
        let entry = f
            .manifest
            .background
            .clone()
            .filter(|_| f.manifest.is_plugin())?;
        Some((f.folder.clone(), entry))
    }

    /// The bar of a protocol 2 plugin that is on and holds `bar` in its live grant (plugins phase 6): its folder, its
    /// entry and its height (the manifest's, already 24 to 160). None for a mod, a plugin with no bar, or one that is
    /// off.
    pub fn bar_of(&self, id: &str) -> Option<(PathBuf, String, u32)> {
        let g = self.lock();
        let token = g.tokens.token_of(id)?;
        let Some(Grant::Mod { permissions, .. }) = g.tokens.grant(token) else {
            return None;
        };
        if !permissions.contains(&Permission::Bar) {
            return None;
        }
        let f = g
            .found
            .iter()
            .find(|f| f.manifest.id == id && f.manifest.is_plugin())?;
        let bar = f.manifest.bar.as_ref()?;
        let height = bar.height.clamp(
            crate::manifest::MIN_BAR_HEIGHT as u32,
            crate::manifest::MAX_BAR_HEIGHT as u32,
        );
        Some((f.folder.clone(), bar.entry.clone(), height))
    }

    /// Whether the plugin's pages may reach the internet: a protocol 2 plugin that is on and holds `network` in its live
    /// grant (plugins phase 6). A protocol 1 mod never does.
    pub fn network_of(&self, id: &str) -> bool {
        let g = self.lock();
        let plugin = g
            .found
            .iter()
            .any(|f| f.manifest.id == id && f.manifest.is_plugin());
        plugin
            && g.tokens.token_of(id).and_then(|t| g.tokens.grant(t)).is_some_and(
                |gr| matches!(gr, Grant::Mod { permissions, .. } if permissions.contains(&Permission::Network)),
            )
    }

    /// The protocol 2 plugins that are on and have a background page, by id.
    pub fn with_background(&self) -> Vec<String> {
        let g = self.lock();
        g.found
            .iter()
            .filter(|f| {
                f.manifest.is_plugin()
                    && f.manifest.background.is_some()
                    && g.tokens.token_of(&f.manifest.id).is_some()
                    && !g.paused.contains(&f.manifest.id)
            })
            .map(|f| f.manifest.id.clone())
            .collect()
    }

    /// The agent's calls of this mod's tools are answered by its background page (a protocol 2 plugin that has
    /// one), not by a panel opened in the caller's tab.
    pub fn answers_in_background(&self, id: &str) -> bool {
        let g = self.lock();
        g.found.iter().any(|f| {
            f.manifest.id == id && f.manifest.is_plugin() && f.manifest.background.is_some()
        })
    }

    /// Whether the plugin's tools are answered by its helper: a protocol 2 plugin with a helper and neither a
    /// background page nor a panel (whose pages would answer them first). The helper hears the `tool` event on its
    /// connection and answers with `tools.respond`.
    pub fn answers_with_helper(&self, id: &str) -> bool {
        let g = self.lock();
        g.found.iter().any(|f| {
            f.manifest.id == id
                && f.manifest.is_plugin()
                && f.manifest.background.is_none()
                && f.manifest.panel.is_none()
                && f.manifest.helper.is_some()
        })
    }

    /// The window stopped the plugin's background page (`Some(why)`), or started it again (`None`).
    pub fn set_stopped(&self, id: &str, why: Option<&str>) {
        let mut g = self.lock();
        match why {
            Some(w) => g.stopped.insert(id.to_string(), w.to_string()),
            None => g.stopped.remove(id),
        };
    }

    /// Why the plugin's background page is stopped, if it is.
    pub fn stopped(&self, id: &str) -> Option<String> {
        self.lock().stopped.get(id).cloned()
    }

    /// The plugin's name for a person (its manifest's), or the id when it is not found.
    pub fn name_of(&self, id: &str) -> String {
        self.lock()
            .found
            .iter()
            .find(|f| f.manifest.id == id)
            .map_or_else(|| id.to_string(), |f| f.manifest.name.clone())
    }

    /// The SVG path of the plugin's own icon `name` (its `contributes.icons`); None when it declares none of that name.
    pub fn icon_of(&self, id: &str, name: &str) -> Option<String> {
        self.lock()
            .found
            .iter()
            .find(|f| f.manifest.id == id)?
            .manifest
            .contributes
            .icons
            .iter()
            .find(|i| i.name == name)
            .map(|i| i.path.clone())
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// What every protocol 2 plugin that is on and not stopped adds to the sidebar and the action places, in the
    /// order the scan found them. A part needs its permission in the live grant (the manifest asks for it too).
    pub fn surfaces(&self) -> Vec<Surfaces> {
        let g = self.lock();
        g.found
            .iter()
            .filter(|f| f.manifest.is_plugin() && !g.stopped.contains_key(&f.manifest.id))
            .filter_map(|f| {
                let m = &f.manifest;
                let token = g.tokens.token_of(&m.id)?;
                let Some(Grant::Mod { permissions, .. }) = g.tokens.grant(token) else {
                    return None;
                };
                let sidebar = if permissions.contains(&Permission::Sidebar) {
                    m.contributes.sidebar.clone()
                } else {
                    Vec::new()
                };
                let actions = if permissions.contains(&Permission::Actions) {
                    m.contributes.actions.clone()
                } else {
                    Vec::new()
                };
                if sidebar.is_empty() && actions.is_empty() {
                    return None;
                }
                Some(Surfaces {
                    id: m.id.clone(),
                    name: m.name.clone(),
                    sidebar,
                    actions,
                    icons: m.contributes.icons.clone(),
                })
            })
            .collect()
    }

    /// Whether the plugin's manifest declares the sidebar section `key`.
    pub fn declares_section(&self, id: &str, key: &str) -> bool {
        self.lock().found.iter().any(|f| {
            f.manifest.id == id && f.manifest.contributes.sidebar.iter().any(|s| s.key == key)
        })
    }

    /// Whether the plugin's manifest declares the action `key`.
    pub fn declares_action(&self, id: &str, key: &str) -> bool {
        self.lock().found.iter().any(|f| {
            f.manifest.id == id && f.manifest.contributes.actions.iter().any(|a| a.key == key)
        })
    }

    /// The person clicked one of the plugin's actions or sidebar items on `tab`: the plugin may act on that tab
    /// from now on, as if it held `tabs` for it alone, until the tab closes or the plugin's token dies (design 13).
    /// A plugin that is off gets nothing.
    pub fn grant_click_scope(&self, id: &str, tab: &str) {
        let mut g = self.lock();
        if g.tokens.token_of(id).is_none() {
            return;
        }
        let list = g.clicked.entry(id.to_string()).or_default();
        // A repeat click makes the tab the newest again.
        list.retain(|t| t != tab);
        if list.len() >= MAX_CLICKED_TABS {
            list.pop_front();
        }
        list.push_back(tab.to_string());
    }

    /// Whether a click gave the plugin scope on `tab` ([`Hub::grant_click_scope`]).
    pub fn has_click_scope(&self, id: &str, tab: &str) -> bool {
        self.lock()
            .clicked
            .get(id)
            .is_some_and(|list| list.iter().any(|t| t == tab))
    }

    /// The tab closed: no plugin keeps scope on it.
    pub fn forget_tab(&self, tab: &str) {
        let mut g = self.lock();
        for list in g.clicked.values_mut() {
            list.retain(|t| t != tab);
        }
        g.clicked.retain(|_, list| !list.is_empty());
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Reads the plugins folder (and, the first time, moves the old mods folder into it and reads the switch file).
    /// Runs on a worker. Replaces what the hub knew; mods that were turned on and are still valid keep their token,
    /// the others lose it. What the move had to leave behind is reported after the folders that were read, for as
    /// long as it is still there.
    ///
    /// An id whose old folder stayed because the plugins folder holds one of the same name is turned off by the first
    /// scan, and saved off: its switch was given to the old folder, not to the one now read (design section 13).
    pub fn scan(&self) -> Change {
        let _disk = self.disk.lock().unwrap_or_else(|e| e.into_inner());
        self.scan_locked()
    }

    /// [`Hub::scan`] with the disk lock already held.
    fn scan_locked(&self) -> Change {
        let mut clashed: Vec<String> = Vec::new();
        let kept = self.kept.get_or_init(|| {
            // What an install of an earlier run left half done is cleared before anything is read.
            crate::install::clean_staging(&self.cfg.dir); // sync-ok: runs on the scan worker
            let Some(old) = &self.cfg.old_mods_dir else {
                return Vec::new();
            };
            let m = crate::migrate::move_into_plugins(old, &self.cfg.dir); // sync-ok: runs on the scan worker
            if !m.moved.is_empty() {
                log::info!(
                    "moved {} from the old mods folder into the plugins folder",
                    m.moved.join(", ")
                );
            }
            clashed = m
                .kept
                .iter()
                .filter(|k| k.clash)
                .map(|k| k.error.folder.clone())
                .collect();
            m.kept
        });
        let mut found = discover(&self.cfg.dir);
        // The sentence says to move or remove the folder; once that is done it is no longer said.
        found.errors.extend(
            kept.iter()
                .filter(|k| std::fs::symlink_metadata(&k.path).is_ok()) // sync-ok: runs on the scan worker
                .map(|k| k.error.clone()),
        );
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
        let (store_ids, read_only, installed, choices) = {
            let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
            store
                .as_ref()
                .map(|s| {
                    let ids: Vec<(String, Vec<Permission>, Identity)> = s
                        .enabled_ids()
                        .into_iter()
                        .filter_map(|id| {
                            let who = s.identity(&id).unwrap_or_default();
                            s.approved(&id).map(|a| (id, a, who))
                        })
                        .collect();
                    (ids, s.is_read_only(), s.all_installed(), s.all_choices())
                })
                .unwrap_or_default()
        };
        let mut g = self.lock();
        if first_load {
            g.installed = installed.into_iter().collect();
            g.choices = choices.into_iter().collect();
            // Safe mode reads the switches and applies none of them: nothing is on this run, nothing is written.
            if g.safe.is_none() {
                g.on = store_ids.iter().map(|(id, ..)| id.clone()).collect();
            }
            g.identity = store_ids
                .iter()
                .map(|(id, _, who)| (id.clone(), who.clone()))
                .collect();
            g.approved = store_ids.into_iter().map(|(id, a, _)| (id, a)).collect();
        }
        clashed.retain(|id| g.on.remove(id));
        for id in &clashed {
            g.approved.remove(id);
            g.identity.remove(id);
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
        self.sync_limits();
        // A mod that now asks for less keeps the smaller set on disk too, and a clash turned off stays off (this runs
        // on the scan worker).
        for id in shrunk.iter().chain(&clashed) {
            let _ = self.persist(id);
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
            // A plugin is also approved as who it said it was: another author or homepage is a new approval.
            // So is where Settings installed it from (a protocol 1 mod too): an update from another address is a new
            // approval. And so is another protocol: a plugin's approval (its token, or a hash only a plugin's has)
            // never carries over to a protocol 1 `mod.json` of the same id, nor the other way round.
            let now = identity_of(f, g.installed.get(&id));
            let plugin = f.manifest.is_plugin();
            let same_protocol = g.tokens.plugin_of(&id).is_none_or(|p| p == plugin);
            let same_maker = same_protocol
                && g.identity.get(&id).map_or(!plugin, |who| {
                    who.author == now.author
                        && who.homepage == now.homepage
                        && who.source == now.source
                        && (plugin || who.hash.is_none())
                });
            match g.approved.get(&id).cloned() {
                Some(ok) if same_maker && asked.iter().all(|p| ok.contains(p)) => {
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
        // The live grant is what the manifest asks for less what the person held back on the details sheet: a held
        // back permission is answered exactly like one never asked for.
        let live: Vec<(String, Vec<Permission>, bool)> = g
            .found
            .iter()
            .filter(|f| g.on.contains(&f.manifest.id))
            .map(|f| {
                let m = &f.manifest;
                let grants = g
                    .choices
                    .get(&m.id)
                    .map_or_else(|| m.permissions.clone(), |c| c.grants(&m.permissions));
                (m.id.clone(), grants, m.is_plugin())
            })
            .collect();
        // A token of a mod that is off, gone or invalid dies.
        let dead: Vec<String> = g
            .tokens
            .ids()
            .into_iter()
            .filter(|id| !live.iter().any(|(l, ..)| l == id))
            .collect();
        for id in dead {
            g.tokens.revoke(&id);
            revoked.push(id);
        }
        // A mod that is on gets a token if it has none; one whose permissions or protocol changed gets a new one.
        for (id, perms, plugin) in &live {
            let stale = g
                .tokens
                .token_of(id)
                .and_then(|t| g.tokens.grant(t))
                .is_some_and(|gr| gr != crate::tokens::mod_grant(id, perms, *plugin));
            if stale {
                revoked.push(id.clone());
            }
            if (stale || g.tokens.token_of(id).is_none())
                && g.tokens.mint(id, perms, *plugin).is_err()
            {
                log::warn!("could not make a token for mod {id}; it stays off");
            }
        }
        // The tool table: only mods that are on and hold a token, and that have something to answer the calls: a
        // panel, or for a protocol 2 plugin its background page (which then answers them, design 13) or its helper
        // (2026-10-28).
        // A tool needs `tools` in the live grant, and a panel or a helper answers only while its permission is held.
        let manifests: Vec<&crate::manifest::Manifest> = g
            .found
            .iter()
            .filter(|f| {
                let m = &f.manifest;
                let held = |p: Permission| {
                    live.iter()
                        .any(|(id, perms, _)| *id == m.id && perms.contains(&p))
                };
                let answers = (m.panel.is_some() && held(Permission::Panel))
                    || (m.is_plugin()
                        && (m.background.is_some()
                            || (m.helper.is_some() && held(Permission::Process))));
                g.tokens.token_of(&m.id).is_some() && held(Permission::Tools) && answers
            })
            .map(|f| &f.manifest)
            .collect();
        let (table, collisions, too_long) = ToolTable::build(manifests);
        g.table = table;
        g.collisions = collisions;
        g.too_long = too_long;
        // Calls waiting on a mod that lost its token can never be answered, and whatever was started for it is of
        // an older generation now.
        for id in &revoked {
            *g.generations.entry(id.clone()).or_insert(0) += 1;
            g.clicked.remove(id);
        }
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

    /// As [`Hub::set_enabled`], for a switch the person clicked on a row of Settings. `shown` is what that card
    /// listed (permission words, author, homepage): turning a mod on approves THAT, so when the manifest found now
    /// says anything else (an edit picked up between the card being drawn and the click) nothing changes and the
    /// person is told to look again.
    pub fn set_enabled_shown(
        &self,
        id: &str,
        on: bool,
        shown: Option<&Shown>,
    ) -> Result<Change, String> {
        let mut g = self.lock();
        if on && !g.found.iter().any(|f| f.manifest.id == id) {
            return Err(format!("there is no valid mod \"{id}\" to turn on"));
        }
        if on && let Some(shown) = shown {
            let now: Shown = g
                .found
                .iter()
                .find(|f| f.manifest.id == id)
                .map(|f| Shown {
                    permissions: f
                        .manifest
                        .permissions
                        .iter()
                        .map(|p| permission_words(*p).to_string())
                        .collect(),
                    author: f.manifest.author.clone().unwrap_or_default(),
                    homepage: f.manifest.homepage.clone().unwrap_or_default(),
                })
                .unwrap_or_default();
            if &now != shown {
                return Err(format!(
                    "the mod \"{id}\" changed what it asks for while you were looking; read the list again before turning it on"
                ));
            }
        }
        // At most MAX_BACKGROUND plugins run a background page at once: one more is refused, with the reason.
        if on && !g.on.contains(id) {
            let has_bg = |f: &Found| f.manifest.is_plugin() && f.manifest.background.is_some();
            let wants = g.found.iter().any(|f| f.manifest.id == id && has_bg(f));
            let running = g
                .found
                .iter()
                .filter(|f| has_bg(f) && g.on.contains(&f.manifest.id))
                .count();
            if wants && running >= MAX_BACKGROUND {
                return Err(format!(
                    "at most {MAX_BACKGROUND} plugins with a background page run at once; turn one of them off before \"{id}\""
                ));
            }
        }
        let before_tools: Vec<Route> = g.table.tools().cloned().collect();
        g.needs.remove(id);
        g.stopped.remove(id);
        g.helper_stopped.remove(id);
        if on {
            // Switching on is the approval: the manifest's list as it stands now.
            let installed = g.installed.get(id).cloned();
            let (asked, who) = g
                .found
                .iter()
                .find(|f| f.manifest.id == id)
                .map(|f| {
                    (
                        f.manifest.permissions.clone(),
                        identity_of(f, installed.as_ref()),
                    )
                })
                .unwrap_or_default();
            g.approved.insert(id.to_string(), asked);
            g.identity.insert(id.to_string(), who);
            g.on.insert(id.to_string());
        } else {
            g.approved.remove(id);
            g.identity.remove(id);
            g.on.remove(id);
        }
        let revoked = Self::reconcile(&mut g);
        let after_tools: Vec<Route> = g.table.tools().cloned().collect();
        drop(g);
        self.sync_limits();
        Ok(Change {
            tools_changed: before_tools != after_tools,
            revoked,
        })
    }

    /// Writes the mod's switch to the file, as it stands NOW. Runs on a worker. It reads the state under the
    /// file's lock, so two saves racing each other (a mod turned on and straight off again) both write the latest
    /// state, never a stale one. An error is a sentence for Settings; the switch keeps its effect for this session.
    ///
    /// A plugin that is on and whose approval holds no content hash yet gets one first (its folder's files are read
    /// here, on the worker, before any lock is taken), so the hash kept is of the folder the person approved.
    pub fn persist(&self, id: &str) -> Result<(), String> {
        let unhashed = {
            let g = self.lock();
            let needs = g.on.contains(id) && g.identity.get(id).is_some_and(|w| w.hash.is_none());
            g.found
                .iter()
                .find(|f| f.manifest.id == id && f.manifest.is_plugin())
                .filter(|_| needs)
                .map(|f| f.folder.clone())
        };
        if let Some(folder) = unhashed {
            match crate::hash::folder_hash(&folder) {
                Ok(hash) => {
                    let mut g = self.lock();
                    if g.on.contains(id)
                        && let Some(who) = g.identity.get_mut(id)
                    {
                        who.hash = Some(hash);
                    }
                }
                Err(why) => log::warn!("no content hash for plugin {id}: {why}"),
            }
        }
        let mut store = self.store.lock().unwrap_or_else(|e| e.into_inner());
        let (on, approved, who, choices) = {
            let g = self.lock();
            (
                g.on.contains(id),
                g.approved.get(id).cloned().unwrap_or_default(),
                g.identity.get(id).cloned().unwrap_or_default(),
                g.choices.get(id).cloned().unwrap_or_default(),
            )
        };
        let store = store.get_or_insert_with(|| EnabledStore::load(self.cfg.state_file.clone())); // sync-ok: runs on a worker
        store.set_full(id, on, &approved, &who, &choices)
    }

    pub fn is_enabled(&self, id: &str) -> bool {
        self.lock().on.contains(id)
    }

    /// What the person held back of the plugin's permissions (the details sheet). The default for a plugin with no
    /// choice made.
    pub fn choices_of(&self, id: &str) -> Choices {
        self.lock().choices.get(id).cloned().unwrap_or_default()
    }

    /// Whether a fill of this plugin goes in with no question ("Always allow", and `fill` held).
    pub fn fills_without_asking(&self, id: &str) -> bool {
        self.lock()
            .choices
            .get(id)
            .is_some_and(Choices::fills_without_asking)
    }

    /// The person set permission `key` of the plugin to `value` on its details sheet (`grants::choose`). Only a
    /// permission the manifest asks for can be set. Applied in memory at once: a plugin that is on gets a new token
    /// with the new grant (its views close and start again), so nothing checked before is applied after. The caller
    /// writes the file with [`Hub::persist`] on a worker.
    pub fn set_choice(&self, id: &str, key: &str, value: &str) -> Result<Change, String> {
        let mut g = self.lock();
        let Some(f) = g.found.iter().find(|f| f.manifest.id == id) else {
            return Err(format!("there is no plugin \"{id}\""));
        };
        let Some(p) = Permission::parse(key).filter(|p| f.manifest.permissions.contains(p)) else {
            return Err(format!("\"{id}\" does not ask for \"{key}\""));
        };
        let mut c = g.choices.get(id).cloned().unwrap_or_default();
        crate::grants::choose(&mut c, p, value)?;
        if c.is_default() {
            g.choices.remove(id);
        } else {
            g.choices.insert(id.to_string(), c);
        }
        let before_tools: Vec<Route> = g.table.tools().cloned().collect();
        let revoked = Self::reconcile(&mut g);
        let after_tools: Vec<Route> = g.table.tools().cloned().collect();
        drop(g);
        self.sync_limits();
        Ok(Change {
            tools_changed: before_tools != after_tools,
            revoked,
        })
    }

    /// What Settings and the window show.
    pub fn snapshot(&self) -> Snapshot {
        let g = self.lock();
        let mods = g
            .found
            .iter()
            .map(|f| ModEntry {
                id: f.manifest.id.clone(),
                protocol: f.manifest.protocol,
                name: f.manifest.name.clone(),
                version: f.manifest.version.clone(),
                description: f.manifest.description.clone(),
                permissions: f.manifest.permissions.clone(),
                panel_title: f.manifest.panel.as_ref().map(|p| p.title.clone()),
                tool_count: f.manifest.tools.len(),
                enabled: g.on.contains(&f.manifest.id),
                needs_approval: g.needs.contains(&f.manifest.id),
                author: f.manifest.author.clone(),
                homepage: f.manifest.homepage.clone(),
                background: f.manifest.is_plugin() && f.manifest.background.is_some(),
                stopped: g
                    .stopped
                    .get(&f.manifest.id)
                    .or_else(|| g.helper_stopped.get(&f.manifest.id))
                    .cloned(),
                source: g
                    .installed
                    .get(&f.manifest.id)
                    .and_then(|i| i.source.clone()),
                brand: f.manifest.brand.clone(),
            })
            .collect();
        let mut notes = Vec::new();
        for f in &g.found {
            let m = &f.manifest;
            if !m.tools.is_empty()
                && m.panel.is_none()
                && !(m.is_plugin() && (m.background.is_some() || m.helper.is_some()))
            {
                notes.push(format!(
                    "The tools of {} are left out: it has no panel, background page or helper to answer them.",
                    m.id
                ));
            }
        }
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
            safe_mode: g.safe.clone(),
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
                tab: None,
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

    /// The tab the agent made the call from (the app knows it; the hub's table does not).
    pub fn set_call_tab(&self, call_id: &str, tab: &str) {
        if let Some(p) = self.lock().pending.get_mut(call_id) {
            p.tab = Some(tab.to_string());
        }
    }

    /// Whether the agent's call from `tab` to `mod_id` is still waiting for the plugin's answer ("tool scope": while
    /// it waits, the plugin may fill a login into that tab's page).
    pub fn has_call_from(&self, mod_id: &str, tab: &str) -> bool {
        self.lock()
            .pending
            .values()
            .any(|p| p.mod_id == mod_id && p.tab.as_deref() == Some(tab))
    }

    /// The asks of the `fill` permission (`browser.fill`, `secret.ask`): how many each plugin made in the last minute
    /// and the plugins the person told "Deny for this session". Shared by the call threads and the window.
    pub fn asks(&self) -> std::sync::MutexGuard<'_, crate::fill::Asks> {
        self.asks.lock().unwrap_or_else(|e| e.into_inner())
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
        let root = dir.path().join("plugins");
        for (id, perms, tools) in mods {
            let f = root.join(id);
            std::fs::create_dir_all(&f).unwrap(); // sync-ok: test code only
            std::fs::write(f.join("mod.json"), mod_json(id, perms, tools)).unwrap(); // sync-ok: test code only
            std::fs::write(f.join("index.html"), "<p>x</p>").unwrap(); // sync-ok: test code only
        }
        let hub = Hub::new(Config {
            dir: root.clone(),
            old_mods_dir: Some(dir.path().join("mods")),
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
        for p in crate::manifest::ASKABLE_V2 {
            let w = permission_words(p);
            assert!(!w.is_empty() && !w.contains('\u{2014}'), "{p:?}");
            assert!(w.split_whitespace().count() <= 10, "{w}");
        }
        // The words of every permission are different, so a list reads as what it asks for.
        let mut all: Vec<&str> = Permission::ALL.into_iter().map(permission_words).collect();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), Permission::ALL.len());
    }

    #[test]
    fn the_full_control_permissions_say_so_in_their_words_and_warnings() {
        assert!(permission_words(Permission::Process).contains("full access"));
        assert!(permission_words(Permission::Input).contains("full control"));
        assert_eq!(permission_warnings(&[Permission::Process]), [WARN_PROCESS]);
        assert_eq!(permission_warnings(&[Permission::Input]), [WARN_INPUT]);
        assert!(is_full_control(&[Permission::View, Permission::Input]));
        assert!(is_full_control(&[Permission::Process]));
        assert!(!is_full_control(&crate::manifest::ASKABLE_V1));
        for w in [WARN_PROCESS, WARN_INPUT] {
            assert!(w.starts_with("Full control:"), "{w}");
        }
        assert!(WARN_PROCESS.contains("outside Flare"));
    }

    #[test]
    fn the_combined_warnings_come_with_both_permissions_only() {
        use Permission as P;
        assert!(permission_warnings(&crate::manifest::ASKABLE_V1).is_empty());
        assert!(permission_warnings(&[P::Sessions, P::Tabs, P::Decorate]).is_empty());
        assert_eq!(
            permission_warnings(&[P::Input, P::Sessions]),
            [WARN_INPUT, WARN_INPUT_SESSIONS]
        );
        assert_eq!(
            permission_warnings(&[P::Network, P::Sessions]),
            [WARN_NETWORK_SESSIONS, WARN_NETWORK]
        );
        assert_eq!(
            permission_warnings(&[P::Tabs, P::Network]),
            [WARN_NETWORK_TABS, WARN_NETWORK]
        );
        assert_eq!(permission_warnings(&[P::Network]), [WARN_NETWORK]);
        assert_eq!(
            permission_warnings(&crate::manifest::ASKABLE_V2),
            [
                WARN_PROCESS,
                WARN_INPUT,
                WARN_INPUT_SESSIONS,
                WARN_NETWORK_SESSIONS,
                WARN_NETWORK_TABS,
                WARN_NETWORK_FILL,
                WARN_NETWORK,
                WARN_FILL
            ]
        );
        assert_eq!(permission_warnings(&[P::Fill]), [WARN_FILL]);
        assert_eq!(permission_words(P::Fill), "Fill logins into your web panel");
        assert!(!is_full_control(&[P::Fill]));
        for w in permission_warnings(&crate::manifest::ASKABLE_V2) {
            assert!(!w.contains('\u{2014}'), "{w}");
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
                permissions: vec![Permission::Panel, Permission::Tools],
                plugin: false,
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
        let shown = Shown {
            permissions: w.hub.snapshot().mods[0]
                .permissions
                .iter()
                .map(|p| permission_words(*p).to_string())
                .collect(),
            ..Shown::default()
        };
        // The manifest asks for something else by the time the click is handled: nothing is approved.
        let mut stale = shown.clone();
        stale.permissions.pop();
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
        w.hub
            .set_enabled_shown("notes", false, Some(&Shown::default()))
            .unwrap();
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

    /// Tool scope (the `fill` permission): while the agent's call from a tab waits on the plugin, the plugin may fill
    /// into that tab; once it is answered (or cancelled), it may not.
    #[test]
    fn a_waiting_call_names_its_tab_until_it_is_answered() {
        let w = notes_world();
        w.hub.scan();
        w.hub.set_enabled("notes", true).unwrap();
        let call = w
            .hub
            .begin_call("mod_notes_append_note", json!({}))
            .unwrap();
        assert!(!w.hub.has_call_from("notes", "t1"), "no tab was named");
        w.hub.set_call_tab(&call.id, "t1");
        assert!(w.hub.has_call_from("notes", "t1"));
        assert!(!w.hub.has_call_from("notes", "t2"));
        assert!(!w.hub.has_call_from("other", "t1"));
        w.hub.respond("notes", &call.id, Ok(json!(1))).unwrap();
        assert!(!w.hub.has_call_from("notes", "t1"));
        let call = w
            .hub
            .begin_call("mod_notes_append_note", json!({}))
            .unwrap();
        w.hub.set_call_tab(&call.id, "t1");
        w.hub.cancel(&call.id);
        assert!(!w.hub.has_call_from("notes", "t1"));
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
            old_mods_dir: Some(dir.path().join("nope-either")),
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

    // ---- protocol 2 plugins in the plugins folder ----

    fn plugin_json(id: &str, author: &str, perms: &str, extra: &str) -> String {
        format!(
            r#"{{"id":"{id}","name":"Plugin {id}","version":"1.0.0","protocol":2,"description":"d","author":"{author}","homepage":"https://example.com/{id}","permissions":{perms}{extra}}}"#
        )
    }

    /// Writes a plugin folder in the world's plugins folder, with a panel page and a background page.
    fn put_plugin(w: &World, id: &str, manifest: &str) {
        let f = w.root.join(id);
        std::fs::create_dir_all(&f).unwrap(); // sync-ok: test code only
        std::fs::write(f.join("plugin.json"), manifest).unwrap(); // sync-ok: test code only
        std::fs::write(f.join("index.html"), "<p>x</p>").unwrap(); // sync-ok: test code only
        std::fs::write(f.join("bg.html"), "<p>bg</p>").unwrap(); // sync-ok: test code only
    }

    fn todo_world() -> World {
        let w = world(&[]);
        put_plugin(
            &w,
            "todo",
            &plugin_json("todo", "someone", r#"["decorate","tabs"]"#, ""),
        );
        w
    }

    #[test]
    fn a_held_back_permission_leaves_the_grant_survives_a_restart_and_an_update_and_never_widens() {
        let w = world(&[]);
        put_plugin(
            &w,
            "vault",
            &plugin_json(
                "vault",
                "someone",
                r#"["panel","tools","decorate","fill"]"#,
                r#","panel":{"entry":"index.html","title":"V"},"tools":[{"name":"go","description":"d","input_schema":{"type":"object"}}]"#,
            ),
        );
        w.hub.scan();
        w.hub.set_enabled("vault", true).unwrap();
        let gen0 = w.hub.generation("vault");
        assert_eq!(w.hub.tools().len(), 1);
        // Held back: the grant drops it, the token is new, and the tool goes with `tools`.
        let c = w.hub.set_choice("vault", "tools", "off").unwrap();
        assert_eq!(c.revoked, ["vault"]);
        assert!(c.tools_changed);
        assert!(w.hub.tools().is_empty());
        assert!(w.hub.generation("vault") > gen0);
        let Some(Grant::Mod { permissions, .. }) = w.hub.grant_of_mod("vault") else {
            panic!("on");
        };
        assert_eq!(
            permissions,
            [Permission::Panel, Permission::Decorate, Permission::Fill]
        );
        // "Always allow" changes no grant.
        let c = w.hub.set_choice("vault", "fill", "always").unwrap();
        assert!(c.revoked.is_empty());
        assert!(w.hub.fills_without_asking("vault"));
        // A permission it does not ask for, or a value it does not have, is refused.
        assert!(w.hub.set_choice("vault", "input", "off").is_err());
        assert!(w.hub.set_choice("vault", "tools", "always").is_err());
        assert!(w.hub.set_choice("nobody", "tools", "off").is_err());
        w.hub.persist("vault").unwrap();
        let text = std::fs::read_to_string(&w.hub.config().state_file).unwrap(); // sync-ok: test code only
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["mods"]["vault"]["withheld"], json!(["tools"]));
        assert_eq!(v["mods"]["vault"]["fill"], "always");
        assert_eq!(
            v["mods"]["vault"]["permissions"],
            json!(["panel", "decorate", "fill"])
        );
        // A restart keeps it on with the same choices.
        let again = Hub::new(w.hub.config().clone());
        again.scan();
        assert!(again.is_enabled("vault"));
        assert!(again.tools().is_empty());
        assert!(again.fills_without_asking("vault"));
        // An update that asks for the same keeps the choices; one that asks for more is still off until approved.
        put_plugin(
            &w,
            "vault",
            &plugin_json(
                "vault",
                "someone",
                r#"["panel","tools","decorate","fill","tabs"]"#,
                r#","panel":{"entry":"index.html","title":"V"},"tools":[{"name":"go","description":"d","input_schema":{"type":"object"}}]"#,
            ),
        );
        again.scan();
        assert!(!again.is_enabled("vault"));
        assert_eq!(again.choices_of("vault").withheld, [Permission::Tools]);
        again.set_enabled("vault", true).unwrap();
        assert!(
            again.tools().is_empty(),
            "still held back after the approval"
        );
        // Giving it back restores the tool.
        again.set_choice("vault", "tools", "on").unwrap();
        assert_eq!(again.tools().len(), 1);
        // Off at the plugin level: `fill` off ends "Always allow" with it.
        again.set_choice("vault", "fill", "off").unwrap();
        assert!(!again.fills_without_asking("vault"));
    }

    #[test]
    fn a_plugin_is_listed_with_its_protocol_author_and_homepage() {
        let w = todo_world();
        w.hub.scan();
        let s = w.hub.snapshot();
        assert!(s.errors.is_empty(), "{:?}", s.errors);
        let e = &s.mods[0];
        assert_eq!(e.protocol, 2);
        assert_eq!(e.author.as_deref(), Some("someone"));
        assert_eq!(e.homepage.as_deref(), Some("https://example.com/todo"));
        assert!(!e.enabled, "off until the user turns it on");
        // A protocol 1 mod says protocol 1 and has no author.
        let n = notes_world();
        n.hub.scan();
        let m = &n.hub.snapshot().mods[0];
        assert_eq!((m.protocol, m.author.clone()), (1, None));
    }

    #[test]
    fn turning_a_plugin_on_grants_its_plugin_permissions_and_keeps_who_made_it_with_a_hash() {
        let w = todo_world();
        w.hub.scan();
        w.hub.set_enabled("todo", true).unwrap();
        assert_eq!(
            w.hub.grant_of_mod("todo").unwrap(),
            Grant::Mod {
                id: "todo".into(),
                permissions: vec![Permission::Decorate, Permission::Tabs],
                plugin: true,
            }
        );
        w.hub.persist("todo").unwrap();
        let text = std::fs::read_to_string(&w.hub.config().state_file).unwrap(); // sync-ok: test code only
        let v: Value = serde_json::from_str(&text).unwrap();
        let e = &v["mods"]["todo"];
        assert_eq!(e["on"], true);
        assert_eq!(e["permissions"], json!(["decorate", "tabs"]));
        assert_eq!(e["author"], "someone");
        assert_eq!(e["homepage"], "https://example.com/todo");
        assert!(
            e["hash"].as_str().is_some_and(crate::hash::is_hash),
            "{text}"
        );
        assert!(e.get("source").is_none(), "no install source yet");
        // A restart reads it back on.
        let again = Hub::new(w.hub.config().clone());
        again.scan();
        assert!(again.is_enabled("todo"));
        // Off drops the identity with the approval.
        w.hub.set_enabled("todo", false).unwrap();
        w.hub.persist("todo").unwrap();
        let text = std::fs::read_to_string(&w.hub.config().state_file).unwrap(); // sync-ok: test code only
        assert!(text.contains(r#""todo":{"on":false}"#), "{text}");
    }

    #[test]
    fn a_switch_refuses_a_plugin_whose_maker_changed_since_its_card_was_drawn() {
        let w = todo_world();
        w.hub.scan();
        let e = w.hub.snapshot().mods[0].clone();
        let shown = Shown {
            permissions: e
                .permissions
                .iter()
                .map(|p| permission_words(*p).to_string())
                .collect(),
            author: e.author.unwrap_or_default(),
            homepage: e.homepage.unwrap_or_default(),
        };
        // Same permissions, another maker, picked up by a rescan before the click: the person never saw this one.
        put_plugin(
            &w,
            "todo",
            &plugin_json("todo", "someone else", r#"["decorate","tabs"]"#, ""),
        );
        w.hub.scan();
        let why = w
            .hub
            .set_enabled_shown("todo", true, Some(&shown))
            .unwrap_err();
        assert!(why.contains("read the list again"), "{why}");
        assert!(!w.hub.is_enabled("todo"));
        assert!(w.hub.grant_of_mod("todo").is_none());
        // Another homepage is refused the same way.
        put_plugin(
            &w,
            "todo",
            &plugin_json("todo", "someone", r#"["decorate","tabs"]"#, "")
                .replace("example.com/todo", "example.org/todo"),
        );
        w.hub.scan();
        assert!(w.hub.set_enabled_shown("todo", true, Some(&shown)).is_err());
        assert!(!w.hub.is_enabled("todo"));
        // Back to what the card showed: the switch approves it.
        put_plugin(
            &w,
            "todo",
            &plugin_json("todo", "someone", r#"["decorate","tabs"]"#, ""),
        );
        w.hub.scan();
        w.hub.set_enabled_shown("todo", true, Some(&shown)).unwrap();
        assert!(w.hub.is_enabled("todo"));
    }

    #[test]
    fn a_plugin_whose_author_or_homepage_changes_is_turned_off_until_approved_again() {
        let w = todo_world();
        w.hub.scan();
        w.hub.set_enabled("todo", true).unwrap();
        w.hub.persist("todo").unwrap();
        put_plugin(
            &w,
            "todo",
            &plugin_json("todo", "someone else", r#"["decorate","tabs"]"#, ""),
        );
        let c = w.hub.scan();
        assert_eq!(c.revoked, vec!["todo".to_string()]);
        assert!(!w.hub.is_enabled("todo"));
        assert!(w.hub.snapshot().mods[0].needs_approval);
        let again = Hub::new(w.hub.config().clone());
        again.scan();
        assert!(!again.is_enabled("todo"), "a restart keeps it off");
        // Switching it on approves the new maker.
        w.hub.set_enabled("todo", true).unwrap();
        w.hub.persist("todo").unwrap();
        w.hub.scan();
        assert!(w.hub.is_enabled("todo"));
        // The homepage counts too.
        put_plugin(
            &w,
            "todo",
            &plugin_json("todo", "someone else", r#"["decorate","tabs"]"#, "")
                .replace("example.com/todo", "example.org/todo"),
        );
        w.hub.scan();
        assert!(!w.hub.is_enabled("todo"));
    }

    #[test]
    fn a_plugin_that_asks_for_more_is_turned_off_like_a_mod() {
        let w = todo_world();
        w.hub.scan();
        w.hub.set_enabled("todo", true).unwrap();
        w.hub.persist("todo").unwrap();
        put_plugin(
            &w,
            "todo",
            &plugin_json("todo", "someone", r#"["decorate","tabs","sessions"]"#, ""),
        );
        w.hub.scan();
        assert!(!w.hub.is_enabled("todo"));
        assert!(w.hub.snapshot().mods[0].needs_approval);
    }

    // ---- the one-time move of the old mods folder ----

    /// Writes a protocol 1 mod in the world's old mods folder, beside its plugins folder.
    fn put_old_mod(w: &World, id: &str) {
        let f = w.root.parent().unwrap().join("mods").join(id);
        std::fs::create_dir_all(&f).unwrap(); // sync-ok: test code only
        std::fs::write(f.join("mod.json"), mod_json(id, r#"["panel"]"#, "[]")).unwrap(); // sync-ok: test code only
        std::fs::write(f.join("index.html"), "<p>x</p>").unwrap(); // sync-ok: test code only
    }

    /// The first scan moves the old mods folder in: a moved mod is found and keeps its switch (it is by id), a name
    /// the plugins folder holds stays behind and is named after the folders that were read, and later scans move
    /// nothing again.
    #[test]
    fn the_first_scan_moves_the_old_mods_in_and_a_moved_mod_keeps_its_switch() {
        let w = world(&[]);
        let old = w.root.parent().unwrap().join("mods");
        put_old_mod(&w, "notes");
        put_old_mod(&w, "clash");
        put_plugin(&w, "clash", &plugin_json("clash", "x", "[]", ""));
        std::fs::write(
            w.root.parent().unwrap().join("mods.json"),
            r#"{"version":1,"mods":{"notes":{"on":true,"permissions":["panel"]}}}"#,
        )
        .unwrap(); // sync-ok: test code only
        w.hub.scan();
        assert!(w.root.join("notes").join("mod.json").is_file(), "moved");
        assert!(!old.join("notes").exists());
        let s = w.hub.snapshot();
        let ids: Vec<&str> = s.mods.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["clash", "notes"]);
        assert_eq!(
            s.mods[0].protocol, 2,
            "the plugins folder's own clash is the one read"
        );
        assert!(w.hub.is_enabled("notes"), "the switch survived the move");
        assert!(w.hub.grant_of_mod("notes").is_some());
        assert_eq!(s.errors.len(), 1, "{:?}", s.errors);
        assert_eq!(s.errors[0].folder, "clash");
        assert!(s.errors[0].reason.contains("already has a folder"));
        assert!(
            old.join("clash").is_dir(),
            "the clash stays in the mods folder"
        );
        // A later scan moves nothing new, even a folder dropped into the old mods folder since.
        put_old_mod(&w, "late-one");
        w.hub.scan();
        assert!(old.join("late-one").is_dir() && !w.root.join("late-one").exists());
        assert_eq!(w.hub.snapshot().errors.len(), 1, "still reported, once");
        // Once the user does what it says, the next scan no longer says it.
        std::fs::remove_dir_all(old.join("clash")).unwrap(); // sync-ok: test code only
        w.hub.scan();
        assert!(w.hub.snapshot().errors.is_empty());
    }

    /// The switch of an id whose old folder stayed behind was given to that folder, not to the plugins folder's one
    /// of the same name: the first scan turns it off and saves it off, so neither folder runs without a new yes.
    #[test]
    fn a_clash_does_not_hand_the_old_mods_switch_to_the_plugins_folder() {
        let w = world(&[("notes", r#"["panel"]"#, "[]")]);
        let old = w.root.parent().unwrap().join("mods");
        put_old_mod(&w, "notes");
        let state = w.root.parent().unwrap().join("mods.json");
        std::fs::write(
            &state,
            r#"{"version":1,"mods":{"notes":{"on":true,"permissions":["panel"]}}}"#,
        )
        .unwrap(); // sync-ok: test code only
        let c = w.hub.scan();
        assert!(
            !w.hub.is_enabled("notes"),
            "the plugins folder's notes was never approved"
        );
        assert!(w.hub.grant_of_mod("notes").is_none());
        assert!(c.revoked.is_empty(), "it never had a token");
        let s = w.hub.snapshot();
        assert!(!s.mods[0].enabled);
        assert_eq!(s.errors.len(), 1, "{:?}", s.errors);
        assert_eq!(s.errors[0].folder, "notes");
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&state).unwrap()).unwrap(); // sync-ok: test code only
        assert_eq!(saved["mods"]["notes"]["on"], false, "{saved}");
        // Removing the old folder leaves the plugins folder's notes off, after a restart too, until it is turned on.
        std::fs::remove_dir_all(old.join("notes")).unwrap(); // sync-ok: test code only
        let again = Hub::new(w.hub.config().clone());
        again.scan();
        assert!(!again.is_enabled("notes"));
        assert!(again.snapshot().errors.is_empty());
        again.set_enabled("notes", true).unwrap();
        assert!(again.grant_of_mod("notes").is_some());
    }

    #[test]
    fn tools_of_a_plugin_with_a_background_page_are_answered_by_it() {
        let w = world(&[]);
        let tools = format!(r#","background":"bg.html","tools":[{}]"#, tool("add_todo"));
        put_plugin(
            &w,
            "todo",
            &plugin_json("todo", "me", r#"["tools"]"#, &tools),
        );
        w.hub.scan();
        assert!(w.hub.answers_in_background("todo"));
        w.hub.set_enabled("todo", true).unwrap();
        assert_eq!(w.hub.tools()[0].name, "mod_todo_add_todo");
        assert!(w.hub.snapshot().notes.is_empty());
        assert_eq!(w.hub.with_background(), ["todo"]);
        let (folder, entry) = w.hub.background_of("todo").unwrap();
        assert!(folder.ends_with("todo"));
        assert_eq!(entry, "bg.html");
        // With a panel as well, the background page still answers them.
        let tools = format!(
            r#","background":"bg.html","panel":{{"entry":"index.html","title":"T"}},"tools":[{}]"#,
            tool("add_todo")
        );
        put_plugin(
            &w,
            "todo",
            &plugin_json("todo", "me", r#"["tools","panel"]"#, &tools),
        );
        w.hub.scan();
        w.hub.set_enabled("todo", true).unwrap();
        assert_eq!(w.hub.tools()[0].name, "mod_todo_add_todo");
        assert!(w.hub.answers_in_background("todo"));
        // A protocol 1 mod never does.
        let n = notes_world();
        n.hub.scan();
        n.hub.set_enabled("notes", true).unwrap();
        assert!(!n.hub.answers_in_background("notes"));
        assert!(n.hub.background_of("notes").is_none());
    }

    #[test]
    fn a_plugins_bar_and_network_count_only_while_it_is_on_with_them_approved() {
        let w = world(&[]);
        put_plugin(
            &w,
            "player",
            &plugin_json(
                "player",
                "me",
                r#"["bar","network"]"#,
                r#","bar":{"entry":"bar.html","height":48}"#,
            ),
        );
        std::fs::write(w.root.join("player").join("bar.html"), "<p>bar</p>").unwrap(); // sync-ok: test code only
        put_plugin(&w, "plain", &plugin_json("plain", "me", r#"["tabs"]"#, ""));
        w.hub.scan();
        assert!(
            w.hub.snapshot().errors.is_empty(),
            "{:?}",
            w.hub.snapshot().errors
        );
        // Off: nothing.
        assert!(w.hub.bar_of("player").is_none());
        assert!(!w.hub.network_of("player"));
        w.hub.set_enabled("player", true).unwrap();
        w.hub.set_enabled("plain", true).unwrap();
        let (folder, entry, height) = w.hub.bar_of("player").unwrap();
        assert!(folder.ends_with("player"));
        assert_eq!((entry.as_str(), height), ("bar.html", 48));
        assert!(w.hub.network_of("player"));
        // A plugin without them has neither.
        assert!(w.hub.bar_of("plain").is_none());
        assert!(!w.hub.network_of("plain"));
        // A protocol 1 mod never reaches the network.
        let n = notes_world();
        n.hub.scan();
        n.hub.set_enabled("notes", true).unwrap();
        assert!(!n.hub.network_of("notes") && n.hub.bar_of("notes").is_none());
        w.hub.set_enabled("player", false).unwrap();
        assert!(w.hub.bar_of("player").is_none() && !w.hub.network_of("player"));
    }

    #[test]
    fn at_most_six_plugins_with_a_background_page_are_on_and_the_seventh_is_told_why() {
        let w = world(&[]);
        for i in 0..7 {
            let id = format!("bg{i}");
            put_plugin(
                &w,
                &id,
                &plugin_json(&id, "me", "[]", r#","background":"bg.html""#),
            );
        }
        put_plugin(&w, "plain", &plugin_json("plain", "me", "[]", ""));
        w.hub.scan();
        for i in 0..6 {
            w.hub.set_enabled(&format!("bg{i}"), true).unwrap();
        }
        let e = w.hub.set_enabled("bg6", true).unwrap_err();
        assert!(e.contains("at most 6") && e.contains("bg6"), "{e}");
        assert!(!w.hub.is_enabled("bg6"));
        // A plugin without one is not counted, and one already on may be switched on again.
        w.hub.set_enabled("plain", true).unwrap();
        w.hub.set_enabled("bg0", true).unwrap();
        w.hub.set_enabled("bg0", false).unwrap();
        w.hub.set_enabled("bg6", true).unwrap();
        assert_eq!(w.hub.with_background().len(), 6);
    }

    #[test]
    fn a_plugin_on_shows_what_it_contributes_and_a_click_scope_lasts_until_it_stops_or_the_tab_closes()
     {
        let w = world(&[]);
        let contributes = r#","background":"bg.html","contributes":{"sidebar":[{"key":"todos","title":"To-dos"}],"actions":[{"key":"add","title":"Add","icon":"plus","where":["tab-menu"]}]}"#;
        put_plugin(
            &w,
            "todo",
            &plugin_json("todo", "someone", r#"["sidebar","actions"]"#, contributes),
        );
        w.hub.scan();
        // Off: nothing on the screen, and no click gives it anything.
        assert!(w.hub.surfaces().is_empty());
        w.hub.grant_click_scope("todo", "t1");
        assert!(!w.hub.has_click_scope("todo", "t1"));
        w.hub.set_enabled("todo", true).unwrap();
        let s = w.hub.surfaces();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].name, "Plugin todo");
        assert_eq!(s[0].sidebar[0].key, "todos");
        assert_eq!(s[0].actions[0].key, "add");
        assert!(w.hub.declares_section("todo", "todos") && !w.hub.declares_section("todo", "add"));
        assert!(w.hub.declares_action("todo", "add") && !w.hub.declares_action("todo", "todos"));
        // A stopped plugin's surfaces leave the screen.
        w.hub.set_stopped("todo", Some("stopped"));
        assert!(w.hub.surfaces().is_empty());
        w.hub.set_stopped("todo", None);
        // A click scope: on that tab only, gone with the tab, and gone when the plugin's token dies.
        w.hub.grant_click_scope("todo", "t1");
        w.hub.grant_click_scope("todo", "t2");
        assert!(w.hub.has_click_scope("todo", "t1") && !w.hub.has_click_scope("todo", "t3"));
        assert!(!w.hub.has_click_scope("other", "t1"));
        w.hub.forget_tab("t1");
        assert!(!w.hub.has_click_scope("todo", "t1") && w.hub.has_click_scope("todo", "t2"));
        w.hub.set_enabled("todo", false).unwrap();
        w.hub.set_enabled("todo", true).unwrap();
        assert!(
            !w.hub.has_click_scope("todo", "t2"),
            "a switch off forgets every click"
        );
        // Kept bounded: past the cap the oldest click goes, not the smallest id. A repeat click on tab-000 makes it
        // the newest, so tab-001 is the oldest when one more tab is clicked.
        for i in 0..MAX_CLICKED_TABS {
            w.hub.grant_click_scope("todo", &format!("tab-{i:03}"));
        }
        w.hub.grant_click_scope("todo", "tab-000");
        w.hub
            .grant_click_scope("todo", &format!("tab-{MAX_CLICKED_TABS:03}"));
        assert!(w.hub.has_click_scope("todo", "tab-000"));
        assert!(!w.hub.has_click_scope("todo", "tab-001"));
        assert!(
            w.hub
                .has_click_scope("todo", &format!("tab-{MAX_CLICKED_TABS:03}"))
        );
    }

    #[test]
    fn a_plugins_generation_changes_whenever_its_token_dies_and_its_budget_follows_the_switch() {
        let w = todo_world();
        w.hub.scan();
        let g0 = w.hub.generation("todo");
        w.hub.set_enabled("todo", true).unwrap();
        assert_eq!(w.hub.generation("todo"), g0, "turning on kills nothing");
        assert_eq!(w.hub.limits().ids(), ["todo"]);
        w.hub.set_enabled("todo", false).unwrap();
        assert_eq!(w.hub.generation("todo"), g0 + 1);
        assert!(w.hub.limits().of("todo").is_none());
        w.hub.set_enabled("todo", true).unwrap();
        // A new manifest that asks for more turns it off: another generation.
        put_plugin(
            &w,
            "todo",
            &plugin_json("todo", "someone", r#"["decorate","tabs","view"]"#, ""),
        );
        w.hub.scan();
        assert_eq!(w.hub.generation("todo"), g0 + 2);
        // A protocol 1 mod has no budget of its own.
        let n = notes_world();
        n.hub.scan();
        n.hub.set_enabled("notes", true).unwrap();
        assert!(n.hub.limits().ids().is_empty());
    }

    #[test]
    fn safe_mode_reads_the_switches_applies_none_and_writes_none() {
        let w = todo_world();
        w.hub.scan();
        w.hub.set_enabled("todo", true).unwrap();
        w.hub.persist("todo").unwrap();
        let safe = Hub::new(w.hub.config().clone());
        safe.set_safe_mode("Plugins are off for this start.");
        safe.scan();
        assert!(!safe.is_enabled("todo"));
        assert!(safe.grant_of_mod("todo").is_none());
        assert_eq!(
            safe.snapshot().safe_mode.as_deref(),
            Some("Plugins are off for this start.")
        );
        // The next run has the switch back.
        let next = Hub::new(w.hub.config().clone());
        next.scan();
        assert!(next.is_enabled("todo"));
        assert_eq!(next.safe_mode(), None);
        // The person may still turn one on by hand.
        safe.set_enabled("todo", true).unwrap();
        assert!(safe.is_enabled("todo"));
    }

    #[test]
    fn a_stopped_background_page_is_named_until_the_plugin_is_switched() {
        let w = world(&[]);
        let extra = r#","background":"bg.html","contributes":{"icons":{"tick":"M4 12l5 5L20 6"}}"#;
        put_plugin(
            &w,
            "pin-up",
            &plugin_json("pin-up", "me", r#"["decorate"]"#, extra),
        );
        w.hub.scan();
        w.hub.set_enabled("pin-up", true).unwrap();
        w.hub.set_stopped("pin-up", Some("It stopped."));
        let e = &w.hub.snapshot().mods[0];
        assert!(e.background);
        assert_eq!(e.stopped.as_deref(), Some("It stopped."));
        w.hub.set_enabled("pin-up", false).unwrap();
        assert_eq!(w.hub.stopped("pin-up"), None);
        assert_eq!(w.hub.name_of("pin-up"), "Plugin pin-up");
        assert_eq!(w.hub.name_of("nobody"), "nobody");
        assert_eq!(
            w.hub.icon_of("pin-up", "tick").as_deref(),
            Some("M4 12l5 5L20 6")
        );
        assert_eq!(w.hub.icon_of("pin-up", "other"), None);
    }
}
