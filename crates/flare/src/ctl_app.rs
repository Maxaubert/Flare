//! The control channel inside Flare (Surfaces stage 2): the [`Dispatcher`] the pipe server calls, and
//! the message it posts to the UI thread.
//!
//! The server runs on its own threads (rule 34); a call arrives on one of them, is checked against
//! its grant by `flare-ctl`, and comes here. Anything that touches the window or a tab's panel is a
//! [`CtlOp`] posted to the UI thread as `UiMsg::Ctl` and answered on a channel, with a time limit, so
//! the UI thread never waits on a client and a client never waits forever on the UI thread. Anything
//! that touches the disk (resolving a path the agent named) happens here, on the call's thread, with
//! the same checks a click on a path gets (rules 13 and 14).
//!
//! A connection's tab is named by its secret (`FLARE_TAB_TOKEN`), never by an id: see `docs/surfaces/protocol.md`.
//! A plugin that holds `tabs` may name another tab in a call, by the public id `tabs.list` gives (never a secret);
//! so may a plugin on a tab where the person clicked one of its actions or sidebar items (click scope, design 13).
//! Every other caller acts on its own tab only.
//!
//! Plugins phase 5: `session.read` reads the last entries of a tab's Claude transcript here, on the call's thread (the
//! window only says where the session is filed); `tabs.input` is checked here (the `input` permission, the tab, the
//! text's size and characters, the rate) before the window types it as a paste (rule 2's plugin exception); and
//! `storage.*` reads and changes the plugin's own store here, its writes going to a worker of their own.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flare_agent::icon_motion::IconMotion;
use flare_agent::kinds::{AgentKind, RESUMABLE_AGENTS};
use flare_ctl::{CallContext, CtlError, Dispatcher, Grant, Method, code};
use flare_mods::Hub;
use flare_surfaces::PanelKind;
use flare_surfaces::agent_job::{Op as AgentOp, Target};
use flare_surfaces::agent_web::{self as agent_web, Button, WaitFor};
use flare_surfaces::app::{self, AppSpec};
use flare_surfaces::preview;
use flare_tabs::decor::{Source as DecorSource, Tone};
use serde_json::{Value, json};

use crate::appctl::{Post, UiMsg};

/// The most text `view.text` takes, and so the most a panel shows of it (the same 4 MB a file gets).
pub const TEXT_MAX: usize = preview::TEXT_CAP;
/// How long a call waits for the UI thread before it answers `timeout`. Under the channel's own 15 s,
/// so the answer is ours, with a reason, and not the server's.
pub const UI_WAIT: Duration = Duration::from_secs(10);
/// How long a browser call waits for its page to load before it answers `timeout`: under the 60 s the
/// channel allows `browser.*`, so the answer is ours, with a reason.
pub const BROWSER_WAIT: Duration = Duration::from_secs(30);
/// How long `mods.call` waits for the window to hand the call to the mod's page, and the margin it keeps under
/// the channel's `MOD_CALL_TIMEOUT` so the page's wait ends first.
const MOD_UI_WAIT: Duration = Duration::from_secs(8);
const MOD_REPLY_MARGIN: Duration = Duration::from_secs(2);
/// How long the hub may still wait for a mod's page after `elapsed` of the call is spent: the rest of the
/// channel's `MOD_CALL_TIMEOUT` minus the margin, and never more than the hub's own `CALL_TIMEOUT`. A silent page
/// therefore ends the call with Flare's answer before the channel gives up on its own.
fn mod_wait_left(elapsed: Duration) -> Duration {
    (flare_ctl::method::MOD_CALL_TIMEOUT - MOD_REPLY_MARGIN)
        .saturating_sub(elapsed)
        .min(flare_mods::hub::CALL_TIMEOUT)
}

/// How long `browser.fill` and `secret.ask` wait for the window: the person's minute at the dialog and the page's
/// fill after it, inside the channel's `FILL_TIMEOUT`, so the answer is Flare's.
pub const ASK_CALL_WAIT: Duration = Duration::from_secs(75);

/// How often a waiting call looks at the stop flag.
const STOP_SLICE: Duration = Duration::from_millis(50);
/// The longest title a caller may give a panel (the header shows one line).
pub const TITLE_MAX: usize = 200;

/// What a call asks of the UI thread. Every variant names its tab: the tab is the connection's own,
/// resolved from its token, never one taken from the call's params.
#[derive(Debug)]
pub enum CtlOp {
    /// `tabs.list`: every shell tab, or only `own` when given.
    Tabs {
        own: Option<String>,
    },
    /// `tabs.list` for a plugin that holds `tabs` (plugins phase 3): every shell tab with its folder, its agent's
    /// kind, state and Claude session, and what its panel shows.
    TabsWide,
    /// The tab's folder, so a relative path can be resolved the way the tab sees it.
    TabCwd {
        tab: String,
    },
    PanelState {
        tab: String,
    },
    PanelClose {
        tab: String,
    },
    /// A file the call thread already vetted. `abs` is the file system's own name for it.
    ShowFile {
        tab: String,
        kind: PanelKind,
        abs: String,
        title: Option<String>,
    },
    ShowText {
        tab: String,
        text: String,
        title: Option<String>,
    },
    /// `panel.open` with kind `changes` (stage 6): that native panel on the tab.
    ShowConvo {
        tab: String,
        kind: PanelKind,
    },
    /// `convo.state` (stage 6): a compact summary of the conversation the tab's tracker has read.
    ConvoState {
        tab: String,
    },
    /// `view.html` and `view.diagram` (stage 5): the page is already written into the folder `spec`
    /// names (and that folder is the UI thread's to remove from now on). Answered when the page has
    /// loaded and, for a diagram, drawn.
    ShowApp {
        tab: String,
        spec: AppSpec,
        title: String,
        diagram: bool,
    },
    /// `browser.open` and `panel.open` web: the page is already vetted (`normalize_url`). Answered when
    /// the page has loaded.
    BrowserOpen {
        tab: String,
        url: String,
    },
    /// `browser.navigate`. Answered when the page has loaded.
    BrowserNavigate {
        tab: String,
        to: NavTo,
    },
    /// `browser.look`, `click`, `type`, `press`, `scroll`, `wait` and `eval` (stage 4): the agent uses
    /// the tab's web panel. Answered by the browser's message when the work is done, or when the
    /// person takes over the page.
    BrowserAgent {
        tab: String,
        op: AgentOp,
    },
    /// `panel.open` with kind `mod:<id>` (stage 8): that mod's panel on the tab. The mod is on and has a panel
    /// (checked by the call's thread); answered once the panel is shown.
    ShowMod {
        tab: String,
        mod_id: String,
        /// The tab is the caller's own, so what its panel shows may go. On another tab (a plugin that holds `tabs`,
        /// or its background page) the panel must be closed or show this mod already, else `busy`.
        own: bool,
        /// The mod's generation ([`Hub::generation`]) when the call was checked: one switched off since is refused.
        generation: u64,
    },
    /// `tabs.badge.set` (plugins phase 1): checked on the call's thread (key, icon, tip, tone, rate), then kept
    /// by the window for the tab and drawn in its chip.
    BadgeSet {
        tab: String,
        source: DecorSource,
        key: String,
        icon: String,
        tip: String,
        tone: Tone,
        /// A plugin's generation ([`Hub::generation`]) when the call was checked (0 for the agent): a plugin
        /// switched off since, even if on again, is refused.
        generation: u64,
    },
    BadgeClear {
        tab: String,
        source: DecorSource,
        key: String,
    },
    /// `tabs.progress.set`: `value` 0 to 1, or None for an unknown amount.
    ProgressSet {
        tab: String,
        source: DecorSource,
        key: String,
        value: Option<f32>,
        label: String,
        /// As for `BadgeSet`.
        generation: u64,
    },
    ProgressClear {
        tab: String,
        source: DecorSource,
        key: String,
    },
    /// `sidebar.set` (plugins phase 4): the items of one of the plugin's sections, every one checked on the call's
    /// thread (the key is declared, the texts are clean, a tab is one the plugin may see).
    SidebarSet {
        plugin: String,
        key: String,
        items: Vec<crate::plugin_ui::Item>,
        /// As for `BadgeSet`.
        generation: u64,
    },
    /// `actions.set` (plugins phase 4): the state of one of the plugin's actions on `tab`, or on every tab.
    ActionsSet {
        plugin: String,
        key: String,
        tab: Option<String>,
        state: crate::plugin_ui::ActionState,
        /// As for `BadgeSet`.
        generation: u64,
    },
    /// `session.read` (plugins phase 5): where the tab's Claude session is filed, so the call's thread can read its
    /// transcript. Answered with `{session, folder, claude_dir}` (`session` null when none is known).
    SessionOf {
        tab: String,
    },
    /// `tabs.input` (plugins phase 5): text a plugin types into the tab's shell as one paste, checked on the call's
    /// thread (the permission, the tab, the size, the characters, the rate), and an Enter after it when asked.
    TabInput {
        plugin: String,
        tab: String,
        text: String,
        enter: bool,
        /// As for `BadgeSet`.
        generation: u64,
    },
    /// `bar.show` (plugins phase 6): the plugin's bar under the tab's terminal, checked on the call's thread (the plugin
    /// has a bar, the tab is one it may see, the rate). `busy` when another plugin's bar shows there.
    BarShow {
        plugin: String,
        tab: String,
        /// As for `BadgeSet`.
        generation: u64,
    },
    /// `bar.hide`: the plugin's bar leaves the tab (another plugin's stays).
    BarHide {
        plugin: String,
        tab: String,
    },
    /// `agents.motion.set` (owner decision 2026-10-26): checked on the call's thread (the agent, a motion of Flare's
    /// list that fits it, the rate); the window keeps the plugin's pick, and the agent's working tabs wear the newest.
    MotionSet {
        plugin: String,
        agent: AgentKind,
        motion: IconMotion,
        /// As for `BadgeSet`.
        generation: u64,
    },
    /// `agents.motion.clear`: the plugin's pick for the agent goes (an older pick of another plugin shows again).
    MotionClear {
        plugin: String,
        agent: AgentKind,
    },
    /// `browser.fill.page` (the `fill` permission): the address of the tab's web page (never a mini-app's or a
    /// plugin's page). Answered with `{tab, url, host}`.
    FillPage {
        tab: String,
    },
    /// `browser.fill`: checked on the call's thread (the plugin, the params, the tab it may fill, its asks); the window
    /// checks the page's site against the login's addresses, asks the person in its own dialog and on Allow types
    /// the values into the page. Answered when the page is filled, or at the person's no. `values` prints nothing.
    Fill {
        plugin: String,
        plugin_name: String,
        tab: String,
        login_name: String,
        login_uris: Vec<String>,
        values: flare_mods::fill::Values,
        /// The plugin may fill `tab` only through an agent's waiting call (tool scope): Allow checks it again.
        via_call: bool,
        /// As for `BadgeSet`.
        generation: u64,
    },
    /// `secret.ask`: Flare's own masked field, titled with the plugin's name. Answered with `{text}` (to this
    /// plugin only, never logged) or `{cancelled: true}`.
    SecretAsk {
        plugin: String,
        plugin_name: String,
        title: String,
        label: String,
        /// The line under the title and the button's word; empty when the plugin left them out.
        sub: String,
        button: String,
        /// As for `BadgeSet`.
        generation: u64,
    },
    /// `mods.call` (stage 8): hand a tool call to the mod's page in this tab, opening the mod's panel there
    /// first when it is not showing. Answered when the call is queued for the page; the page's own answer comes
    /// back through the hub.
    ModTool {
        tab: String,
        mod_id: String,
        call_id: String,
        tool: String,
        input: Value,
    },
}

/// Where `browser.navigate` sends the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavTo {
    Url(String),
    Back,
    Forward,
    Reload,
}

/// A request for the UI thread with the channel its answer goes back on.
pub struct CtlRequest {
    pub op: CtlOp,
    /// When the caller stops waiting. The UI thread does nothing for a request that arrives after it:
    /// the caller was told `timeout`, and the window must not change after that.
    pub deadline: Instant,
    pub reply: mpsc::Sender<Result<Value, CtlError>>,
}

/// A [`CtlRequest`] that can sit in a `UiMsg` (which is `Debug`): it prints its kind only, never a
/// path or the text.
pub struct CtlRequestBox(pub CtlRequest);

impl std::fmt::Debug for CtlRequestBox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self.0.op {
            CtlOp::Tabs { .. } => "CtlOp::Tabs",
            CtlOp::TabsWide => "CtlOp::TabsWide",
            CtlOp::TabCwd { .. } => "CtlOp::TabCwd",
            CtlOp::PanelState { .. } => "CtlOp::PanelState",
            CtlOp::PanelClose { .. } => "CtlOp::PanelClose",
            CtlOp::ShowFile { .. } => "CtlOp::ShowFile",
            CtlOp::ShowText { .. } => "CtlOp::ShowText",
            CtlOp::ShowConvo { .. } => "CtlOp::ShowConvo",
            CtlOp::ConvoState { .. } => "CtlOp::ConvoState",
            CtlOp::ShowApp { .. } => "CtlOp::ShowApp",
            CtlOp::BrowserOpen { .. } => "CtlOp::BrowserOpen",
            CtlOp::BrowserNavigate { .. } => "CtlOp::BrowserNavigate",
            CtlOp::BrowserAgent { .. } => "CtlOp::BrowserAgent",
            CtlOp::ShowMod { .. } => "CtlOp::ShowMod",
            CtlOp::ModTool { .. } => "CtlOp::ModTool",
            CtlOp::BadgeSet { .. } => "CtlOp::BadgeSet",
            CtlOp::BadgeClear { .. } => "CtlOp::BadgeClear",
            CtlOp::ProgressSet { .. } => "CtlOp::ProgressSet",
            CtlOp::ProgressClear { .. } => "CtlOp::ProgressClear",
            CtlOp::SidebarSet { .. } => "CtlOp::SidebarSet",
            CtlOp::ActionsSet { .. } => "CtlOp::ActionsSet",
            CtlOp::SessionOf { .. } => "CtlOp::SessionOf",
            CtlOp::TabInput { .. } => "CtlOp::TabInput",
            CtlOp::BarShow { .. } => "CtlOp::BarShow",
            CtlOp::BarHide { .. } => "CtlOp::BarHide",
            CtlOp::MotionSet { .. } => "CtlOp::MotionSet",
            CtlOp::MotionClear { .. } => "CtlOp::MotionClear",
            CtlOp::FillPage { .. } => "CtlOp::FillPage",
            CtlOp::Fill { .. } => "CtlOp::Fill",
            CtlOp::SecretAsk { .. } => "CtlOp::SecretAsk",
        })
    }
}

/// Where a path of the agent's is resolved again: the checks of a click on a path. A seam so a test
/// can answer without a disk.
pub type Resolve = Arc<dyn Fn(&str, &str, &str) -> Option<(String, String)> + Send + Sync>;

/// The production resolver: `flare_term::path_open::resolve_openable_file` (rule 13).
pub fn real_resolve() -> Resolve {
    Arc::new(flare_term::path_open::resolve_openable_file)
}

/// Which tab owns a secret (the session manager's answer, from any thread).
pub type TabOf = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Is "Let mini apps use the network" on? Asked from any thread, at every request.
pub type NetSwitch = Arc<dyn Fn() -> bool + Send + Sync>;

/// What `view.html` and `view.diagram` need: where the apps' folders go, how a page named by path is
/// resolved again, the user's network switch and the Mermaid build a diagram is drawn with.
pub struct AppsCfg {
    pub root: std::path::PathBuf,
    pub resolve_html: Resolve,
    pub network: NetSwitch,
    pub mermaid: &'static [u8],
    /// False only for the e2e's seam `APP_NO_POLICY`: a page is written without its policy so the second
    /// wall (the browser glue's refusal of outside requests) can be tested alone. Always true otherwise.
    pub policy: bool,
    seq: AtomicU64,
}

impl AppsCfg {
    pub fn new(
        root: std::path::PathBuf,
        resolve_html: Resolve,
        network: NetSwitch,
        mermaid: &'static [u8],
    ) -> Self {
        Self {
            root,
            resolve_html,
            network,
            mermaid,
            policy: true,
            seq: AtomicU64::new(0),
        }
    }

    /// The e2e's seam: pages are written without their policy.
    pub fn without_policy(mut self) -> Self {
        self.policy = false;
        self
    }
}

/// The Mermaid build every diagram is drawn with (assets/mermaid/README.md).
pub static MERMAID: &[u8] = include_bytes!("../../../assets/mermaid/mermaid.min.js");

pub struct AppDispatcher {
    post: Post,
    /// The tab that owns a token (the session manager's, off the UI thread).
    tab_of: TabOf,
    resolve: Resolve,
    home: String,
    /// Set first thing at quit: every waiting call answers `unavailable` within one slice.
    stopping: Arc<AtomicBool>,
    /// The user's "Let agents run scripts in pages" switch, read at every request (off by default).
    eval: EvalSwitch,
    /// Mini-apps (stage 5); without it `view.html` and `view.diagram` answer `unavailable`.
    apps: Option<AppsCfg>,
    /// Mods (stage 8): tokens, tools and the calls waiting on a mod's page. Without it no token resolves and
    /// the mod methods answer `unavailable`.
    mods: Option<Arc<Hub>>,
    /// The decor limit (plugins phase 1): one bucket per tab and caller, spent here before the window hears of a
    /// change, so a flood of badges never reaches the UI thread.
    decor_rate: Mutex<HashMap<(String, DecorSource), flare_ctl::session::Bucket>>,
    /// A plugin's decor limit across all its tabs (design 13): one bucket per plugin.
    decor_plugin_rate: Mutex<HashMap<String, flare_ctl::session::Bucket>>,
    /// `sidebar.set` per section and `actions.set` per plugin (design 13), spent here like the decor limit.
    surface_rate: Mutex<HashMap<String, flare_ctl::session::Bucket>>,
    /// The decor limit's clock (a test passes a still one).
    clock: fn() -> Instant,
}

/// Buckets kept at most; past it the map starts again (a bucket is only a second of memory).
const DECOR_BUCKETS_MAX: usize = 1024;
/// Decor changes one plugin may make in a second, all its tabs together (design 13).
pub const DECOR_PER_PLUGIN: f64 = 30.0;

/// Is `browser.eval` switched on for the agent? Asked from any thread, at every request.
pub type EvalSwitch = Arc<dyn Fn() -> bool + Send + Sync>;

impl AppDispatcher {
    pub fn new(
        post: Post,
        tab_of: TabOf,
        resolve: Resolve,
        home: String,
        stopping: Arc<AtomicBool>,
    ) -> Self {
        Self {
            post,
            tab_of,
            resolve,
            home,
            stopping,
            eval: Arc::new(|| false),
            apps: None,
            mods: None,
            decor_rate: Mutex::new(HashMap::new()),
            decor_plugin_rate: Mutex::new(HashMap::new()),
            surface_rate: Mutex::new(HashMap::new()),
            clock: Instant::now,
        }
    }

    /// The tab a call acts on (plugins phase 3): the `tab` it names, or else the caller's own. A caller may name its
    /// own tab; another one only with a grant that may act on any tab (a plugin that holds `tabs`). A background
    /// page has no tab of its own, so without `tabs` it can name none, and with it, it must name one. The id is
    /// the public one `tabs.list` gives; a tab that is not open answers `no_tab` on the UI thread. A tab where the
    /// person clicked one of the plugin's actions or sidebar items counts as one it may name (click scope).
    fn scoped_tab(&self, ctx: &CallContext, params: &Value) -> Result<String, CtlError> {
        match str_param(params, "tab")? {
            Some(t) if ctx.tab.as_deref() == Some(t) => Ok(t.to_string()),
            Some(t) if ctx.grant.may_act_on_any_tab() || self.clicked_on(ctx, t) => {
                if t.is_empty() || t.len() > 64 {
                    return Err(CtlError::new(code::BAD_PARAMS, "tab is a tab's id"));
                }
                Ok(t.to_string())
            }
            Some(_) => Err(CtlError::new(
                code::FORBIDDEN,
                "naming another tab needs the tabs permission",
            )),
            None => Self::tab_of_call(ctx),
        }
    }

    /// Whether the person's click on one of the calling plugin's actions or sidebar items gave it scope on `tab`.
    fn clicked_on(&self, ctx: &CallContext, tab: &str) -> bool {
        match (&ctx.grant, &self.mods) {
            (Grant::Mod { id, .. }, Some(hub)) => hub.has_click_scope(id, tab),
            _ => false,
        }
    }

    /// Whether the calling plugin may see `tab`: its own, any with `tabs`, or one a click gave it.
    fn may_see_tab(&self, ctx: &CallContext, tab: &str) -> bool {
        ctx.tab.as_deref() == Some(tab)
            || ctx.grant.may_act_on_any_tab()
            || self.clicked_on(ctx, tab)
    }

    /// The tab a decor call is about, and who is calling: the caller's own tab, or for a plugin that holds `tabs`
    /// any tab it names ([`AppDispatcher::scoped_tab`]).
    fn decor_target(
        &self,
        ctx: &CallContext,
        params: &Value,
    ) -> Result<(String, DecorSource), CtlError> {
        let tab = self.scoped_tab(ctx, params)?;
        let source = match &ctx.grant {
            Grant::Agent { .. } => DecorSource::Agent,
            Grant::Mod { id, .. } => DecorSource::Plugin(id.clone()),
            Grant::Ui => {
                return Err(CtlError::new(
                    code::FORBIDDEN,
                    "Flare's own interface draws its own marks",
                ));
            }
        };
        Ok((tab, source))
    }

    /// One decor change of `source` on `tab` spent from its bucket ([`flare_ctl::decor::RATE_PER_TAB`] a
    /// second), or `rate_limited`.
    fn spend_decor(&self, tab: &str, source: &DecorSource) -> Result<(), CtlError> {
        let now = (self.clock)();
        let mut map = self.decor_rate.lock().unwrap_or_else(|e| e.into_inner());
        if map.len() >= DECOR_BUCKETS_MAX && !map.contains_key(&(tab.to_string(), source.clone())) {
            map.clear();
        }
        let rate = flare_ctl::decor::RATE_PER_TAB;
        let ok = map
            .entry((tab.to_string(), source.clone()))
            .or_insert_with(|| flare_ctl::session::Bucket::new(rate, rate, now))
            .take(now);
        drop(map);
        if !ok {
            return Err(CtlError::new(
                code::RATE_LIMITED,
                format!("at most {rate} badge or progress changes a second on one tab"),
            ));
        }
        // A plugin also has a second for all its tabs together, so naming many tabs is no way round the first.
        if let DecorSource::Plugin(id) = source {
            let mut all = self
                .decor_plugin_rate
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if all.len() >= DECOR_BUCKETS_MAX && !all.contains_key(id) {
                all.clear();
            }
            let ok = all
                .entry(id.clone())
                .or_insert_with(|| {
                    flare_ctl::session::Bucket::new(DECOR_PER_PLUGIN, DECOR_PER_PLUGIN, now)
                })
                .take(now);
            if !ok {
                return Err(CtlError::new(
                    code::RATE_LIMITED,
                    format!(
                        "at most {DECOR_PER_PLUGIN} badge or progress changes a second for one plugin"
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Whether `icon` may go on a badge of this caller: a built-in one, or for a plugin one of its own
    /// `contributes.icons`.
    fn badge_icon_ok(&self, ctx: &CallContext, icon: &str) -> bool {
        if flare_ctl::decor::is_badge_icon(icon) {
            return true;
        }
        match (&ctx.grant, &self.mods) {
            (Grant::Mod { id, .. }, Some(hub)) => hub.icon_of(id, icon).is_some(),
            _ => false,
        }
    }

    /// `tabs.badge.*` and `tabs.progress.*`: every param is checked here, on the call's thread, and the change
    /// is spent from the tab's limit before the window is asked.
    fn decor_call(
        &self,
        ctx: &CallContext,
        method: Method,
        params: &Value,
    ) -> Result<Value, CtlError> {
        let bad = |m: String| CtlError::new(code::BAD_PARAMS, m);
        // Read before anything is checked, so a switch off during the checks is seen by the window.
        let generation = self.generation_of(ctx);
        let (tab, source) = self.decor_target(ctx, params)?;
        let spend = (tab.clone(), source.clone());
        let key = str_param(params, "key")?
            .ok_or_else(|| bad("key is required".into()))?
            .to_string();
        if !flare_ctl::decor::valid_key(&key) {
            return Err(bad(format!(
                "key is 1 to {} of a-z, 0-9 and -",
                flare_ctl::decor::KEY_MAX
            )));
        }
        // A tip or a label: absent or empty is none; else one clean line (the manifest's rule for text a person reads).
        let line = |name: &str| -> Result<String, CtlError> {
            match str_param(params, name)? {
                None | Some("") => Ok(String::new()),
                Some(t) => {
                    flare_mods::manifest::clean_line(t.to_string(), name, flare_ctl::decor::TIP_MAX)
                        .map_err(bad)
                }
            }
        };
        let op = match method {
            Method::TabsBadgeSet => {
                let icon =
                    str_param(params, "icon")?.ok_or_else(|| bad("icon is required".into()))?;
                if !self.badge_icon_ok(ctx, icon) {
                    let own = if matches!(ctx.grant, Grant::Mod { .. }) {
                        " or one of the plugin's contributes.icons"
                    } else {
                        ""
                    };
                    return Err(bad(format!(
                        "icon is one of {}{own}",
                        flare_ctl::decor::BADGE_ICONS.join(", ")
                    )));
                }
                let tone = match str_param(params, "tone")? {
                    None => Tone::Ink,
                    Some(t) => {
                        Tone::parse(t).ok_or_else(|| bad("tone is ink, accent or muted".into()))?
                    }
                };
                if tone == Tone::Accent && flare_ctl::decor::accent_refused(icon) {
                    return Err(bad(
                        "an accent mic is Flare's dictation mark: use ink or muted".into(),
                    ));
                }
                CtlOp::BadgeSet {
                    tab,
                    source,
                    key,
                    icon: icon.to_string(),
                    tip: line("tip")?,
                    tone,
                    generation,
                }
            }
            Method::TabsBadgeClear => CtlOp::BadgeClear { tab, source, key },
            Method::TabsProgressSet => {
                let value = match num_param(params, "value")? {
                    None => None,
                    Some(v) if (0.0..=1.0).contains(&v) => Some(v as f32),
                    Some(_) => {
                        return Err(bad(
                            "value is 0 to 1 (leave it out for an unknown amount)".into()
                        ));
                    }
                };
                CtlOp::ProgressSet {
                    tab,
                    source,
                    key,
                    value,
                    label: line("label")?,
                    generation,
                }
            }
            Method::TabsProgressClear => CtlOp::ProgressClear { tab, source, key },
            _ => return Err(CtlError::new(code::INTERNAL, "not a decor method")),
        };
        // Only a call that is right spends from the limit.
        self.spend_decor(&spend.0, &spend.1)?;
        self.ask(op)
    }

    /// One call spent from the bucket `name` (`rate` a second, as many at once), or `rate_limited` with `what`.
    fn spend_surface(&self, name: String, rate: f64, what: &str) -> Result<(), CtlError> {
        let now = (self.clock)();
        let mut map = self.surface_rate.lock().unwrap_or_else(|e| e.into_inner());
        if map.len() >= DECOR_BUCKETS_MAX && !map.contains_key(&name) {
            map.clear();
        }
        let ok = map
            .entry(name)
            .or_insert_with(|| flare_ctl::session::Bucket::new(rate, rate, now))
            .take(now);
        if ok {
            Ok(())
        } else {
            Err(CtlError::new(
                code::RATE_LIMITED,
                format!("at most {rate} {what} a second"),
            ))
        }
    }

    /// The calling plugin's id and the hub, for `sidebar.set` and `actions.set`: only a plugin calls them.
    fn surface_caller(&self, ctx: &CallContext) -> Result<(String, &Arc<Hub>), CtlError> {
        match &ctx.grant {
            Grant::Mod { id, .. } => Ok((id.clone(), self.hub()?)),
            _ => Err(CtlError::new(
                code::FORBIDDEN,
                "sidebar sections and actions are a plugin's",
            )),
        }
    }

    /// `sidebar.set {key, items}`: every item is checked here, on the call's thread, and the call is spent from the
    /// section's limit before the window is asked.
    fn sidebar_set(&self, ctx: &CallContext, params: &Value) -> Result<Value, CtlError> {
        use crate::plugin_ui::{ITEM_ID_MAX, ITEM_TEXT_MAX, Item, MAX_ITEMS, SIDEBAR_RATE_PER_KEY};
        let bad = |m: String| CtlError::new(code::BAD_PARAMS, m);
        let generation = self.generation_of(ctx);
        let (plugin, hub) = self.surface_caller(ctx)?;
        let key = str_param(params, "key")?
            .ok_or_else(|| bad("key is required".into()))?
            .to_string();
        if !hub.declares_section(&plugin, &key) {
            return Err(bad(format!(
                "\"{key}\" is not one of the plugin's contributes.sidebar sections"
            )));
        }
        if let Some(k) = params.as_object().and_then(|o| {
            o.keys()
                .find(|k| !matches!(k.as_str(), "key" | "items"))
                .cloned()
        }) {
            return Err(bad(format!("{k} is not a field of sidebar.set")));
        }
        let list = match params.get("items") {
            Some(Value::Array(a)) => a,
            _ => return Err(bad("items is a list of items".into())),
        };
        if list.len() > MAX_ITEMS {
            return Err(bad(format!("a section holds at most {MAX_ITEMS} items")));
        }
        let mut items: Vec<Item> = Vec::with_capacity(list.len());
        for (i, v) in list.iter().enumerate() {
            let at = format!("items[{i}]");
            let o = v
                .as_object()
                .ok_or_else(|| bad(format!("{at} must be an object")))?;
            if let Some(k) = o.keys().find(|k| {
                !matches!(
                    k.as_str(),
                    "id" | "text" | "sub" | "icon" | "checked" | "tab"
                )
            }) {
                return Err(bad(format!("{at}.{k} is not a field of an item")));
            }
            let text_of = |name: &str| -> Result<Option<String>, CtlError> {
                match o.get(name) {
                    None | Some(Value::Null) => Ok(None),
                    Some(Value::String(s)) => Ok(Some(s.clone())),
                    Some(_) => Err(bad(format!("{at}.{name} must be text"))),
                }
            };
            let id = text_of("id")?.ok_or_else(|| bad(format!("{at}.id is required")))?;
            let id = flare_mods::manifest::clean_line(id, &format!("{at}.id"), ITEM_ID_MAX)
                .map_err(bad)?;
            if items.iter().any(|it| it.id == id) {
                return Err(bad(format!("{at}.id \"{id}\" is listed twice")));
            }
            let text = text_of("text")?.ok_or_else(|| bad(format!("{at}.text is required")))?;
            let text = flare_mods::manifest::clean_line(text, &format!("{at}.text"), ITEM_TEXT_MAX)
                .map_err(bad)?;
            let sub = match text_of("sub")? {
                None => String::new(),
                Some(s) if s.is_empty() => s,
                Some(s) => flare_mods::manifest::clean_line(s, &format!("{at}.sub"), ITEM_TEXT_MAX)
                    .map_err(bad)?,
            };
            let icon = text_of("icon")?.unwrap_or_default();
            if !icon.is_empty() && !self.badge_icon_ok(ctx, &icon) {
                return Err(bad(format!(
                    "{at}.icon is one of {} or one of the plugin's contributes.icons",
                    flare_ctl::decor::BADGE_ICONS.join(", ")
                )));
            }
            let checked = match o.get("checked") {
                None | Some(Value::Null) => None,
                Some(Value::Bool(b)) => Some(*b),
                Some(_) => return Err(bad(format!("{at}.checked must be true or false"))),
            };
            let tab = match text_of("tab")? {
                None => None,
                Some(t) if t.is_empty() || t.len() > 64 => {
                    return Err(bad(format!("{at}.tab is a tab's id")));
                }
                // An item may lead only to a tab the plugin may see.
                Some(t) if !self.may_see_tab(ctx, &t) => {
                    return Err(CtlError::new(
                        code::FORBIDDEN,
                        format!("{at}.tab: naming another tab needs the tabs permission"),
                    ));
                }
                Some(t) => Some(t),
            };
            items.push(Item {
                id,
                text,
                sub,
                icon,
                checked,
                tab,
            });
        }
        self.spend_surface(
            format!("sidebar:{plugin}/{key}"),
            SIDEBAR_RATE_PER_KEY,
            "sidebar.set calls for one section",
        )?;
        self.ask(CtlOp::SidebarSet {
            plugin,
            key,
            items,
            generation,
        })
    }

    /// `actions.set {key, tab?, enabled?, checked?, title?}`: on the named tab (one the plugin may act on), on the
    /// caller's own when it names none, and on every tab when a background page names none.
    fn actions_set(&self, ctx: &CallContext, params: &Value) -> Result<Value, CtlError> {
        use crate::plugin_ui::{ACTION_TITLE_MAX, ACTIONS_RATE, ActionState};
        let bad = |m: String| CtlError::new(code::BAD_PARAMS, m);
        let generation = self.generation_of(ctx);
        let (plugin, hub) = self.surface_caller(ctx)?;
        let key = str_param(params, "key")?
            .ok_or_else(|| bad("key is required".into()))?
            .to_string();
        if !hub.declares_action(&plugin, &key) {
            return Err(bad(format!(
                "\"{key}\" is not one of the plugin's contributes.actions"
            )));
        }
        if let Some(k) = params.as_object().and_then(|o| {
            o.keys()
                .find(|k| !matches!(k.as_str(), "key" | "tab" | "enabled" | "checked" | "title"))
                .cloned()
        }) {
            return Err(bad(format!("{k} is not a field of actions.set")));
        }
        let tab = match (str_param(params, "tab")?, &ctx.tab) {
            (Some(_), _) => Some(self.scoped_tab(ctx, params)?),
            (None, Some(own)) => Some(own.clone()),
            (None, None) => None,
        };
        let title = match str_param(params, "title")? {
            None => None,
            Some(t) => Some(
                flare_mods::manifest::clean_line(t.to_string(), "title", ACTION_TITLE_MAX)
                    .map_err(bad)?,
            ),
        };
        let state = ActionState {
            enabled: bool_param(params, "enabled")?,
            checked: bool_param(params, "checked")?,
            title,
        };
        self.spend_surface(
            format!("actions:{plugin}"),
            ACTIONS_RATE,
            "actions.set calls for one plugin",
        )?;
        self.ask(CtlOp::ActionsSet {
            plugin,
            key,
            tab,
            state,
            generation,
        })
    }

    /// The calling plugin's id: the phase 5 methods are a plugin's (the grant already keeps the agent out).
    fn plugin_caller(ctx: &CallContext, what: &str) -> Result<String, CtlError> {
        match &ctx.grant {
            Grant::Mod { id, .. } => Ok(id.clone()),
            _ => Err(CtlError::new(
                code::FORBIDDEN,
                format!("{what} is a plugin's"),
            )),
        }
    }

    /// `session.read {tab?}`: the last entries of the Claude session in the tab (the caller's own, or one it may name),
    /// read from the transcript's end here on the call's thread. A tab with no known session has none.
    fn session_read(&self, ctx: &CallContext, params: &Value) -> Result<Value, CtlError> {
        use crate::plugin_sessions::{READ_RATE, entry_json};
        let plugin = Self::plugin_caller(ctx, "reading a session")?;
        only_fields(params, &["tab"], "session.read")?;
        let tab = self.scoped_tab(ctx, params)?;
        self.spend_surface(
            format!("session.read:{plugin}/{tab}"),
            READ_RATE,
            "session.read calls for one tab",
        )?;
        let at = self.ask(CtlOp::SessionOf { tab: tab.clone() })?;
        let none = || Ok(json!({"tab": tab, "session": null, "entries": []}));
        let (Some(session), Some(folder), Some(dir)) = (
            at["session"].as_str(),
            at["folder"].as_str(),
            at["claude_dir"].as_str(),
        ) else {
            return none();
        };
        let dir = std::path::Path::new(dir);
        let Some(mut path) = flare_convo::transcript_path_in(dir, folder, session) else {
            return none();
        };
        // The tab's folder may have moved since the session began: the same session in another project folder. These
        // look at the disk on the control channel's worker, never the UI thread.
        if !path.is_file()
            && let Some(other) = flare_convo::find_transcript_in(dir, session)
        {
            path = other;
        }
        let entries =
            flare_convo::said::read_last(&path, flare_convo::said::READ_LAST).map_err(|e| {
                CtlError::new(
                    code::UNAVAILABLE,
                    format!("the session's transcript could not be read: {e}"),
                )
            })?;
        let entries: Vec<Value> = entries.iter().map(entry_json).collect();
        Ok(json!({"tab": tab, "session": session, "entries": entries}))
    }

    /// `tabs.input {tab?, text, enter?}`: a plugin that holds `input` types `text` into a tab it may see, as one paste
    /// (bracketed when the program asked), and presses Enter after it when `enter` is true. Refused here: a text that
    /// is empty, over 4 KB or holds a control character (only a tab and line breaks), and more than 2 a second on a
    /// tab.
    fn tabs_input(&self, ctx: &CallContext, params: &Value) -> Result<Value, CtlError> {
        use crate::plugin_sessions::{INPUT_RATE, input_refusal};
        let bad = |m: String| CtlError::new(code::BAD_PARAMS, m);
        let generation = self.generation_of(ctx);
        let plugin = Self::plugin_caller(ctx, "typing into a terminal")?;
        only_fields(params, &["tab", "text", "enter"], "tabs.input")?;
        let tab = self.scoped_tab(ctx, params)?;
        let text = match params.get("text") {
            Some(Value::String(s)) => s.clone(),
            _ => return Err(bad("text is required".into())),
        };
        if let Some(why) = input_refusal(&text) {
            return Err(bad(why));
        }
        let enter = bool_param(params, "enter")?.unwrap_or(false);
        self.spend_surface(
            format!("tabs.input:{tab}"),
            INPUT_RATE,
            "tabs.input calls on one tab",
        )?;
        self.ask(CtlOp::TabInput {
            plugin,
            tab,
            text,
            enter,
            generation,
        })
    }

    /// `bar.show {tab?}` and `bar.hide {tab?}` (plugins phase 6): the calling plugin's bar on the tab it names (one it may
    /// see) or its own; a background page names one. The plugin must have a bar it may show (its manifest's, with `bar`
    /// approved), and each is spent from its own second on that tab.
    fn bar_call(
        &self,
        ctx: &CallContext,
        method: Method,
        params: &Value,
    ) -> Result<Value, CtlError> {
        use crate::plugin_bar::BAR_RATE;
        let generation = self.generation_of(ctx);
        let plugin = Self::plugin_caller(ctx, "a bar")?;
        only_fields(params, &["tab"], method.name())?;
        if self.hub()?.bar_of(&plugin).is_none() {
            return Err(CtlError::new(
                code::BAD_PARAMS,
                "the plugin has no bar (plugin.json \"bar\")",
            ));
        }
        let tab = self.scoped_tab(ctx, params)?;
        self.spend_surface(
            format!("{}:{plugin}/{tab}", method.name()),
            BAR_RATE,
            &format!("{} calls on one tab", method.name()),
        )?;
        self.ask(if method == Method::BarShow {
            CtlOp::BarShow {
                plugin,
                tab,
                generation,
            }
        } else {
            CtlOp::BarHide { plugin, tab }
        })
    }

    /// `agents.motion.set {agent, motion}` and `agents.motion.clear {agent}` (owner decision 2026-10-26): how an agent's
    /// tab icon moves while it works, picked from Flare's fixed list (`IconMotion`). A plugin's only (`agent-looks`);
    /// every param is checked here, and the call is spent from the plugin's second for that agent before the window is
    /// asked. The plugin never sends artwork: Flare draws the motion, and the agent's marks stay Flare's.
    fn motion_call(
        &self,
        ctx: &CallContext,
        method: Method,
        params: &Value,
    ) -> Result<Value, CtlError> {
        let bad = |m: String| CtlError::new(code::BAD_PARAMS, m);
        let generation = self.generation_of(ctx);
        let plugin = Self::plugin_caller(ctx, "an agent's motion")?;
        let set = method == Method::AgentsMotionSet;
        let fields: &[&str] = if set {
            &["agent", "motion"]
        } else {
            &["agent"]
        };
        only_fields(params, fields, method.name())?;
        let agents = RESUMABLE_AGENTS.map(AgentKind::as_str).join(", ");
        let agent = str_param(params, "agent")?
            .ok_or_else(|| bad(format!("agent is required: one of {agents}")))?;
        let agent =
            AgentKind::parse(agent).ok_or_else(|| bad(format!("agent is one of {agents}")))?;
        let op = if set {
            let fits = || IconMotion::names_for(agent).join(", ");
            let motion = str_param(params, "motion")?
                .ok_or_else(|| bad(format!("motion is required: one of {}", fits())))?;
            let motion = IconMotion::parse(motion)
                .filter(|m| m.fits(agent))
                .ok_or_else(|| {
                    bad(format!(
                        "motion for {} is one of {}",
                        agent.as_str(),
                        fits()
                    ))
                })?;
            CtlOp::MotionSet {
                plugin: plugin.clone(),
                agent,
                motion,
                generation,
            }
        } else {
            CtlOp::MotionClear {
                plugin: plugin.clone(),
                agent,
            }
        };
        // Only a call that is right spends from the limit: as many changes a second as decor on one tab.
        self.spend_surface(
            format!("agents.motion:{plugin}/{}", agent.as_str()),
            flare_ctl::decor::RATE_PER_TAB,
            "motion changes for one agent",
        )?;
        self.ask(op)
    }

    /// `storage.get {key?}` and `storage.set {key?, value}`: the calling plugin's own store, read and changed here on
    /// the call's thread (its writes go to a worker of their own). A protocol 1 mod has none.
    fn storage_call(
        &self,
        ctx: &CallContext,
        method: Method,
        params: &Value,
    ) -> Result<Value, CtlError> {
        let plugin = Self::plugin_caller(ctx, "storage")?;
        let hub = self.hub()?;
        if !hub.is_plugin(&plugin) {
            return Err(CtlError::new(
                code::FORBIDDEN,
                "storage is for plugins (plugin.json, protocol 2)",
            ));
        }
        let key = str_param(params, "key")?;
        match method {
            Method::StorageGet => {
                only_fields(params, &["key"], "storage.get")?;
                Ok(json!({"value": hub.storage().get(&plugin, key)?}))
            }
            _ => {
                only_fields(params, &["key", "value"], "storage.set")?;
                let value = params.get("value").cloned().ok_or_else(|| {
                    CtlError::new(
                        code::BAD_PARAMS,
                        "value is required (with a key, null removes it)",
                    )
                })?;
                hub.storage().set(&plugin, key, value)?;
                Ok(json!({}))
            }
        }
    }

    /// The mods hub (stage 8).
    pub fn with_mods(mut self, hub: Arc<Hub>) -> Self {
        self.mods = Some(hub);
        self
    }

    /// The calling plugin's generation ([`Hub::generation`]), 0 for any other caller.
    fn generation_of(&self, ctx: &CallContext) -> u64 {
        match (&ctx.grant, &self.mods) {
            (Grant::Mod { id, .. }, Some(hub)) => hub.generation(id),
            _ => 0,
        }
    }

    fn hub(&self) -> Result<&Arc<Hub>, CtlError> {
        self.mods.as_ref().ok_or_else(|| {
            CtlError::new(
                code::UNAVAILABLE,
                "mods are not available in this run of Flare",
            )
        })
    }

    /// `mods.tools`: the tools the enabled mods offer the agent, by the name the agent calls them.
    fn mods_tools(&self) -> Result<Value, CtlError> {
        let tools: Vec<Value> = self
            .hub()?
            .tools()
            .into_iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.input_schema,
                })
            })
            .collect();
        Ok(json!({"tools": tools}))
    }

    /// `mods.call`: routes the agent's call to its mod, has the window put it in front of the mod's page and waits
    /// for the page's answer (at most [`flare_mods::hub::CALL_TIMEOUT`]).
    fn mods_call(&self, ctx: &CallContext, params: &Value) -> Result<Value, CtlError> {
        let tab = Self::tab_of_call(ctx)?;
        let hub = self.hub()?;
        let name = str_param(params, "name")?
            .ok_or_else(|| CtlError::new(code::BAD_PARAMS, "name is required"))?;
        let input = match params.get("input") {
            None | Some(Value::Null) => json!({}),
            Some(v @ Value::Object(_)) => v.clone(),
            Some(_) => return Err(CtlError::new(code::BAD_PARAMS, "input must be an object")),
        };
        // One deadline for the whole call, inside the channel's own (`MOD_CALL_TIMEOUT`): the answer is Flare's.
        let started = Instant::now();
        let call = hub.begin_call(name, input)?;
        // While the call waits, the plugin may fill a login into this tab's page (tool scope).
        hub.set_call_tab(&call.id, &tab);
        let op = CtlOp::ModTool {
            tab,
            mod_id: call.mod_id.clone(),
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            input: call.input.clone(),
        };
        if let Err(e) = self.ask_by(op, started + MOD_UI_WAIT) {
            hub.cancel(&call.id);
            return Err(e);
        }
        let left = mod_wait_left(started.elapsed());
        let stopping = Arc::clone(&self.stopping);
        // sync-ok: a worker thread of the control channel waits for the mod's page, never the UI thread
        let result = hub.wait(call, left, &|| stopping.load(Ordering::SeqCst))?;
        Ok(json!({"result": result}))
    }

    /// `tools.respond`: a mod answers a call (the page's `window.flare.respond`, or a helper with the mod's
    /// token). Only the mod the call was routed to may answer it.
    fn tools_respond(&self, ctx: &CallContext, params: &Value) -> Result<Value, CtlError> {
        let hub = self.hub()?;
        let Grant::Mod { id, .. } = &ctx.grant else {
            return Err(CtlError::new(
                code::FORBIDDEN,
                "only a mod answers a tool call",
            ));
        };
        let call_id = str_param(params, "call_id")?
            .ok_or_else(|| CtlError::new(code::BAD_PARAMS, "call_id is required"))?;
        let outcome = match (params.get("result"), params.get("error")) {
            (Some(r), None) => Ok(r.clone()),
            (None, Some(Value::String(e))) => Err(e.clone()),
            _ => {
                return Err(CtlError::new(
                    code::BAD_PARAMS,
                    "send either result or error (a string), not both and not neither",
                ));
            }
        };
        hub.respond(id, call_id, outcome)?;
        Ok(json!({}))
    }

    // ---- fill (the `fill` permission, 2026-10-28) ------------------------------------------------------------------

    /// The plugin that calls a `fill` method, its name as Settings shows it, and its generation.
    fn fill_caller(
        &self,
        ctx: &CallContext,
        what: &str,
    ) -> Result<(String, String, u64), CtlError> {
        let id = Self::plugin_caller(ctx, what)?;
        let hub = self.hub()?;
        let name = hub
            .found(&id)
            .map_or_else(|| id.clone(), |f| f.manifest.name.clone());
        Ok((id.clone(), name, hub.generation(&id)))
    }

    /// The tab a fill acts on: the caller's own, one it may see (`tabs`, or a click there), or the tab an agent's
    /// call to this plugin came from while that call waits for the plugin's answer (tool scope). A helper sits in no
    /// tab, so it names one.
    fn fill_tab(&self, ctx: &CallContext, params: &Value) -> Result<String, CtlError> {
        match str_param(params, "tab")? {
            Some(t) if t.is_empty() || t.len() > 64 => {
                Err(CtlError::new(code::BAD_PARAMS, "tab is a tab's id"))
            }
            Some(t) if self.may_see_tab(ctx, t) || self.tool_scope(ctx, t) => Ok(t.to_string()),
            Some(_) => Err(CtlError::new(
                code::FORBIDDEN,
                "a plugin fills only its own tab, a tab it may see, or the tab of an agent's call it is answering",
            )),
            None => Self::tab_of_call(ctx),
        }
    }

    /// Whether an agent's call from `tab` to the calling plugin waits for the plugin's answer.
    fn tool_scope(&self, ctx: &CallContext, tab: &str) -> bool {
        match (&ctx.grant, &self.mods) {
            (Grant::Mod { id, .. }, Some(hub)) => hub.has_call_from(id, tab),
            _ => false,
        }
    }

    /// `browser.fill.page {tab?}`: the address of the tab's web page, so the plugin can pick the login for it.
    fn fill_page(&self, ctx: &CallContext, params: &Value) -> Result<Value, CtlError> {
        Self::plugin_caller(ctx, "the address of a page to fill")?;
        only_fields(params, &["tab"], "browser.fill.page")?;
        let tab = self.fill_tab(ctx, params)?;
        self.ask(CtlOp::FillPage { tab })
    }

    /// `browser.fill {tab?, login_name, login_uris, username?, password?, totp?}`: checked here (the params, the tab,
    /// the session's Deny, six asks a minute), then the window checks the page's site, asks the person and fills.
    /// The values are never in an error, a log line or the answer.
    fn fill_call(&self, ctx: &CallContext, params: &Value) -> Result<Value, CtlError> {
        let (plugin, plugin_name, generation) = self.fill_caller(ctx, "filling a login")?;
        let tab = self.fill_tab(ctx, params)?;
        let via_call = !self.may_see_tab(ctx, &tab);
        let req = flare_mods::fill::parse_fill(params)?;
        let hub = self.hub()?;
        if hub.asks().denied(&plugin) {
            log::info!("fill: plugin {plugin}: declined (denied for this session)");
            return Ok(json!({"declined": true, "for_session": true}));
        }
        hub.asks().admit(&plugin, Instant::now())?;
        let op = CtlOp::Fill {
            plugin,
            plugin_name,
            tab,
            login_name: req.login_name,
            login_uris: req.login_uris,
            values: req.values,
            via_call,
            generation,
        };
        self.ask_by(op, Instant::now() + ASK_CALL_WAIT)
    }

    /// `secret.ask {title, label, sub?, button?}`: Flare's own masked field; the text comes back to this plugin only.
    fn secret_ask(&self, ctx: &CallContext, params: &Value) -> Result<Value, CtlError> {
        let (plugin, plugin_name, generation) = self.fill_caller(ctx, "asking for a secret")?;
        let req = flare_mods::fill::parse_secret(params)?;
        self.hub()?.asks().admit(&plugin, Instant::now())?;
        self.ask_by(
            CtlOp::SecretAsk {
                plugin,
                plugin_name,
                title: req.title,
                label: req.label,
                sub: req.sub,
                button: req.button,
                generation,
            },
            Instant::now() + ASK_CALL_WAIT,
        )
    }

    /// `panel.open` kind `mod:<id>`: the mod's panel on the caller's tab (or, for a plugin that holds `tabs`, on a
    /// tab it names).
    fn open_mod_panel(
        &self,
        ctx: &CallContext,
        mod_id: &str,
        params: &Value,
    ) -> Result<Value, CtlError> {
        let tab = self.scoped_tab(ctx, params)?;
        let hub = self.hub()?;
        let generation = hub.generation(mod_id);
        if hub.grant_of_mod(mod_id).is_none() {
            return Err(CtlError::new(
                code::NOT_FOUND,
                format!("no mod \"{mod_id}\" is turned on"),
            ));
        }
        match hub.found(mod_id) {
            Some(f) if f.manifest.panel.is_some() => {}
            _ => {
                return Err(CtlError::new(
                    code::NOT_FOUND,
                    format!("the mod \"{mod_id}\" has no panel"),
                ));
            }
        }
        let own = ctx.tab.as_deref() == Some(tab.as_str());
        self.ask(CtlOp::ShowMod {
            tab,
            mod_id: mod_id.to_string(),
            own,
            generation,
        })
    }

    /// Where mini-apps are written and the user's network switch for them (stage 5).
    pub fn with_apps(mut self, apps: AppsCfg) -> Self {
        self.apps = Some(apps);
        self
    }

    /// A still clock for the decor limit, so a test can spend a second's worth at one instant.
    #[cfg(test)]
    fn with_clock(mut self, clock: fn() -> Instant) -> Self {
        self.clock = clock;
        self
    }

    /// The user's switch for `browser.eval` (S4); without one it is off.
    pub fn with_eval(mut self, eval: EvalSwitch) -> Self {
        self.eval = eval;
        self
    }

    /// Posts `op` to the UI thread and waits for its answer, at most [`UI_WAIT`].
    fn ask(&self, op: CtlOp) -> Result<Value, CtlError> {
        self.ask_by(op, Instant::now() + UI_WAIT)
    }

    /// [`AppDispatcher::ask`] with a deadline the whole call shares, so two asks and a disk read in
    /// between still answer inside it.
    fn ask_by(&self, op: CtlOp, deadline: Instant) -> Result<Value, CtlError> {
        let shutting = || CtlError::new(code::UNAVAILABLE, "Flare is shutting down");
        if self.stopping.load(Ordering::SeqCst) {
            return Err(shutting());
        }
        let (tx, rx) = mpsc::channel();
        (self.post)(UiMsg::Ctl(CtlRequestBox(CtlRequest {
            op,
            deadline,
            reply: tx,
        })));
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            // sync-ok: a worker thread of the control channel waits for the UI thread, never the UI thread itself
            match rx.recv_timeout(left.min(STOP_SLICE)) {
                Ok(answer) => return answer,
                // The message was dropped unanswered: the window is gone, or Flare is ending.
                Err(RecvTimeoutError::Disconnected) => return Err(shutting()),
                Err(RecvTimeoutError::Timeout) => {
                    if self.stopping.load(Ordering::SeqCst) {
                        return Err(shutting());
                    }
                    if left.is_zero() {
                        return Err(CtlError::new(
                            code::TIMEOUT,
                            "Flare's window did not answer in time",
                        ));
                    }
                }
            }
        }
    }

    /// The call's tab, or `no_tab`.
    fn tab_of_call(ctx: &CallContext) -> Result<String, CtlError> {
        ctx.tab.clone().ok_or_else(|| {
            CtlError::new(
                code::NO_TAB,
                "this connection is not in a Flare tab (no tab token)",
            )
        })
    }

    /// `view.image` and `view.file`: the path is resolved again here (rule 13), must be on the click
    /// allowlist as named and as the file system names it (rule 14) and be of the kind asked for, and
    /// only then does the UI thread get it.
    fn view_file(
        &self,
        ctx: &CallContext,
        want: PanelKind,
        params: &Value,
    ) -> Result<Value, CtlError> {
        let tab = Self::tab_of_call(ctx)?;
        let path = str_param(params, "path")?
            .filter(|p| !p.trim().is_empty())
            .ok_or_else(|| CtlError::new(code::BAD_PARAMS, "path is required"))?;
        if path.len() > 4096 || path.contains('\0') {
            return Err(CtlError::new(code::BAD_PARAMS, "that is not a usable path"));
        }
        let title = title_param(params)?;
        if want == PanelKind::Text
            && let Some(l) = params.get("line").filter(|l| !l.is_null())
            && !l.as_u64().is_some_and(|n| n >= 1)
        {
            return Err(CtlError::new(
                code::BAD_PARAMS,
                "line must be a whole number from 1",
            ));
        }
        // One deadline for the whole call: the folder question, the disk read and the show.
        let deadline = Instant::now() + UI_WAIT;
        // An absolute path needs no folder, so the UI thread is not asked for one.
        let base = if std::path::Path::new(path).is_absolute() {
            String::new()
        } else {
            let cwd = self.ask_by(CtlOp::TabCwd { tab: tab.clone() }, deadline)?;
            cwd["cwd"].as_str().unwrap_or_default().to_string()
        };
        // A path nobody may open (missing, a share, a program, outside the allowlist) answers the same
        // way, so a probe learns nothing about the disk.
        let (abs, real) = (self.resolve)(&base, path, &self.home)
            .ok_or_else(|| CtlError::new(code::NOT_FOUND, "no such file that Flare may show"))?;
        let kind = preview::classify_resolved(&abs, &real).ok_or_else(|| {
            CtlError::new(
                code::BAD_PARAMS,
                "Flare shows pictures (png, jpg, gif, webp, bmp, ico) and text or code files only; a PDF, sound or video plays in the browser panel (open_page) from a local web server",
            )
        })?;
        // A file on a mapped network drive (or a link to one): the panel reads local drives only. Said as the
        // resolve step says it, not as a file of the other kind.
        if !preview::loadable(&real, &kind) {
            return Err(CtlError::new(
                code::NOT_FOUND,
                "no such file that Flare may show",
            ));
        }
        if kind != want {
            let (this, other) = if want == PanelKind::Image {
                ("a picture", "show_file")
            } else {
                ("a text or code file", "show_image")
            };
            return Err(CtlError::new(
                code::BAD_PARAMS,
                format!("that file is not {this}; {other} shows it"),
            ));
        }
        // The disk read can have used the time: a show that could not be answered in time is not sent.
        if Instant::now() >= deadline {
            return Err(CtlError::new(
                code::TIMEOUT,
                "Flare took too long to look at that file",
            ));
        }
        self.ask_by(
            CtlOp::ShowFile {
                tab,
                kind,
                abs: real,
                title,
            },
            deadline,
        )
    }
}

impl AppDispatcher {
    fn apps(&self) -> Result<&AppsCfg, CtlError> {
        self.apps.as_ref().ok_or_else(|| {
            CtlError::new(
                code::UNAVAILABLE,
                "mini-apps are not available in this run of Flare",
            )
        })
    }

    /// Writes the app's files and asks the UI thread to show it. The folder belongs to the UI thread
    /// from the moment it is asked (it removes it when the panel goes, and when it refuses).
    fn show_app(
        &self,
        tab: String,
        files: &[(&str, &[u8])],
        allow_network: bool,
        title: String,
        diagram: bool,
        deadline: Instant,
    ) -> Result<Value, CtlError> {
        let apps = self.apps()?;
        let seq = apps.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let spec = app::write_app(&apps.root, seq, files, allow_network).map_err(|e| {
            CtlError::new(
                code::INTERNAL,
                format!("the page could not be written: {e}"),
            )
        })?;
        self.ask_by(
            CtlOp::ShowApp {
                tab,
                spec,
                title,
                diagram,
            },
            deadline,
        )
    }

    /// `view.html`: a page the agent wrote, from `html` or from an `html` file named by `path`.
    fn view_html(&self, ctx: &CallContext, params: &Value) -> Result<Value, CtlError> {
        let tab = Self::tab_of_call(ctx)?;
        let apps = self.apps()?;
        let bad = |m: &str| CtlError::new(code::BAD_PARAMS, m.to_string());
        let html = str_param(params, "html")?;
        let path = str_param(params, "path")?;
        let title = title_param(params)?;
        let wants_network = bool_param(params, "allow_network")?.unwrap_or(false);
        // One deadline for the whole call: the folder question, the disk read, the write and the load.
        let deadline = Instant::now() + BROWSER_WAIT;
        let text = match (html, path) {
            (Some(h), None) => {
                if h.trim().is_empty() {
                    return Err(bad("html is empty"));
                }
                if h.len() > app::HTML_MAX {
                    return Err(bad("html is over 4 MB"));
                }
                h.to_string()
            }
            (None, Some(p)) => {
                if p.trim().is_empty() || p.len() > 4096 || p.contains('\0') {
                    return Err(bad("that is not a usable path"));
                }
                let base = if std::path::Path::new(p).is_absolute() {
                    String::new()
                } else {
                    let cwd = self.ask_by(CtlOp::TabCwd { tab: tab.clone() }, deadline)?;
                    cwd["cwd"].as_str().unwrap_or_default().to_string()
                };
                // A page nobody may show (missing, a share, not html, a link that changes kind) answers
                // the same way, so a probe learns nothing about the disk.
                let (_, real) = (apps.resolve_html)(&base, p, &self.home).ok_or_else(|| {
                    CtlError::new(code::NOT_FOUND, "no such html file that Flare may show")
                })?;
                read_page(&real)?
            }
            _ => return Err(bad("give either html or path, not both and not neither")),
        };
        // The network switch is the user's consent for the AGENT's pages, never for a mod's.
        if wants_network && matches!(ctx.grant, Grant::Mod { .. }) {
            return Err(CtlError::new(
                code::FORBIDDEN,
                "a mod's page cannot reach the network (allow_network is not for mods)",
            ));
        }
        let allow = wants_network && (apps.network)();
        let page = if apps.policy {
            app::wrap_html(&text, allow)
        } else {
            text
        };
        let title = title.unwrap_or_else(|| app::default_title(false).to_string());
        self.show_app(
            tab,
            &[(app::PAGE, page.as_bytes())],
            allow,
            title,
            false,
            deadline,
        )
    }

    /// `view.audio {path | url, title?, autoplay?}` (plugins phase 6): Flare's audio player in the caller's panel. A
    /// `path` is resolved again here (rule 13), must be on the click allowlist as named and as the file system names it
    /// (rule 14) and be a sound file by both names; it is copied beside the page (at most 100 MB). A `url` must be a
    /// public `https` address (never loopback, a private range or a local name), and the page may load that origin's
    /// media only. The page is Flare's: no script of the caller's, and it plays by itself only with `autoplay`, once.
    fn view_audio(&self, ctx: &CallContext, params: &Value) -> Result<Value, CtlError> {
        use flare_surfaces::audio;
        let tab = Self::tab_of_call(ctx)?;
        self.apps()?;
        only_fields(params, &["path", "url", "title", "autoplay"], "view.audio")?;
        let bad = |m: &str| CtlError::new(code::BAD_PARAMS, m.to_string());
        let title = title_param(params)?.unwrap_or_else(|| audio::DEFAULT_TITLE.to_string());
        let autoplay = bool_param(params, "autoplay")?.unwrap_or(false);
        let deadline = Instant::now() + BROWSER_WAIT;
        match (str_param(params, "path")?, str_param(params, "url")?) {
            (Some(p), None) => {
                if p.trim().is_empty() || p.len() > 4096 || p.contains('\0') {
                    return Err(bad("that is not a usable path"));
                }
                let base = if std::path::Path::new(p).is_absolute() {
                    String::new()
                } else {
                    let cwd = self.ask_by(CtlOp::TabCwd { tab: tab.clone() }, deadline)?;
                    cwd["cwd"].as_str().unwrap_or_default().to_string()
                };
                // A file nobody may open answers the same way as a missing one, so a probe learns nothing.
                let (abs, real) = (self.resolve)(&base, p, &self.home).ok_or_else(|| {
                    CtlError::new(code::NOT_FOUND, "no such file that Flare may play")
                })?;
                let ext = audio::audio_ext(&abs, &real).ok_or_else(|| {
                    bad(&format!(
                        "Flare plays sound files only ({})",
                        audio::AUDIO_EXTS.join(", ")
                    ))
                })?;
                let bytes = read_audio(&real)?;
                let track = audio::track_name(ext);
                let page = audio::page(&track, &title, None, autoplay);
                self.show_app(
                    tab,
                    &[(app::PAGE, page.as_bytes()), (track.as_str(), &bytes)],
                    false,
                    title,
                    false,
                    deadline,
                )
            }
            (None, Some(u)) => {
                if u.len() > 2048 {
                    return Err(bad("url is over 2 KB"));
                }
                // An address is the internet: a plugin plays one only with `network` (else the address could carry
                // what the plugin knows out of the PC).
                if let Grant::Mod { id, .. } = &ctx.grant
                    && !self.hub()?.network_of(id)
                {
                    return Err(CtlError::new(
                        code::FORBIDDEN,
                        "playing an address needs the network permission",
                    ));
                }
                if let Some(why) = flare_surfaces::net::refusal(u) {
                    return Err(bad(&format!("that address cannot be played: {why}")));
                }
                let origin =
                    audio::origin_of(u).ok_or_else(|| bad("that address has no origin"))?;
                let page = audio::page(u, &title, Some(&origin), autoplay);
                // The browser of networked mini-apps: the page's own policy lets in this origin's media and nothing else.
                self.show_app(
                    tab,
                    &[(app::PAGE, page.as_bytes())],
                    true,
                    title,
                    false,
                    deadline,
                )
            }
            _ => Err(bad("give either path or url, not both and not neither")),
        }
    }

    /// `view.diagram`: Mermaid text drawn by the bundled build.
    fn view_diagram(&self, ctx: &CallContext, params: &Value) -> Result<Value, CtlError> {
        let tab = Self::tab_of_call(ctx)?;
        let apps = self.apps()?;
        let src = str_param(params, "mermaid")?
            .ok_or_else(|| CtlError::new(code::BAD_PARAMS, "mermaid is required"))?;
        if src.trim().is_empty() {
            return Err(CtlError::new(code::BAD_PARAMS, "mermaid is empty"));
        }
        if src.len() > app::DIAGRAM_MAX {
            return Err(CtlError::new(code::BAD_PARAMS, "mermaid is over 200 KB"));
        }
        let title = title_param(params)?.unwrap_or_else(|| app::default_title(true).to_string());
        let page = app::wrap_html(&app::diagram_page(src), false);
        self.show_app(
            tab,
            &[
                (app::PAGE, page.as_bytes()),
                (app::MERMAID_FILE, apps.mermaid),
            ],
            false,
            title,
            true,
            Instant::now() + BROWSER_WAIT,
        )
    }
}

/// Reads a page the backend has resolved: at most the cap, and text.
fn read_page(real: &str) -> Result<String, CtlError> {
    use std::io::Read;
    let bad = |m: &str| CtlError::new(code::BAD_PARAMS, m.to_string());
    let file = std::fs::File::open(real) // sync-ok: the channel call's own thread, never the UI thread
        .map_err(|_| CtlError::new(code::NOT_FOUND, "no such html file that Flare may show"))?;
    let mut bytes = Vec::new();
    file.take(app::HTML_MAX as u64 + 1)
        .read_to_end(&mut bytes) // sync-ok: the channel call's own thread, never the UI thread
        .map_err(|_| bad("that page could not be read"))?;
    if bytes.len() > app::HTML_MAX {
        return Err(bad("that page is over 4 MB"));
    }
    let text = String::from_utf8(bytes).map_err(|_| bad("that page is not UTF-8 text"))?;
    if text.trim().is_empty() {
        return Err(bad("that page is empty"));
    }
    Ok(text)
}

/// Reads a sound file the backend has resolved: at most [`flare_surfaces::audio::AUDIO_MAX`].
fn read_audio(real: &str) -> Result<Vec<u8>, CtlError> {
    use std::io::Read;
    let file = std::fs::File::open(real) // sync-ok: the channel call's own thread, never the UI thread
        .map_err(|_| CtlError::new(code::NOT_FOUND, "no such file that Flare may play"))?;
    let max = flare_surfaces::audio::AUDIO_MAX;
    let mut bytes = Vec::new();
    file.take(max + 1)
        .read_to_end(&mut bytes) // sync-ok: the channel call's own thread, never the UI thread
        .map_err(|_| CtlError::new(code::BAD_PARAMS, "that sound file could not be read"))?;
    if bytes.len() as u64 > max {
        return Err(CtlError::new(
            code::BAD_PARAMS,
            "that sound file is over 100 MB",
        ));
    }
    Ok(bytes)
}

/// `bad_params` for a field the method does not take.
fn only_fields(params: &Value, allowed: &[&str], method: &str) -> Result<(), CtlError> {
    match params
        .as_object()
        .and_then(|o| o.keys().find(|k| !allowed.contains(&k.as_str())))
    {
        Some(k) => Err(CtlError::new(
            code::BAD_PARAMS,
            format!("{k} is not a field of {method}"),
        )),
        None => Ok(()),
    }
}

fn str_param<'a>(params: &'a Value, key: &str) -> Result<Option<&'a str>, CtlError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.as_str())),
        Some(_) => Err(CtlError::new(
            code::BAD_PARAMS,
            format!("{key} must be a string"),
        )),
    }
}

/// A `url` param, vetted: http, https or about:blank, no login, no spaces (`normalize_url`).
fn web_url(u: &str) -> Result<String, CtlError> {
    flare_surfaces::web::normalize_url(u).map_err(|e| {
        CtlError::new(
            code::BAD_PARAMS,
            format!("that is not an address to open: {e}"),
        )
    })
}

/// The optional `title`: one line, trimmed, at most [`TITLE_MAX`] characters (cut, not refused).
fn title_param(params: &Value) -> Result<Option<String>, CtlError> {
    Ok(str_param(params, "title")?
        .map(|t| {
            t.chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .take(TITLE_MAX)
                .collect::<String>()
                .trim()
                .to_string()
        })
        .filter(|t| !t.is_empty()))
}

fn num_param(params: &Value, key: &str) -> Result<Option<f64>, CtlError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => match n.as_f64() {
            Some(v) if v.is_finite() => Ok(Some(v)),
            _ => Err(CtlError::new(
                code::BAD_PARAMS,
                format!("{key} must be a number"),
            )),
        },
        Some(_) => Err(CtlError::new(
            code::BAD_PARAMS,
            format!("{key} must be a number"),
        )),
    }
}

fn bool_param(params: &Value, key: &str) -> Result<Option<bool>, CtlError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(CtlError::new(
            code::BAD_PARAMS,
            format!("{key} must be true or false"),
        )),
    }
}

/// The `ref` param: the short name a look gave an element (`e12`).
fn ref_param(params: &Value) -> Result<Option<String>, CtlError> {
    match str_param(params, "ref")? {
        None => Ok(None),
        Some(r)
            if !r.is_empty() && r.len() <= 12 && r.bytes().all(|b| b.is_ascii_alphanumeric()) =>
        {
            Ok(Some(r.to_string()))
        }
        Some(_) => Err(CtlError::new(
            code::BAD_PARAMS,
            "ref is an element name from the last look, such as e12",
        )),
    }
}

/// The stage 4 methods as agent jobs, and how long the call may wait for the browser's answer.
pub fn agent_op(method: Method, params: &Value) -> Result<(AgentOp, Duration), CtlError> {
    let bad = |m: &str| CtlError::new(code::BAD_PARAMS, m.to_string());
    let op = match method {
        Method::BrowserLook => AgentOp::Look { picture: true },
        Method::BrowserClick => {
            let target = match (
                ref_param(params)?,
                num_param(params, "x")?,
                num_param(params, "y")?,
            ) {
                (Some(r), None, None) => Target::Ref(r),
                (None, Some(x), Some(y)) => Target::Point(x, y),
                _ => {
                    return Err(bad(
                        "give either ref, or both x and y, not both and not neither",
                    ));
                }
            };
            let button = match str_param(params, "button")? {
                None => Button::Left,
                Some(b) => {
                    Button::parse(b).ok_or_else(|| bad("button is left, right or middle"))?
                }
            };
            AgentOp::Click {
                target,
                button,
                double: bool_param(params, "double")?.unwrap_or(false),
            }
        }
        Method::BrowserType => {
            let text = str_param(params, "text")?.ok_or_else(|| bad("text is required"))?;
            if text.chars().count() > agent_web::TYPE_MAX {
                return Err(bad(&format!(
                    "text is over {} characters",
                    agent_web::TYPE_MAX
                )));
            }
            AgentOp::Type {
                text: text.to_string(),
                target: ref_param(params)?,
            }
        }
        Method::BrowserPress => {
            let key = str_param(params, "key")?.ok_or_else(|| bad("key is required"))?;
            let mut spec = String::new();
            match params.get("modifiers") {
                None | Some(Value::Null) => {}
                Some(Value::Array(list)) => {
                    for m in list.iter().take(8) {
                        let m = m
                            .as_str()
                            .ok_or_else(|| bad("modifiers is a list of names"))?;
                        spec.push_str(m);
                        spec.push('+');
                    }
                }
                Some(_) => return Err(bad("modifiers is a list of names")),
            }
            spec.push_str(key);
            let key = agent_web::parse_key(&spec).map_err(|e| bad(&e))?;
            AgentOp::Press { key }
        }
        Method::BrowserScroll => {
            let dx = num_param(params, "dx")?.unwrap_or(0.0);
            let dy = num_param(params, "dy")?.unwrap_or(0.0);
            if dx == 0.0 && dy == 0.0 {
                return Err(bad("give dx or dy (page pixels; dy positive scrolls down)"));
            }
            if dx.abs() > 100_000.0 || dy.abs() > 100_000.0 {
                return Err(bad("dx and dy are at most 100000"));
            }
            AgentOp::Scroll {
                dx,
                dy,
                target: ref_param(params)?,
            }
        }
        Method::BrowserWait => {
            let text = str_param(params, "text")?;
            let selector = str_param(params, "selector")?;
            let ms = num_param(params, "ms")?;
            let what = match (text, selector, ms) {
                (Some(t), None, None) => {
                    if t.is_empty() || t.chars().count() > agent_web::WAIT_TEXT_MAX {
                        return Err(bad("text is 1 to 500 characters"));
                    }
                    WaitFor::Text(t.to_string())
                }
                (None, Some(s), None) => {
                    if s.is_empty() || s.chars().count() > agent_web::WAIT_TEXT_MAX {
                        return Err(bad("selector is 1 to 500 characters"));
                    }
                    WaitFor::Selector(s.to_string())
                }
                (None, None, Some(ms)) => {
                    if ms < 0.0 {
                        return Err(bad("ms is zero or more"));
                    }
                    WaitFor::Ms((ms as u64).min(agent_web::WAIT_MAX_MS))
                }
                _ => return Err(bad("give exactly one of text, selector or ms")),
            };
            let given = match num_param(params, "timeout_ms")? {
                Some(t) if t < 0.0 => return Err(bad("timeout_ms is zero or more")),
                Some(t) => Some(t as u64),
                None => None,
            };
            let timeout_ms = match &what {
                WaitFor::Ms(ms) => *ms,
                _ => agent_web::wait_timeout(given),
            };
            let wait = Duration::from_millis(timeout_ms) + Duration::from_secs(10);
            return Ok((AgentOp::Wait { what, timeout_ms }, wait));
        }
        Method::BrowserEval => {
            let e =
                str_param(params, "expression")?.ok_or_else(|| bad("expression is required"))?;
            if e.trim().is_empty() || e.chars().count() > agent_web::EVAL_MAX {
                return Err(bad("expression is 1 to 20000 characters"));
            }
            AgentOp::Eval {
                expression: e.to_string(),
            }
        }
        _ => return Err(CtlError::new(code::INTERNAL, "not an agent browser method")),
    };
    Ok((op, BROWSER_WAIT))
}

impl Dispatcher for AppDispatcher {
    /// A mod's token (stage 8): the grant of exactly its permissions while it is on, nothing once it is off.
    fn resolve_token(&self, token: &str) -> Option<Grant> {
        self.mods.as_ref()?.grant_of_token(token)
    }

    fn resolve_tab_token(&self, tab_token: &str) -> Option<String> {
        (self.tab_of)(tab_token)
    }

    /// The agent may run scripts in the page only while the user's switch is on (S4); asked at every
    /// request, so the switch is live.
    fn agent_grant(&self) -> Grant {
        Grant::Agent {
            eval: (self.eval)(),
        }
    }

    /// What this build does. `panel.open` answers (`unavailable`, no web panel yet) but is not listed:
    /// a client that reads this list must not offer what cannot be done.
    fn capabilities(&self) -> Vec<Method> {
        let mut all = vec![
            Method::Hello,
            Method::TabsList,
            Method::PanelClose,
            Method::PanelState,
            Method::PanelOpen,
            Method::BrowserOpen,
            Method::BrowserNavigate,
            Method::BrowserLook,
            Method::BrowserClick,
            Method::BrowserType,
            Method::BrowserPress,
            Method::BrowserScroll,
            Method::BrowserWait,
            Method::BrowserEval,
            Method::ViewImage,
            Method::ViewFile,
            Method::ViewText,
            Method::ConvoState,
            Method::TabsBadgeSet,
            Method::TabsBadgeClear,
            Method::TabsProgressSet,
            Method::TabsProgressClear,
        ];
        // Mini-apps, and the audio player that is one, are listed only when this run can write them.
        if self.apps.is_some() {
            all.extend([Method::ViewHtml, Method::ViewDiagram, Method::ViewAudio]);
        }
        // Mods (stage 8) are listed only where there is a hub for them, and so are the plugins' own surfaces.
        if self.mods.is_some() {
            all.extend([
                Method::ModsTools,
                Method::ModsCall,
                Method::ToolsRespond,
                Method::SidebarSet,
                Method::ActionsSet,
                Method::SessionRead,
                Method::TabsInput,
                Method::StorageGet,
                Method::StorageSet,
                Method::BarShow,
                Method::BarHide,
                Method::AgentsMotionSet,
                Method::AgentsMotionClear,
                Method::BrowserFill,
                Method::BrowserFillPage,
                Method::SecretAsk,
            ]);
        }
        all
    }

    fn call(&self, ctx: &CallContext, method: Method, params: Value) -> Result<Value, CtlError> {
        // A plugin's connection (its helper, later) spends from the plugin's one budget, here on the worker before
        // anything is posted. A page's messages were spent as the window took them (`conn` 0).
        if ctx.conn != 0
            && let Grant::Mod { id, .. } = &ctx.grant
            && let Some(limiter) = self.mods.as_ref().and_then(|h| h.limits().of(id))
            && let Err(over) = limiter.admit(params.to_string().len(), Instant::now())
        {
            return Err(CtlError::new(code::RATE_LIMITED, over.why()));
        }
        match method {
            // A plugin that holds `tabs` sees every tab, with what its agent is doing (plugins phase 3).
            Method::TabsList
                if matches!(ctx.grant, Grant::Mod { .. }) && ctx.grant.may_act_on_any_tab() =>
            {
                self.ask(CtlOp::TabsWide)
            }
            Method::TabsList => {
                // The agent and a mod see their own tab only (id and title, never a screen: S3). Tab titles carry
                // folders, commands and task names, which a mod's "show its panel" approval never covered.
                let own = match &ctx.tab {
                    Some(t) => Some(t.clone()),
                    None => return Ok(json!({"tabs": []})),
                };
                self.ask(CtlOp::Tabs { own })
            }
            Method::PanelState => self.ask(CtlOp::PanelState {
                tab: Self::tab_of_call(ctx)?,
            }),
            Method::PanelClose => self.ask(CtlOp::PanelClose {
                tab: Self::tab_of_call(ctx)?,
            }),
            Method::PanelOpen => {
                // A mod opens its OWN panel and nothing else: a web panel would reach the network in the
                // user's shared profile (S4/S5), the rest are not the mod's to drive (rule 207). A plugin that holds
                // `tabs` may open it on a tab it names.
                let mod_kind = str_param(&params, "kind")?.is_some_and(|k| k.starts_with("mod:"));
                let tab = if mod_kind {
                    self.scoped_tab(ctx, &params)?
                } else {
                    Self::tab_of_call(ctx)?
                };
                if let Grant::Mod { id, .. } = &ctx.grant {
                    let own = format!("mod:{id}");
                    if let Some(k) = str_param(&params, "kind")?
                        && k != own
                    {
                        return Err(CtlError::new(
                            code::FORBIDDEN,
                            format!("a mod may open only its own panel ({own}), not \"{k}\""),
                        ));
                    }
                }
                match str_param(&params, "kind")? {
                    None => Err(CtlError::new(code::BAD_PARAMS, "kind is required")),
                    Some(k @ "changes") => {
                        // Native panels (the conversation): no file, no address, nothing to wait for.
                        if let Some(extra) = ["url", "path", "text"]
                            .iter()
                            .find(|p| params.get(**p).is_some_and(|v| !v.is_null()))
                        {
                            return Err(CtlError::new(
                                code::BAD_PARAMS,
                                format!("{extra} does not go with kind {k}"),
                            ));
                        }
                        let kind = PanelKind::parse(k)
                            .ok_or_else(|| CtlError::new(code::INTERNAL, "unknown panel kind"))?;
                        self.ask(CtlOp::ShowConvo { tab, kind })
                    }
                    Some("web") => {
                        let url = match str_param(&params, "url")? {
                            Some(u) => web_url(u)?,
                            None => "about:blank".to_string(),
                        };
                        self.ask_by(
                            CtlOp::BrowserOpen { tab, url },
                            Instant::now() + BROWSER_WAIT,
                        )
                    }
                    Some(k) if k.starts_with("mod:") => {
                        if let Some(extra) = ["url", "path", "text"]
                            .iter()
                            .find(|p| params.get(**p).is_some_and(|v| !v.is_null()))
                        {
                            return Err(CtlError::new(
                                code::BAD_PARAMS,
                                format!("{extra} does not go with kind {k}"),
                            ));
                        }
                        let id = k.trim_start_matches("mod:");
                        if !flare_surfaces::kind::valid_mod_id(id) {
                            return Err(CtlError::new(code::BAD_PARAMS, "that is not a mod id"));
                        }
                        self.open_mod_panel(ctx, id, &params)
                    }
                    Some(_) => Err(CtlError::new(
                        code::BAD_PARAMS,
                        "panel.open takes kind web, changes or mod:<id>; use view.image, view.file or view.text",
                    )),
                }
            }
            Method::BrowserOpen => {
                let tab = Self::tab_of_call(ctx)?;
                let url = str_param(&params, "url")?
                    .ok_or_else(|| CtlError::new(code::BAD_PARAMS, "url is required"))?;
                let url = web_url(url)?;
                self.ask_by(
                    CtlOp::BrowserOpen { tab, url },
                    Instant::now() + BROWSER_WAIT,
                )
            }
            Method::BrowserNavigate => {
                let tab = Self::tab_of_call(ctx)?;
                let url = str_param(&params, "url")?;
                let action = str_param(&params, "action")?;
                let to = match (url, action) {
                    (Some(u), None) => NavTo::Url(web_url(u)?),
                    (None, Some("back")) => NavTo::Back,
                    (None, Some("forward")) => NavTo::Forward,
                    (None, Some("reload")) => NavTo::Reload,
                    (None, Some(_)) => {
                        return Err(CtlError::new(
                            code::BAD_PARAMS,
                            "action is back, forward or reload",
                        ));
                    }
                    _ => {
                        return Err(CtlError::new(
                            code::BAD_PARAMS,
                            "give either url or action, not both and not neither",
                        ));
                    }
                };
                self.ask_by(
                    CtlOp::BrowserNavigate { tab, to },
                    Instant::now() + BROWSER_WAIT,
                )
            }
            Method::BrowserLook
            | Method::BrowserClick
            | Method::BrowserType
            | Method::BrowserPress
            | Method::BrowserScroll
            | Method::BrowserWait
            | Method::BrowserEval => {
                let tab = Self::tab_of_call(ctx)?;
                let (op, wait) = agent_op(method, &params)?;
                self.ask_by(CtlOp::BrowserAgent { tab, op }, Instant::now() + wait)
            }
            Method::ConvoState => self.ask(CtlOp::ConvoState {
                tab: Self::tab_of_call(ctx)?,
            }),
            Method::TabsBadgeSet
            | Method::TabsBadgeClear
            | Method::TabsProgressSet
            | Method::TabsProgressClear => self.decor_call(ctx, method, &params),
            Method::SidebarSet => self.sidebar_set(ctx, &params),
            Method::ActionsSet => self.actions_set(ctx, &params),
            Method::SessionRead => self.session_read(ctx, &params),
            Method::TabsInput => self.tabs_input(ctx, &params),
            Method::StorageGet | Method::StorageSet => self.storage_call(ctx, method, &params),
            Method::BarShow | Method::BarHide => self.bar_call(ctx, method, &params),
            Method::AgentsMotionSet | Method::AgentsMotionClear => {
                self.motion_call(ctx, method, &params)
            }
            Method::ViewAudio => self.view_audio(ctx, &params),
            Method::ModsTools => self.mods_tools(),
            Method::ModsCall => self.mods_call(ctx, &params),
            Method::ToolsRespond => self.tools_respond(ctx, &params),
            Method::BrowserFill => self.fill_call(ctx, &params),
            Method::BrowserFillPage => self.fill_page(ctx, &params),
            Method::SecretAsk => self.secret_ask(ctx, &params),
            Method::ViewHtml => self.view_html(ctx, &params),
            Method::ViewDiagram => self.view_diagram(ctx, &params),
            Method::ViewImage => self.view_file(ctx, PanelKind::Image, &params),
            Method::ViewFile => self.view_file(ctx, PanelKind::Text, &params),
            Method::ViewText => {
                let tab = Self::tab_of_call(ctx)?;
                let text = str_param(&params, "text")?
                    .ok_or_else(|| CtlError::new(code::BAD_PARAMS, "text is required"))?;
                if text.len() > TEXT_MAX {
                    return Err(CtlError::new(
                        code::BAD_PARAMS,
                        format!("text is over {} MB", TEXT_MAX / (1024 * 1024)),
                    ));
                }
                let title = title_param(&params)?;
                // `language` is a hint for a later syntax-aware viewer; it is checked and ignored.
                str_param(&params, "language")?;
                self.ask(CtlOp::ShowText {
                    tab,
                    text: text.to_string(),
                    title,
                })
            }
            other => Err(CtlError::new(
                code::UNAVAILABLE,
                format!("{} is not built in this version of Flare", other.name()),
            )),
        }
    }
}

/// The pipe of this run. The default profile (`%APPDATA%\Flare`) uses the user's plain name, so any
/// client of the same user finds it with no environment; any other profile (an e2e run, a second
/// profile of the owner) adds its folder to the name, so it never meets the owner's Flare.
pub fn pipe_for(
    user_data_dir: Option<&std::path::Path>,
    profile_dir: &std::path::Path,
) -> Option<String> {
    let scope = user_data_dir.map(|_| profile_dir.to_string_lossy().into_owned());
    flare_ctl::pipe::user_pipe_name(scope.as_deref())
}

/// `flare-mcp.exe` beside the running exe, when it is there.
pub fn mcp_exe_beside(exe: &std::path::Path) -> Option<String> {
    let path = exe.parent()?.join("flare-mcp.exe");
    path.is_file() // sync-ok: startup, one stat
        .then(|| path.to_string_lossy().into_owned())
}

/// The running pipe server and the flag that makes every waiting call let go.
pub struct CtlServer {
    server: Arc<std::sync::Mutex<Option<flare_ctl::Server>>>,
    stopping: Arc<AtomicBool>,
    dispatcher: Arc<AppDispatcher>,
}

impl CtlServer {
    /// Starts the server on `pipe` (its own threads; the UI thread only makes the pipe). A failure
    /// (the name is taken, no SID) is the caller's to log: Flare runs without the channel.
    pub fn start(
        pipe: &str,
        post: Post,
        tab_of: TabOf,
        home: String,
        eval: EvalSwitch,
        apps: AppsCfg,
        mods: Option<Arc<Hub>>,
    ) -> std::io::Result<Self> {
        let stopping = Arc::new(AtomicBool::new(false));
        let mut d = AppDispatcher::new(post, tab_of, real_resolve(), home, Arc::clone(&stopping))
            .with_eval(eval)
            .with_apps(apps);
        if let Some(hub) = mods {
            d = d.with_mods(hub);
        }
        let dispatcher = Arc::new(d);
        // sync-ok: startup, creates the pipe and its accept thread
        let server = flare_ctl::Server::start(
            pipe,
            flare_ctl::ServerConfig::default(),
            Arc::clone(&dispatcher) as Arc<dyn Dispatcher>,
        )?;
        Ok(Self {
            server: Arc::new(std::sync::Mutex::new(Some(server))),
            stopping,
            dispatcher,
        })
    }

    /// The dispatcher the pipe's calls run on, for the window's own in-process calls (a mod's page talks to
    /// Flare through the same one, with its mod's grant).
    pub fn dispatcher(&self) -> Arc<dyn Dispatcher> {
        Arc::clone(&self.dispatcher) as Arc<dyn Dispatcher>
    }

    /// Where the app's events go: to the clients of the tab they are about. Nothing once stopped.
    pub fn events(&self) -> crate::appctl::CtlEvents {
        let server = Arc::clone(&self.server);
        Arc::new(move |event, params, tab| {
            if let Some(s) = server.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                s.publish(event, params, tab);
            }
        })
    }

    /// Where the events for one plugin's helper go: to the clients that said hello with that plugin's token.
    pub fn owner_events(&self) -> crate::mods_ctl::OwnerEvents {
        let server = Arc::clone(&self.server);
        Arc::new(move |event, params, owner| {
            server
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .map_or(0, |s| s.publish_to_owner(event, params, owner))
        })
    }

    /// Quit: waiting calls answer `unavailable` within a slice, the pipe closes, the workers leave.
    /// Bounded by the server's own stop wait (2 s), inside the quit budget (rule 28). Returns how many
    /// handlers were still running.
    pub fn stop(self) -> usize {
        self.stopping.store(true, Ordering::SeqCst);
        let server = self.server.lock().unwrap_or_else(|e| e.into_inner()).take();
        server.map_or(0, flare_ctl::Server::stop) // sync-ok: quit, bounded by the server's stop wait
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_ctl::Grant;
    use flare_ctl::Permission;
    use std::sync::Mutex;

    /// A dispatcher whose UI thread is a closure: it answers each op at once on the call's own thread
    /// (the post runs inline), or never when `silent`.
    struct Rig {
        d: AppDispatcher,
        ops: Arc<Mutex<Vec<String>>>,
    }

    fn rig(
        answer: impl Fn(&CtlOp) -> Result<Value, CtlError> + Send + Sync + 'static,
        silent: bool,
        files: &'static [(&'static str, &'static str)],
    ) -> Rig {
        let ops = Arc::new(Mutex::new(Vec::new()));
        let seen = ops.clone();
        let post: Post = Arc::new(move |m: UiMsg| {
            let UiMsg::Ctl(CtlRequestBox(req)) = m else {
                return;
            };
            let boxed = CtlRequestBox(req);
            seen.lock().unwrap().push(format!("{boxed:?}"));
            let req = boxed.0;
            if silent {
                // Keep the sender alive so the call waits, as a busy UI thread would.
                std::mem::forget(req.reply);
                return;
            }
            let _ = req.reply.send(answer(&req.op));
        });
        let resolve: Resolve = Arc::new(move |_base, text, _home| {
            files
                .iter()
                .find(|(name, _)| *name == text)
                .map(|(name, real)| ((*name).to_string(), (*real).to_string()))
        });
        let tab_of: TabOf = Arc::new(|t| (t == "secret").then(|| "tab-1".to_string()));
        Rig {
            d: AppDispatcher::new(
                post,
                tab_of,
                resolve,
                "C:\\Users\\x".into(),
                Arc::new(AtomicBool::new(false)),
            ),
            ops,
        }
    }

    fn ctx(tab: Option<&str>, grant: Grant) -> CallContext {
        CallContext {
            conn: 1,
            client: "t".into(),
            client_version: "1".into(),
            grant,
            tab: tab.map(str::to_string),
        }
    }

    fn agent_in_tab() -> CallContext {
        ctx(Some("tab-1"), Grant::agent())
    }

    fn ok_all(op: &CtlOp) -> Result<Value, CtlError> {
        Ok(match op {
            CtlOp::TabCwd { .. } => json!({"cwd": "C:\\work"}),
            _ => json!({"panel": {"kind": "image"}}),
        })
    }

    const FILES: &[(&str, &str)] = &[
        ("C:\\work\\a.png", "C:\\work\\a.png"),
        ("C:\\work\\a.txt", "C:\\work\\a.txt"),
        ("C:\\work\\link.png", "C:\\work\\run.txt"),
        ("C:\\work\\doc.pdf", "C:\\work\\doc.pdf"),
        ("C:\\work\\sneaky.png", "C:\\work\\sneaky.exe"),
    ];

    /// The review (2026-10-24): a picture on a mapped network drive was refused as "not a picture; show_file
    /// shows it", and show_file sent the caller back.
    #[test]
    fn a_file_on_a_network_drive_is_not_found_by_either_tool() {
        static FILES: &[(&str, &str)] = &[
            ("Z:\\a.png", "UNC\\server\\share\\a.png"),
            ("Z:\\a.txt", "UNC\\server\\share\\a.txt"),
        ];
        let r = rig(ok_all, false, FILES);
        for (m, p) in [
            (Method::ViewImage, "Z:\\a.png"),
            (Method::ViewFile, "Z:\\a.txt"),
        ] {
            let got = r.d.call(&agent_in_tab(), m, json!({ "path": p }));
            assert_eq!(code_of(got), code::NOT_FOUND, "{p}");
        }
    }

    fn code_of(r: Result<Value, CtlError>) -> String {
        r.expect_err("an error").code
    }

    /// The ops the UI was asked for, with the address each carried.
    fn browser_rig() -> (Rig, Arc<Mutex<Vec<String>>>) {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let a = asked.clone();
        let r = rig(
            move |op| {
                match op {
                    CtlOp::BrowserOpen { tab, url } => {
                        a.lock().unwrap().push(format!("open {tab} {url}"));
                    }
                    CtlOp::BrowserNavigate { tab, to } => {
                        a.lock().unwrap().push(format!("navigate {tab} {to:?}"));
                    }
                    _ => {}
                }
                Ok(json!({"url": "x"}))
            },
            false,
            FILES,
        );
        (r, asked)
    }

    #[test]
    fn browser_open_vets_the_address_before_the_ui_hears_of_it() {
        let (r, asked) = browser_rig();
        for bad in [
            "file:///C:/Windows/win.ini",
            "javascript:alert(1)",
            "data:text/html,x",
            "ftp://a.example/",
            "https://user:pw@a.example/",
            "two words",
            "",
        ] {
            let e =
                r.d.call(&agent_in_tab(), Method::BrowserOpen, json!({"url": bad}))
                    .expect_err(bad);
            assert_eq!(e.code, code::BAD_PARAMS, "{bad:?}");
        }
        for (given, want) in [
            ("example.com", "https://example.com/"),
            ("localhost:3000/app", "http://localhost:3000/app"),
            ("HTTP://Example.COM/a", "http://example.com/a"),
            ("about:blank", "about:blank"),
        ] {
            r.d.call(&agent_in_tab(), Method::BrowserOpen, json!({"url": given}))
                .unwrap();
            assert_eq!(
                asked.lock().unwrap().pop().unwrap(),
                format!("open tab-1 {want}")
            );
        }
        assert!(
            asked.lock().unwrap().is_empty(),
            "the refused ones asked nothing"
        );
        for p in [json!({}), json!({"url": 5}), json!({"url": null})] {
            let e =
                r.d.call(&agent_in_tab(), Method::BrowserOpen, p.clone())
                    .expect_err("no url");
            assert_eq!(e.code, code::BAD_PARAMS, "{p}");
        }
    }

    #[test]
    fn browser_navigate_takes_a_url_or_an_action() {
        let (r, asked) = browser_rig();
        r.d.call(
            &agent_in_tab(),
            Method::BrowserNavigate,
            json!({"url": "a.example/x"}),
        )
        .unwrap();
        for action in ["back", "forward", "reload"] {
            r.d.call(
                &agent_in_tab(),
                Method::BrowserNavigate,
                json!({"action": action}),
            )
            .unwrap();
        }
        assert_eq!(
            asked.lock().unwrap().as_slice(),
            [
                "navigate tab-1 Url(\"https://a.example/x\")",
                "navigate tab-1 Back",
                "navigate tab-1 Forward",
                "navigate tab-1 Reload"
            ]
        );
        let before = asked.lock().unwrap().len();
        for p in [
            json!({}),
            json!({"url": "a.example", "action": "back"}),
            json!({"action": "sideways"}),
            json!({"action": 3}),
            json!({"url": "file:///C:/x"}),
        ] {
            let e =
                r.d.call(&agent_in_tab(), Method::BrowserNavigate, p.clone())
                    .expect_err("refused");
            assert_eq!(e.code, code::BAD_PARAMS, "{p}");
        }
        assert_eq!(asked.lock().unwrap().len(), before, "none reached the UI");
    }

    #[test]
    fn panel_open_web_opens_a_page_and_anything_else_is_refused() {
        let (r, asked) = browser_rig();
        r.d.call(&agent_in_tab(), Method::PanelOpen, json!({"kind": "web"}))
            .unwrap();
        r.d.call(
            &agent_in_tab(),
            Method::PanelOpen,
            json!({"kind": "web", "url": "https://a.example/"}),
        )
        .unwrap();
        assert_eq!(
            asked.lock().unwrap().as_slice(),
            ["open tab-1 about:blank", "open tab-1 https://a.example/"]
        );
        for p in [
            json!({}),
            json!({"kind": "image"}),
            json!({"kind": "web", "url": "file:///C:/x"}),
        ] {
            let e =
                r.d.call(&agent_in_tab(), Method::PanelOpen, p.clone())
                    .expect_err("refused");
            assert_eq!(e.code, code::BAD_PARAMS, "{p}");
        }
    }

    #[test]
    fn mods_call_ends_before_the_channels_own_timeout_whatever_the_window_took() {
        use flare_ctl::method::MOD_CALL_TIMEOUT;
        for ms in (0..=MOD_UI_WAIT.as_millis() as u64).step_by(250) {
            let spent = Duration::from_millis(ms);
            let left = mod_wait_left(spent);
            assert!(left <= flare_mods::hub::CALL_TIMEOUT, "{ms}");
            assert!(
                spent + left <= MOD_CALL_TIMEOUT - MOD_REPLY_MARGIN,
                "{ms}: the call would outlive the channel's wait"
            );
        }
        // Even a call that somehow spent the whole channel wait has nothing left, never a wrapped value.
        assert_eq!(mod_wait_left(MOD_CALL_TIMEOUT), Duration::ZERO);
    }

    #[test]
    fn a_browser_call_waits_longer_than_a_view_call() {
        assert!(BROWSER_WAIT > UI_WAIT);
        assert!(BROWSER_WAIT < Method::BrowserOpen.timeout());
    }

    #[test]
    fn the_token_decides_the_tab() {
        let r = rig(ok_all, false, FILES);
        assert_eq!(r.d.resolve_tab_token("secret").as_deref(), Some("tab-1"));
        assert_eq!(r.d.resolve_tab_token("tab-1"), None);
        assert_eq!(r.d.resolve_tab_token(""), None);
        // No mod has a token yet.
        assert!(r.d.resolve_token("anything").is_none());
    }

    #[test]
    fn what_this_build_can_do_is_listed_and_the_grants_still_apply() {
        let r = rig(ok_all, false, FILES);
        let caps = r.d.capabilities();
        for m in [
            Method::TabsList,
            Method::PanelOpen,
            Method::PanelClose,
            Method::PanelState,
            Method::ViewImage,
            Method::ViewFile,
            Method::ViewText,
            Method::BrowserOpen,
            Method::BrowserNavigate,
        ] {
            assert!(caps.contains(&m), "{}", m.name());
        }
        // The agent's hands on the page (stage 4) and the conversation summary (stage 6) are built.
        for m in [
            Method::BrowserLook,
            Method::BrowserClick,
            Method::BrowserType,
            Method::BrowserPress,
            Method::BrowserScroll,
            Method::BrowserWait,
            Method::BrowserEval,
        ] {
            assert!(caps.contains(&m), "{}", m.name());
        }
        assert!(caps.contains(&Method::ConvoState));
        // Badges and progress on the caller's own tab (plugins phase 1).
        for m in [
            Method::TabsBadgeSet,
            Method::TabsBadgeClear,
            Method::TabsProgressSet,
            Method::TabsProgressClear,
        ] {
            assert!(caps.contains(&m), "{}", m.name());
        }
        assert!(
            !caps.contains(&Method::ToolsRespond),
            "mods come in stage 8"
        );
    }

    #[test]
    fn the_agent_grant_follows_the_users_switch_for_scripts() {
        let on = Arc::new(AtomicBool::new(false));
        let flag = on.clone();
        let r = rig(ok_all, false, FILES);
        let d = r.d.with_eval(Arc::new(move || flag.load(Ordering::SeqCst)));
        assert_eq!(d.agent_grant(), Grant::Agent { eval: false });
        assert!(!d.agent_grant().may_call(Method::BrowserEval));
        on.store(true, Ordering::SeqCst);
        assert_eq!(d.agent_grant(), Grant::Agent { eval: true });
        assert!(d.agent_grant().may_call(Method::BrowserEval));
        // Whatever the switch says, the agent never types into a shell or changes a setting (S3).
        for m in [Method::ShellInput, Method::SettingsSet, Method::ModsEnable] {
            assert!(!d.agent_grant().may_call(m));
        }
        // And the rest of the browser is the agent's with the switch off too.
        on.store(false, Ordering::SeqCst);
        for m in [
            Method::BrowserLook,
            Method::BrowserClick,
            Method::BrowserType,
            Method::BrowserPress,
            Method::BrowserScroll,
            Method::BrowserWait,
        ] {
            assert!(d.agent_grant().may_call(m), "{}", m.name());
        }
    }

    #[test]
    fn an_agent_call_is_posted_for_the_callers_own_tab_only() {
        let r = rig(ok_all, false, FILES);
        let c = agent_in_tab();
        for (m, p) in [
            (Method::BrowserLook, json!({})),
            (Method::BrowserClick, json!({"ref": "e1"})),
            (Method::BrowserType, json!({"text": "hi", "ref": "e2"})),
            (Method::BrowserPress, json!({"key": "Enter"})),
            (Method::BrowserScroll, json!({"dy": 100})),
            (Method::BrowserWait, json!({"text": "Done"})),
            (Method::BrowserEval, json!({"expression": "1"})),
        ] {
            // A `tab` in the params is never read: the tab is the connection's.
            let mut p = p;
            p["tab"] = json!("tab-9");
            assert!(r.d.call(&c, m, p).is_ok(), "{}", m.name());
        }
        let seen = r.ops.lock().unwrap().clone();
        assert_eq!(seen.len(), 7);
        assert!(seen.iter().all(|s| s == "CtlOp::BrowserAgent"));
        // No tab, no call, and the UI is not asked.
        let none = ctx(None, Grant::agent());
        for m in [
            Method::BrowserLook,
            Method::BrowserClick,
            Method::BrowserWait,
        ] {
            assert_eq!(
                code_of(r.d.call(&none, m, json!({"ref": "e1", "text": "x"}))),
                code::NO_TAB
            );
        }
        assert_eq!(r.ops.lock().unwrap().len(), 7);
    }

    /// One instant for the whole test: the decor limit never refills.
    fn still() -> Instant {
        static AT: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        *AT.get_or_init(Instant::now)
    }

    /// What a rig saw, one line per op.
    type Log = Arc<Mutex<Vec<String>>>;

    /// A rig whose window records what each decor op carried.
    fn decor_rig() -> (AppDispatcher, Log, Log) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        let r = rig(
            move |op| {
                let line = match op {
                    CtlOp::BadgeSet {
                        tab,
                        source,
                        key,
                        icon,
                        tip,
                        tone,
                        ..
                    } => format!("badge {tab} {source:?} {key} {icon} {tip:?} {tone:?}"),
                    CtlOp::BadgeClear { tab, source, key } => {
                        format!("clear {tab} {source:?} {key}")
                    }
                    CtlOp::ProgressSet {
                        tab,
                        source,
                        key,
                        value,
                        label,
                        ..
                    } => format!("progress {tab} {source:?} {key} {value:?} {label:?}"),
                    CtlOp::ProgressClear { tab, source, key } => {
                        format!("unprogress {tab} {source:?} {key}")
                    }
                    _ => "other".to_string(),
                };
                s.lock().unwrap().push(line);
                Ok(json!({}))
            },
            false,
            FILES,
        );
        (r.d.with_clock(still), r.ops, seen)
    }

    #[test]
    fn decor_calls_are_posted_for_the_callers_own_tab_with_what_they_carry() {
        let (d, ops, seen) = decor_rig();
        let c = agent_in_tab();
        let calls = [
            (
                Method::TabsBadgeSet,
                json!({"key": "call-me", "icon": "phone", "tip": "Call-me is on"}),
            ),
            // The own tab named outright is the same tab; no tip is an empty one; a tone is taken.
            (
                Method::TabsBadgeSet,
                json!({"tab": "tab-1", "key": "x", "icon": "globe", "tone": "accent", "tip": ""}),
            ),
            (Method::TabsBadgeClear, json!({"key": "x"})),
            (
                Method::TabsProgressSet,
                json!({"key": "progress", "value": 0.25, "label": "Review"}),
            ),
            (Method::TabsProgressSet, json!({"key": "progress"})),
            (Method::TabsProgressClear, json!({"key": "progress"})),
        ];
        for (m, p) in calls {
            assert!(d.call(&c, m, p.clone()).is_ok(), "{} {p}", m.name());
        }
        assert_eq!(
            *seen.lock().unwrap(),
            [
                "badge tab-1 Agent call-me phone \"Call-me is on\" Ink",
                "badge tab-1 Agent x globe \"\" Accent",
                "clear tab-1 Agent x",
                "progress tab-1 Agent progress Some(0.25) \"Review\"",
                "progress tab-1 Agent progress None \"\"",
                "unprogress tab-1 Agent progress",
            ]
        );
        assert_eq!(
            *ops.lock().unwrap(),
            [
                "CtlOp::BadgeSet",
                "CtlOp::BadgeSet",
                "CtlOp::BadgeClear",
                "CtlOp::ProgressSet",
                "CtlOp::ProgressSet",
                "CtlOp::ProgressClear",
            ]
        );
        // A mod's token is its own source (no mod may hold `decorate` yet; the dispatcher is ready for one).
        let m = ctx(
            Some("tab-1"),
            Grant::Mod {
                id: "todo".into(),
                permissions: vec![Permission::Decorate],
                plugin: true,
            },
        );
        d.call(
            &m,
            Method::TabsBadgeSet,
            json!({"key": "a", "icon": "star"}),
        )
        .unwrap();
        assert!(
            seen.lock()
                .unwrap()
                .last()
                .unwrap()
                .contains("Plugin(\"todo\")")
        );
    }

    #[test]
    fn a_decor_call_for_another_tab_or_no_tab_never_reaches_the_window() {
        let (d, ops, _) = decor_rig();
        let c = agent_in_tab();
        for m in [
            Method::TabsBadgeSet,
            Method::TabsBadgeClear,
            Method::TabsProgressSet,
            Method::TabsProgressClear,
        ] {
            let p = json!({"tab": "tab-9", "key": "a", "icon": "phone"});
            assert_eq!(code_of(d.call(&c, m, p)), code::FORBIDDEN, "{}", m.name());
            let none = ctx(None, Grant::agent());
            assert_eq!(
                code_of(d.call(&none, m, json!({"key": "a", "icon": "phone"}))),
                code::NO_TAB,
                "{}",
                m.name()
            );
        }
        assert!(ops.lock().unwrap().is_empty());
    }

    #[test]
    fn bad_keys_icons_tones_tips_and_values_are_named_on_the_worker() {
        let (d, ops, _) = decor_rig();
        let c = agent_in_tab();
        let long = "a".repeat(flare_ctl::decor::KEY_MAX + 1);
        let long_tip = "t".repeat(flare_ctl::decor::TIP_MAX + 1);
        let bad = [
            (
                Method::TabsBadgeSet,
                json!({"icon": "phone"}),
                "key is required",
            ),
            (
                Method::TabsBadgeSet,
                json!({"key": "Call Me", "icon": "phone"}),
                "key is 1 to 24",
            ),
            (
                Method::TabsBadgeSet,
                json!({"key": "", "icon": "phone"}),
                "key is 1 to 24",
            ),
            (
                Method::TabsBadgeSet,
                json!({"key": long, "icon": "phone"}),
                "key is 1 to 24",
            ),
            (
                Method::TabsBadgeSet,
                json!({"key": 7, "icon": "phone"}),
                "key must be a string",
            ),
            (Method::TabsBadgeClear, json!({}), "key is required"),
            (
                Method::TabsBadgeSet,
                json!({"key": "a"}),
                "icon is required",
            ),
            // No check and no dot: they would read as the agent's own marks.
            (
                Method::TabsBadgeSet,
                json!({"key": "a", "icon": "check"}),
                "icon is one of",
            ),
            (
                Method::TabsBadgeSet,
                json!({"key": "a", "icon": "dot"}),
                "icon is one of",
            ),
            (
                Method::TabsBadgeSet,
                json!({"key": "a", "icon": "C:\\x.png"}),
                "icon is one of",
            ),
            (
                Method::TabsBadgeSet,
                json!({"key": "a", "icon": "phone", "tone": "red"}),
                "tone is ink",
            ),
            (
                Method::TabsBadgeSet,
                json!({"key": "a", "icon": "mic", "tone": "accent"}),
                "dictation mark",
            ),
            (
                Method::TabsBadgeSet,
                json!({"key": "a", "icon": "phone", "tip": "two\nlines"}),
                "control character",
            ),
            (
                Method::TabsBadgeSet,
                json!({"key": "a", "icon": "phone", "tip": "a\u{202e}b"}),
                "hidden formatting",
            ),
            (
                Method::TabsBadgeSet,
                json!({"key": "a", "icon": "phone", "tip": "   "}),
                "must not be empty",
            ),
            (
                Method::TabsBadgeSet,
                json!({"key": "a", "icon": "phone", "tip": long_tip}),
                "longer than 80",
            ),
            (
                Method::TabsProgressSet,
                json!({"key": "p", "value": 1.5}),
                "value is 0 to 1",
            ),
            (
                Method::TabsProgressSet,
                json!({"key": "p", "value": -0.1}),
                "value is 0 to 1",
            ),
            (
                Method::TabsProgressSet,
                json!({"key": "p", "value": "half"}),
                "value must be a number",
            ),
            (
                Method::TabsProgressSet,
                json!({"key": "p", "label": "x\u{200b}"}),
                "hidden formatting",
            ),
            (
                Method::TabsProgressSet,
                json!({"value": 0.5}),
                "key is required",
            ),
        ];
        for (m, p, why) in bad {
            let e = d.call(&c, m, p.clone()).unwrap_err();
            assert_eq!(e.code, code::BAD_PARAMS, "{} {p}", m.name());
            assert!(e.message.contains(why), "{p}: {}", e.message);
        }
        assert!(ops.lock().unwrap().is_empty(), "the window heard nothing");
        // The edges are fine.
        for v in [0.0, 1.0] {
            d.call(&c, Method::TabsProgressSet, json!({"key": "p", "value": v}))
                .unwrap();
        }
    }

    #[test]
    fn ten_decor_changes_a_second_per_tab_and_the_rest_are_rate_limited_before_posting() {
        let (d, ops, _) = decor_rig();
        let c = agent_in_tab();
        // Wrong calls spend nothing.
        for _ in 0..20 {
            let _ = d.call(
                &c,
                Method::TabsBadgeSet,
                json!({"key": "a", "icon": "check"}),
            );
        }
        for i in 0..10 {
            let m = if i % 2 == 0 {
                Method::TabsBadgeSet
            } else {
                Method::TabsProgressSet
            };
            d.call(&c, m, json!({"key": "a", "icon": "star", "value": 0.1}))
                .unwrap_or_else(|e| panic!("call {i}: {e:?}"));
        }
        let e = d
            .call(&c, Method::TabsBadgeClear, json!({"key": "a"}))
            .unwrap_err();
        assert_eq!(e.code, code::RATE_LIMITED);
        assert!(e.message.contains("10"), "{}", e.message);
        assert_eq!(
            ops.lock().unwrap().len(),
            10,
            "the eleventh was never posted"
        );
        // Another tab, and another caller on the same tab, have their own second.
        let other = ctx(Some("tab-2"), Grant::agent());
        d.call(&other, Method::TabsBadgeClear, json!({"key": "a"}))
            .unwrap();
        let m = ctx(
            Some("tab-1"),
            Grant::Mod {
                id: "todo".into(),
                permissions: vec![Permission::Decorate],
                plugin: true,
            },
        );
        d.call(&m, Method::TabsBadgeClear, json!({"key": "a"}))
            .unwrap();
    }

    fn plugin(tab: Option<&str>, perms: &[Permission]) -> CallContext {
        ctx(
            tab,
            Grant::Mod {
                id: "badger".into(),
                permissions: perms.to_vec(),
                plugin: true,
            },
        )
    }

    /// The tab scope (plugins phase 3): `tabs` lets a plugin name any tab; without it a caller names its own or none,
    /// and a background page (no tab of its own) can name none.
    #[test]
    fn a_plugin_with_tabs_may_name_any_tab_and_every_other_caller_its_own_only() {
        let (d, ops, seen) = decor_rig();
        let star = |tab: Option<&str>| match tab {
            Some(t) => json!({"tab": t, "key": "a", "icon": "star"}),
            None => json!({"key": "a", "icon": "star"}),
        };
        let wide = plugin(Some("tab-1"), &[Permission::Decorate, Permission::Tabs]);
        d.call(&wide, Method::TabsBadgeSet, star(Some("tab-9")))
            .unwrap();
        // A background page (no tab) with `tabs` names one; naming none is no tab.
        let bg = plugin(None, &[Permission::Decorate, Permission::Tabs]);
        d.call(&bg, Method::TabsBadgeSet, star(Some("tab-7")))
            .unwrap();
        assert_eq!(
            code_of(d.call(&bg, Method::TabsBadgeSet, star(None))),
            code::NO_TAB
        );
        assert!(seen.lock().unwrap()[0].starts_with("badge tab-9 Plugin(\"badger\")"));
        assert!(seen.lock().unwrap()[1].starts_with("badge tab-7 "));
        // Without `tabs`: its own tab only, and from a background page none at all.
        let narrow = plugin(Some("tab-1"), &[Permission::Decorate]);
        assert_eq!(
            code_of(d.call(&narrow, Method::TabsBadgeSet, star(Some("tab-9")))),
            code::FORBIDDEN
        );
        d.call(&narrow, Method::TabsBadgeSet, star(Some("tab-1")))
            .unwrap();
        let narrow_bg = plugin(None, &[Permission::Decorate]);
        for t in [Some("tab-1"), None] {
            let want = if t.is_some() {
                code::FORBIDDEN
            } else {
                code::NO_TAB
            };
            assert_eq!(
                code_of(d.call(&narrow_bg, Method::TabsBadgeSet, star(t))),
                want
            );
        }
        // An id that is no id is refused before the window is asked.
        assert_eq!(
            code_of(d.call(&wide, Method::TabsBadgeSet, star(Some("")))),
            code::BAD_PARAMS
        );
        assert_eq!(ops.lock().unwrap().len(), 3);
        // The agent keeps to its own tab whatever it names.
        assert_eq!(
            code_of(d.call(&agent_in_tab(), Method::TabsBadgeSet, star(Some("tab-9")))),
            code::FORBIDDEN
        );
    }

    #[test]
    fn a_plugin_makes_thirty_decor_changes_a_second_across_all_its_tabs() {
        let (d, ops, _) = decor_rig();
        let wide = plugin(Some("tab-1"), &[Permission::Decorate, Permission::Tabs]);
        // Thirty tabs, one change each: each tab's own second is far from spent, the plugin's is not.
        for i in 0..30 {
            d.call(
                &wide,
                Method::TabsBadgeClear,
                json!({"tab": format!("tab-{i}"), "key": "a"}),
            )
            .unwrap_or_else(|e| panic!("change {i}: {e:?}"));
        }
        let e = d
            .call(
                &wide,
                Method::TabsBadgeClear,
                json!({"tab": "tab-99", "key": "a"}),
            )
            .unwrap_err();
        assert_eq!(e.code, code::RATE_LIMITED);
        assert!(e.message.contains("one plugin"), "{}", e.message);
        assert_eq!(ops.lock().unwrap().len(), 30);
        // The agent is not a plugin: only its tab's second holds it.
        d.call(&agent_in_tab(), Method::TabsBadgeClear, json!({"key": "a"}))
            .unwrap();
    }

    #[test]
    fn a_plugin_with_tabs_gets_the_wide_list_and_every_other_caller_its_own_tab() {
        let r = rig(ok_all, false, FILES);
        let wide = plugin(None, &[Permission::Tabs]);
        r.d.call(&wide, Method::TabsList, json!({})).unwrap();
        r.d.call(
            &plugin(Some("tab-1"), &[Permission::Tabs]),
            Method::TabsList,
            json!({}),
        )
        .unwrap();
        r.d.call(&plugin(Some("tab-1"), &[]), Method::TabsList, json!({}))
            .unwrap();
        r.d.call(&agent_in_tab(), Method::TabsList, json!({}))
            .unwrap();
        // A background page without `tabs` has no tab: nothing to list, nothing asked.
        let v =
            r.d.call(
                &plugin(None, &[Permission::View]),
                Method::TabsList,
                json!({}),
            )
            .unwrap();
        assert_eq!(v, json!({"tabs": []}));
        assert_eq!(
            r.ops.lock().unwrap().as_slice(),
            [
                "CtlOp::TabsWide",
                "CtlOp::TabsWide",
                "CtlOp::Tabs",
                "CtlOp::Tabs"
            ]
        );
    }

    #[test]
    fn a_plugin_with_tabs_may_open_its_own_panel_on_a_tab_it_names_and_nothing_else() {
        let r = rig(ok_all, false, FILES);
        let wide = plugin(None, &[Permission::Panel, Permission::Tabs]);
        // There is no hub in this rig: the call gets as far as the mods and stops there, past the scope.
        let e =
            r.d.call(
                &wide,
                Method::PanelOpen,
                json!({"kind": "mod:badger", "tab": "tab-3"}),
            )
            .unwrap_err();
        assert_eq!(e.code, code::UNAVAILABLE);
        for (p, want) in [
            (
                json!({"kind": "mod:other", "tab": "tab-3"}),
                code::FORBIDDEN,
            ),
            (json!({"kind": "web", "tab": "tab-3"}), code::NO_TAB),
        ] {
            assert_eq!(
                code_of(r.d.call(&wide, Method::PanelOpen, p.clone())),
                want,
                "{p}"
            );
        }
        let narrow = plugin(None, &[Permission::Panel]);
        assert_eq!(
            code_of(r.d.call(
                &narrow,
                Method::PanelOpen,
                json!({"kind": "mod:badger", "tab": "tab-3"})
            )),
            code::FORBIDDEN
        );
        assert!(r.ops.lock().unwrap().is_empty());
    }

    fn parsed(m: Method, p: Value) -> Result<(AgentOp, Duration), CtlError> {
        agent_op(m, &p)
    }

    #[test]
    fn click_takes_a_ref_or_a_point_and_nothing_else() {
        let ok = parsed(Method::BrowserClick, json!({"ref": "e12"}))
            .unwrap()
            .0;
        assert_eq!(
            ok,
            AgentOp::Click {
                target: Target::Ref("e12".into()),
                button: Button::Left,
                double: false
            }
        );
        let ok = parsed(
            Method::BrowserClick,
            json!({"x": 10, "y": 20.5, "button": "right", "double": true}),
        )
        .unwrap()
        .0;
        assert_eq!(
            ok,
            AgentOp::Click {
                target: Target::Point(10.0, 20.5),
                button: Button::Right,
                double: true
            }
        );
        for bad in [
            json!({}),
            json!({"x": 1}),
            json!({"ref": "e1", "x": 1, "y": 2}),
            json!({"ref": "e 1"}),
            json!({"ref": ""}),
            json!({"ref": "e1", "button": "back"}),
            json!({"ref": "e1", "double": "yes"}),
            json!({"x": "1", "y": 2}),
            json!({"ref": 7}),
        ] {
            assert_eq!(
                parsed(Method::BrowserClick, bad.clone()).unwrap_err().code,
                code::BAD_PARAMS,
                "{bad}"
            );
        }
    }

    #[test]
    fn type_press_and_scroll_check_their_params() {
        assert!(parsed(Method::BrowserType, json!({"text": "hello"})).is_ok());
        assert!(parsed(Method::BrowserType, json!({"text": ""})).is_ok());
        for bad in [
            json!({}),
            json!({"text": 5}),
            json!({"text": "x", "ref": "!"}),
        ] {
            assert_eq!(
                parsed(Method::BrowserType, bad).unwrap_err().code,
                code::BAD_PARAMS
            );
        }
        let long = "x".repeat(agent_web::TYPE_MAX + 1);
        assert!(parsed(Method::BrowserType, json!({ "text": long })).is_err());

        let k = parsed(
            Method::BrowserPress,
            json!({"key": "a", "modifiers": ["Control", "Shift"]}),
        )
        .unwrap()
        .0;
        let AgentOp::Press { key } = k else {
            panic!("{k:?}")
        };
        assert_eq!((key.key.as_str(), key.modifiers), ("A", 10));
        assert!(parsed(Method::BrowserPress, json!({"key": "Enter"})).is_ok());
        for bad in [
            json!({}),
            json!({"key": "Banana"}),
            json!({"key": "a", "modifiers": "Control"}),
            json!({"key": "a", "modifiers": ["Hyper"]}),
            json!({"key": "a", "modifiers": [3]}),
        ] {
            assert_eq!(
                parsed(Method::BrowserPress, bad).unwrap_err().code,
                code::BAD_PARAMS
            );
        }

        assert!(parsed(Method::BrowserScroll, json!({"dy": 300})).is_ok());
        assert!(parsed(Method::BrowserScroll, json!({"dx": -5, "ref": "e3"})).is_ok());
        for bad in [
            json!({}),
            json!({"dx": 0, "dy": 0}),
            json!({"dy": 1e9}),
            json!({"dy": "5"}),
        ] {
            assert_eq!(
                parsed(Method::BrowserScroll, bad).unwrap_err().code,
                code::BAD_PARAMS
            );
        }
    }

    #[test]
    fn a_wait_is_one_thing_for_at_most_thirty_seconds() {
        let (op, wait) = parsed(Method::BrowserWait, json!({"text": "Done"})).unwrap();
        assert_eq!(
            op,
            AgentOp::Wait {
                what: WaitFor::Text("Done".into()),
                timeout_ms: agent_web::WAIT_DEFAULT_MS
            }
        );
        assert!(wait > Duration::from_millis(agent_web::WAIT_DEFAULT_MS));
        assert!(wait < Duration::from_secs(60), "inside the channel's 60 s");
        let (op, _) = parsed(
            Method::BrowserWait,
            json!({"selector": "#go", "timeout_ms": 999_999}),
        )
        .unwrap();
        assert_eq!(
            op,
            AgentOp::Wait {
                what: WaitFor::Selector("#go".into()),
                timeout_ms: agent_web::WAIT_MAX_MS
            }
        );
        let (op, wait) = parsed(Method::BrowserWait, json!({"ms": 120_000})).unwrap();
        assert_eq!(
            op,
            AgentOp::Wait {
                what: WaitFor::Ms(agent_web::WAIT_MAX_MS),
                timeout_ms: agent_web::WAIT_MAX_MS
            }
        );
        assert!(wait < Duration::from_secs(60));
        for bad in [
            json!({}),
            json!({"text": "a", "ms": 5}),
            json!({"text": "a", "selector": "b"}),
            json!({"text": ""}),
            json!({"selector": ""}),
            json!({"ms": -1}),
            json!({"text": "a", "timeout_ms": -5}),
            json!({"text": "x".repeat(501)}),
        ] {
            assert_eq!(
                parsed(Method::BrowserWait, bad.clone()).unwrap_err().code,
                code::BAD_PARAMS,
                "{bad}"
            );
        }
    }

    #[test]
    fn eval_needs_an_expression() {
        assert!(parsed(Method::BrowserEval, json!({"expression": "document.title"})).is_ok());
        for bad in [
            json!({}),
            json!({"expression": "  "}),
            json!({"expression": 5}),
            json!({"expression": "x".repeat(agent_web::EVAL_MAX + 1)}),
        ] {
            assert_eq!(
                parsed(Method::BrowserEval, bad).unwrap_err().code,
                code::BAD_PARAMS
            );
        }
    }

    #[test]
    fn a_call_with_no_tab_is_no_tab_and_asks_the_ui_nothing() {
        let r = rig(ok_all, false, FILES);
        let c = ctx(None, Grant::agent());
        for (m, p) in [
            (Method::PanelState, json!({})),
            (Method::PanelClose, json!({})),
            (Method::ViewImage, json!({"path": "C:\\work\\a.png"})),
            (Method::ViewFile, json!({"path": "C:\\work\\a.txt"})),
            (Method::ViewText, json!({"text": "x"})),
            (Method::ViewHtml, json!({"html": "<p>x"})),
            (Method::ViewDiagram, json!({"mermaid": "graph TD\nA-->B"})),
            (Method::BrowserOpen, json!({"url": "https://a.example/"})),
            (Method::BrowserNavigate, json!({"action": "reload"})),
            (Method::PanelOpen, json!({"kind": "web"})),
            (Method::BrowserLook, json!({})),
            (Method::BrowserClick, json!({"ref": "e1"})),
            (Method::BrowserEval, json!({"expression": "1"})),
        ] {
            assert_eq!(code_of(r.d.call(&c, m, p)), code::NO_TAB, "{}", m.name());
        }
        assert!(r.ops.lock().unwrap().is_empty());
    }

    #[test]
    fn tabs_list_gives_the_agent_its_own_tab_only() {
        let r = rig(ok_all, false, FILES);
        r.d.call(&agent_in_tab(), Method::TabsList, json!({}))
            .unwrap();
        assert_eq!(r.ops.lock().unwrap().as_slice(), ["CtlOp::Tabs"]);
        // The asked op carries the caller's own tab and nothing wider.
        let own = Arc::new(Mutex::new(None));
        let o = own.clone();
        let r = rig(
            move |op| {
                if let CtlOp::Tabs { own } = op {
                    *o.lock().unwrap() = Some(own.clone());
                }
                Ok(json!({"tabs": []}))
            },
            false,
            FILES,
        );
        r.d.call(&agent_in_tab(), Method::TabsList, json!({}))
            .unwrap();
        assert_eq!(*own.lock().unwrap(), Some(Some("tab-1".to_string())));
        // No tab: nothing to list, and the UI is not asked.
        let r = rig(ok_all, false, FILES);
        let v =
            r.d.call(&ctx(None, Grant::agent()), Method::TabsList, json!({}))
                .unwrap();
        assert_eq!(v, json!({"tabs": []}));
        assert!(r.ops.lock().unwrap().is_empty());
    }

    #[test]
    fn a_mod_sees_its_own_tab_only_whatever_it_may_do() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        let r = rig(
            move |op| {
                if let CtlOp::Tabs { own } = op {
                    s.lock().unwrap().push(own.clone());
                }
                Ok(json!({"tabs": []}))
            },
            false,
            FILES,
        );
        let m = |p: Vec<Permission>| {
            ctx(
                Some("tab-1"),
                Grant::Mod {
                    id: "m".into(),
                    permissions: p,
                    plugin: true,
                },
            )
        };
        r.d.call(&m(vec![Permission::Panel]), Method::TabsList, json!({}))
            .unwrap();
        r.d.call(&m(vec![Permission::View]), Method::TabsList, json!({}))
            .unwrap();
        assert_eq!(
            *seen.lock().unwrap(),
            [Some("tab-1".to_string()), Some("tab-1".to_string())],
            "the panel permission does not widen the list"
        );
    }

    #[test]
    fn view_image_resolves_vets_and_then_shows_on_the_callers_tab() {
        let shown = Arc::new(Mutex::new(Vec::new()));
        let s = shown.clone();
        let r = rig(
            move |op| match op {
                CtlOp::TabCwd { tab } => {
                    assert_eq!(tab, "tab-1");
                    Ok(json!({"cwd": "C:\\work"}))
                }
                CtlOp::ShowFile {
                    tab,
                    kind,
                    abs,
                    title,
                } => {
                    s.lock()
                        .unwrap()
                        .push((tab.clone(), kind.clone(), abs.clone(), title.clone()));
                    Ok(json!({"panel": {"kind": "image"}}))
                }
                _ => unreachable!(),
            },
            false,
            FILES,
        );
        let v =
            r.d.call(
                &agent_in_tab(),
                Method::ViewImage,
                json!({"path": "C:\\work\\a.png", "title": "  A\ncat  "}),
            )
            .unwrap();
        assert_eq!(v["panel"]["kind"], "image");
        assert_eq!(
            shown.lock().unwrap().as_slice(),
            [(
                "tab-1".to_string(),
                PanelKind::Image,
                "C:\\work\\a.png".to_string(),
                Some("A cat".to_string())
            )]
        );
    }

    #[test]
    fn a_path_nobody_may_open_is_not_found_and_never_reaches_the_ui_thread() {
        let r = rig(ok_all, false, FILES);
        for p in [
            "C:\\work\\missing.png",
            "\\\\server\\share\\a.png",
            "C:\\x.exe",
        ] {
            let e =
                r.d.call(&agent_in_tab(), Method::ViewImage, json!({"path": p}))
                    .unwrap_err();
            assert_eq!(e.code, code::NOT_FOUND, "{p}");
        }
        let ops = r.ops.lock().unwrap();
        assert!(ops.iter().all(|o| o == "CtlOp::TabCwd"), "{ops:?}");
    }

    #[test]
    fn the_kind_must_match_and_a_link_that_changes_kind_shows_nothing() {
        let r = rig(ok_all, false, FILES);
        let call = |m, p: &str| r.d.call(&agent_in_tab(), m, json!({"path": p}));
        // A picture through view.file, a text file through view.image, a pdf through either.
        assert_eq!(
            code_of(call(Method::ViewFile, "C:\\work\\a.png")),
            code::BAD_PARAMS
        );
        assert_eq!(
            code_of(call(Method::ViewImage, "C:\\work\\a.txt")),
            code::BAD_PARAMS
        );
        assert_eq!(
            code_of(call(Method::ViewImage, "C:\\work\\doc.pdf")),
            code::BAD_PARAMS
        );
        // a.png as named, but the file system says run.txt / sneaky.exe (rule 14).
        assert_eq!(
            code_of(call(Method::ViewImage, "C:\\work\\link.png")),
            code::BAD_PARAMS
        );
        assert_eq!(
            code_of(call(Method::ViewImage, "C:\\work\\sneaky.png")),
            code::BAD_PARAMS
        );
        assert!(
            r.ops.lock().unwrap().iter().all(|o| o == "CtlOp::TabCwd"),
            "nothing was shown"
        );
    }

    #[test]
    fn bad_params_are_named_before_the_ui_is_asked() {
        let r = rig(ok_all, false, FILES);
        let c = agent_in_tab();
        for (m, p) in [
            (Method::ViewImage, json!({})),
            (Method::ViewImage, json!({"path": ""})),
            (Method::ViewImage, json!({"path": "  "})),
            (Method::ViewImage, json!({"path": 7})),
            (Method::ViewImage, json!({"path": "a\u{0}b"})),
            (
                Method::ViewFile,
                json!({"path": "C:\\work\\a.txt", "line": 0}),
            ),
            (
                Method::ViewFile,
                json!({"path": "C:\\work\\a.txt", "line": "3"}),
            ),
            (
                Method::ViewImage,
                json!({"path": "C:\\work\\a.png", "title": 3}),
            ),
            (Method::ViewText, json!({})),
            (Method::ViewText, json!({"text": 3})),
            (Method::ViewText, json!({"text": "x", "title": []})),
            (Method::ViewText, json!({"text": "x", "language": 1})),
        ] {
            assert_eq!(
                code_of(r.d.call(&c, m, p.clone())),
                code::BAD_PARAMS,
                "{} {p}",
                m.name()
            );
        }
        assert!(r.ops.lock().unwrap().is_empty());
    }

    #[test]
    fn view_text_is_capped_and_a_title_is_one_short_line() {
        let r = rig(ok_all, false, FILES);
        let c = agent_in_tab();
        let big = "x".repeat(TEXT_MAX + 1);
        assert_eq!(
            code_of(r.d.call(&c, Method::ViewText, json!({"text": big}))),
            code::BAD_PARAMS
        );
        let exact = "x".repeat(TEXT_MAX);
        assert!(
            r.d.call(&c, Method::ViewText, json!({"text": exact}))
                .is_ok()
        );
        assert_eq!(
            title_param(&json!({"title": "a\r\nb\tc"})).unwrap(),
            Some("a  b c".to_string())
        );
        assert_eq!(title_param(&json!({"title": "   "})).unwrap(), None);
        let long = "t".repeat(TITLE_MAX + 50);
        assert_eq!(
            title_param(&json!({"title": long})).unwrap().unwrap().len(),
            TITLE_MAX
        );
    }

    #[test]
    fn convo_state_asks_for_the_callers_own_tab_only() {
        let r = rig(|_| Ok(json!({"tracking": false})), false, FILES);
        let v =
            r.d.call(&agent_in_tab(), Method::ConvoState, json!({"tab": "tab-2"}))
                .unwrap();
        assert_eq!(v["tracking"], json!(false));
        let ops = r.ops.lock().unwrap().clone();
        assert_eq!(ops, ["CtlOp::ConvoState"]);
        // A caller with no tab has nothing to ask about.
        assert_eq!(
            code_of(r.d.call(
                &ctx(None, Grant::Agent { eval: false }),
                Method::ConvoState,
                json!({})
            )),
            code::NO_TAB
        );
    }

    #[test]
    fn panel_open_takes_the_conversation_kinds_and_nothing_more() {
        let r = rig(ok_all, false, FILES);
        let v =
            r.d.call(
                &agent_in_tab(),
                Method::PanelOpen,
                json!({"kind": "changes"}),
            )
            .unwrap();
        assert!(v["panel"].is_object());
        assert_eq!(r.ops.lock().unwrap().as_slice(), ["CtlOp::ShowConvo"]);
        // A path, an address or a text does not go with them, and an unknown kind is refused.
        for extra in ["url", "path", "text"] {
            assert_eq!(
                code_of(r.d.call(
                    &agent_in_tab(),
                    Method::PanelOpen,
                    json!({"kind": "changes", extra: "x"})
                )),
                code::BAD_PARAMS,
                "{extra}"
            );
        }
        assert_eq!(
            code_of(r.d.call(
                &agent_in_tab(),
                Method::PanelOpen,
                json!({"kind": "dashboard"})
            )),
            code::BAD_PARAMS
        );
        assert_eq!(
            code_of(r.d.call(&agent_in_tab(), Method::PanelOpen, json!({"kind": "board"}))),
            code::BAD_PARAMS,
            "the agent board was removed (2026-10-22): its kind is unknown"
        );
        assert_eq!(
            code_of(r.d.call(&agent_in_tab(), Method::PanelOpen, json!({"kind": "tasks"}))),
            code::BAD_PARAMS,
            "the Tasks panel was removed (2026-10-26): its kind is unknown"
        );
        assert_eq!(
            r.ops.lock().unwrap().len(),
            1,
            "the refusals never reached the window"
        );
    }

    #[test]
    fn methods_this_build_does_not_have_are_unavailable() {
        let r = rig(ok_all, false, FILES);
        assert_eq!(
            code_of(r.d.call(&agent_in_tab(), Method::ToolsRespond, json!({}))),
            code::UNAVAILABLE
        );
    }

    #[test]
    fn an_unanswered_call_times_out_and_a_stop_ends_the_wait_within_a_slice() {
        // Stop flag: the wait ends within one slice of it being set.
        let r = rig(ok_all, true, FILES);
        let stopping = r.d.stopping.clone();
        let started = Instant::now();
        let h = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            stopping.store(true, Ordering::SeqCst);
        });
        let e =
            r.d.call(&agent_in_tab(), Method::PanelState, json!({}))
                .unwrap_err();
        h.join().unwrap();
        assert_eq!(e.code, code::UNAVAILABLE);
        assert!(started.elapsed() < Duration::from_secs(2));
        // Already stopping: no message is posted at all.
        let before = r.ops.lock().unwrap().len();
        let e =
            r.d.call(&agent_in_tab(), Method::PanelClose, json!({}))
                .unwrap_err();
        assert_eq!(e.code, code::UNAVAILABLE);
        assert_eq!(r.ops.lock().unwrap().len(), before);
    }

    #[test]
    fn a_message_dropped_by_the_ui_thread_answers_unavailable_at_once() {
        // The window is gone: the post drops the message, so its sender is dropped unanswered.
        let post: Post = Arc::new(|_m: UiMsg| {});
        let d = AppDispatcher::new(
            post,
            Arc::new(|_| None),
            real_resolve(),
            String::new(),
            Arc::new(AtomicBool::new(false)),
        );
        let started = Instant::now();
        let e = d
            .call(&agent_in_tab(), Method::PanelState, json!({}))
            .unwrap_err();
        assert_eq!(e.code, code::UNAVAILABLE);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn an_absolute_path_never_asks_the_ui_for_a_folder_and_a_relative_one_does() {
        const REL: &[(&str, &str)] = &[("a.png", "C:\\work\\a.png")];
        let r = rig(ok_all, false, FILES);
        r.d.call(
            &agent_in_tab(),
            Method::ViewImage,
            json!({"path": "C:\\work\\a.png"}),
        )
        .unwrap();
        assert_eq!(r.ops.lock().unwrap().as_slice(), ["CtlOp::ShowFile"]);
        let r = rig(ok_all, false, REL);
        r.d.call(&agent_in_tab(), Method::ViewImage, json!({"path": "a.png"}))
            .unwrap();
        assert_eq!(
            r.ops.lock().unwrap().as_slice(),
            ["CtlOp::TabCwd", "CtlOp::ShowFile"]
        );
    }

    #[test]
    fn every_request_carries_one_deadline_inside_the_channel_s_own_timeout() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        let post: Post = Arc::new(move |m: UiMsg| {
            let UiMsg::Ctl(CtlRequestBox(req)) = m else {
                return;
            };
            s.lock().unwrap().push(req.deadline);
            let _ = req
                .reply
                .send(Ok(json!({"cwd": "C:\\work", "panel": {"kind": "image"}})));
        });
        let resolve: Resolve = Arc::new(|_b, t, _h| {
            let full = format!("C:\\work\\{t}");
            Some((full.clone(), full))
        });
        let d = AppDispatcher::new(
            post,
            Arc::new(|_| None),
            resolve,
            String::new(),
            Arc::new(AtomicBool::new(false)),
        );
        let before = Instant::now();
        d.call(&agent_in_tab(), Method::ViewImage, json!({"path": "a.png"}))
            .unwrap();
        let deadlines = seen.lock().unwrap().clone();
        assert_eq!(deadlines.len(), 2, "the folder question and the show");
        assert_eq!(
            deadlines[0], deadlines[1],
            "one deadline for the whole call"
        );
        assert!(deadlines[0] > before && deadlines[0] <= Instant::now() + UI_WAIT);
        assert!(
            UI_WAIT < Method::ViewImage.timeout(),
            "the call answers `timeout` itself before the server gives up on it"
        );
    }

    // ---- mini-apps and diagrams (stage 5) ------------------------------------------------------------

    const FAKE_MERMAID: &[u8] = b"/* mermaid stand-in */";

    /// A dispatcher that can write mini-apps into `root`; the UI thread answers every op at once and
    /// the ops it saw are kept.
    struct AppRig {
        d: AppDispatcher,
        ops: Arc<Mutex<Vec<CtlOp>>>,
    }

    fn app_rig(
        root: &std::path::Path,
        network: bool,
        pages: &'static [(&'static str, &'static str)],
    ) -> AppRig {
        let ops: Arc<Mutex<Vec<CtlOp>>> = Arc::default();
        let seen = ops.clone();
        let post: Post = Arc::new(move |m: UiMsg| {
            let UiMsg::Ctl(CtlRequestBox(req)) = m else {
                return;
            };
            let answer = match &req.op {
                CtlOp::TabCwd { .. } => json!({"cwd": "C:\\work"}),
                CtlOp::ShowApp { spec, .. } => json!({
                    "panel": {"kind": "app", "network": spec.allow_network},
                    "loaded": true,
                }),
                _ => json!({}),
            };
            seen.lock().unwrap().push(req.op);
            let _ = req.reply.send(Ok(answer));
        });
        let resolve_html: Resolve = Arc::new(move |_base, text, _home| {
            pages
                .iter()
                .find(|(name, _)| *name == text)
                .map(|(name, real)| ((*name).to_string(), (*real).to_string()))
        });
        let tab_of: TabOf = Arc::new(|t| (t == "secret").then(|| "tab-1".to_string()));
        let d = AppDispatcher::new(
            post,
            tab_of,
            Arc::new(|_, _, _| None),
            "C:\\Users\\x".into(),
            Arc::new(AtomicBool::new(false)),
        )
        .with_apps(AppsCfg::new(
            root.to_path_buf(),
            resolve_html,
            Arc::new(move || network),
            FAKE_MERMAID,
        ));
        AppRig { d, ops }
    }

    /// The `ShowApp` the UI thread was last asked for: (tab, spec, title, diagram).
    fn last_app(r: &AppRig) -> (String, AppSpec, String, bool) {
        match r.ops.lock().unwrap().iter().rev().find_map(|o| match o {
            CtlOp::ShowApp {
                tab,
                spec,
                title,
                diagram,
            } => Some((tab.clone(), spec.clone(), title.clone(), *diagram)),
            _ => None,
        }) {
            Some(x) => x,
            None => panic!("the UI thread was not asked to show an app"),
        }
    }

    #[test]
    fn view_html_writes_the_page_policy_first_and_asks_the_ui_to_show_it() {
        let dir = tempfile::tempdir().unwrap();
        let r = app_rig(dir.path(), false, &[]);
        let v =
            r.d.call(
                &agent_in_tab(),
                Method::ViewHtml,
                json!({"html": "<h1 id=a>hi</h1><script>1</script>", "title": "Chart"}),
            )
            .unwrap();
        assert_eq!(v["panel"]["kind"], "app");
        let (tab, spec, title, diagram) = last_app(&r);
        assert_eq!(
            (tab.as_str(), title.as_str(), diagram),
            ("tab-1", "Chart", false)
        );
        assert!(!spec.allow_network);
        assert_eq!(spec.url, app::app_url(&spec.host));
        assert!(app::is_app_url(&spec.url));
        assert!(spec.folder.starts_with(dir.path()));
        let page = std::fs::read_to_string(spec.folder.join(app::PAGE)).unwrap();
        assert!(page.starts_with("<!doctype html><meta charset=\"utf-8\">"));
        assert!(page.contains(&app::csp(false)));
        assert!(page.find("Content-Security-Policy") < page.find("<h1"));
        assert!(page.ends_with("<h1 id=a>hi</h1><script>1</script>"));
        // Only the page: no other file is served from the folder.
        assert_eq!(std::fs::read_dir(&spec.folder).unwrap().count(), 1);
    }

    #[test]
    fn a_title_defaults_and_each_app_gets_a_host_and_folder_of_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let r = app_rig(dir.path(), false, &[]);
        let c = agent_in_tab();
        r.d.call(&c, Method::ViewHtml, json!({"html": "<p>one"}))
            .unwrap();
        let first = last_app(&r);
        r.d.call(&c, Method::ViewHtml, json!({"html": "<p>two"}))
            .unwrap();
        let second = last_app(&r);
        assert_eq!(first.2, "Mini-app");
        assert_ne!(first.1.host, second.1.host);
        assert_ne!(first.1.folder, second.1.folder);
        assert!(first.1.folder.is_dir() && second.1.folder.is_dir());
    }

    #[test]
    fn a_mod_opens_only_its_own_panel_and_never_gets_a_networked_page() {
        let dir = tempfile::tempdir().unwrap();
        // The user's network switch is ON: it is for the agent's pages, not a mod's.
        let r = app_rig(dir.path(), true, &[]);
        let m = ctx(
            Some("tab-1"),
            Grant::Mod {
                id: "m".into(),
                permissions: vec![Permission::Panel, Permission::View],
                plugin: true,
            },
        );
        for kind in ["web", "changes", "mod:other"] {
            let e =
                r.d.call(
                    &m,
                    Method::PanelOpen,
                    json!({"kind": kind, "url": "https://evil.example/?d=1"}),
                )
                .unwrap_err();
            assert_eq!(e.code, code::FORBIDDEN, "{kind}: {}", e.message);
        }
        let e =
            r.d.call(
                &m,
                Method::ViewHtml,
                json!({"html": "<img src=https://evil/>", "allow_network": true}),
            )
            .unwrap_err();
        assert_eq!(e.code, code::FORBIDDEN, "{}", e.message);
        assert!(r.ops.lock().unwrap().is_empty(), "the window was not asked");
        // Its own panel is not refused by the rule (this rig has no mods hub, so it is unavailable).
        let e =
            r.d.call(&m, Method::PanelOpen, json!({"kind": "mod:m"}))
                .unwrap_err();
        assert_eq!(e.code, code::UNAVAILABLE);
        // Without the network ask a mod's page is still allowed, and closed to the network.
        r.d.call(&m, Method::ViewHtml, json!({"html": "<p>x"}))
            .unwrap();
        assert!(!last_app(&r).1.allow_network);
    }

    #[test]
    fn the_network_needs_the_agent_to_ask_and_the_users_switch_to_be_on() {
        let page = |network: bool, ask: Value| {
            let dir = tempfile::tempdir().unwrap();
            let r = app_rig(dir.path(), network, &[]);
            let mut p = json!({"html": "<p>x"});
            if !ask.is_null() {
                p["allow_network"] = ask;
            }
            r.d.call(&agent_in_tab(), Method::ViewHtml, p).unwrap();
            let (_, spec, _, _) = last_app(&r);
            let text = std::fs::read_to_string(spec.folder.join(app::PAGE)).unwrap();
            (spec.allow_network, text.contains("https:"))
        };
        assert_eq!(page(false, json!(null)), (false, false));
        assert_eq!(page(false, json!(false)), (false, false));
        assert_eq!(
            page(false, json!(true)),
            (false, false),
            "the switch is off"
        );
        assert_eq!(
            page(true, json!(null)),
            (false, false),
            "the agent did not ask"
        );
        assert_eq!(page(true, json!(false)), (false, false));
        assert_eq!(page(true, json!(true)), (true, true));
    }

    #[test]
    fn view_html_checks_its_params_before_anything_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let r = app_rig(dir.path(), false, &[]);
        let c = agent_in_tab();
        let too_big = "x".repeat(app::HTML_MAX + 1);
        for bad in [
            json!({}),
            json!({"html": ""}),
            json!({"html": "   "}),
            json!({"html": 4}),
            json!({"html": too_big}),
            json!({"html": "x", "path": "C:\\a.html"}),
            json!({"path": ""}),
            json!({"path": "a\u{0}b.html"}),
            json!({"html": "x", "allow_network": "yes"}),
            json!({"html": "x", "title": []}),
        ] {
            let shown = format!("{bad}").chars().take(60).collect::<String>();
            assert_eq!(
                code_of(r.d.call(&c, Method::ViewHtml, bad)),
                code::BAD_PARAMS,
                "{shown}"
            );
        }
        assert!(r.ops.lock().unwrap().is_empty(), "the UI was never asked");
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            0,
            "nothing written"
        );
        // Exactly the cap is fine.
        let exact = "x".repeat(app::HTML_MAX);
        assert!(
            r.d.call(&c, Method::ViewHtml, json!({"html": exact}))
                .is_ok()
        );
    }

    #[test]
    fn a_page_named_by_path_is_resolved_again_vetted_and_read() {
        let dir = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let good = src.path().join("chart.html");
        std::fs::write(&good, "<p id=g>from a file</p>").unwrap();
        let big = src.path().join("big.html");
        std::fs::write(&big, vec![b'x'; app::HTML_MAX + 1]).unwrap();
        let bin = src.path().join("bin.html");
        std::fs::write(&bin, [0xffu8, 0xfe, 0x00, 0x80]).unwrap();
        let empty = src.path().join("empty.html");
        std::fs::write(&empty, "  \n").unwrap();
        // The resolver stands in for the backend's: it knows these names only.
        let leak: &'static [(&'static str, &'static str)] = Box::leak(
            vec![
                (
                    "chart.html",
                    &*Box::leak(good.to_string_lossy().into_owned().into_boxed_str()),
                ),
                (
                    "big.html",
                    &*Box::leak(big.to_string_lossy().into_owned().into_boxed_str()),
                ),
                (
                    "bin.html",
                    &*Box::leak(bin.to_string_lossy().into_owned().into_boxed_str()),
                ),
                (
                    "empty.html",
                    &*Box::leak(empty.to_string_lossy().into_owned().into_boxed_str()),
                ),
                ("gone.html", "C:\\no\\such\\gone.html"),
            ]
            .into_boxed_slice(),
        );
        let r = app_rig(dir.path(), false, leak);
        let c = agent_in_tab();
        r.d.call(&c, Method::ViewHtml, json!({"path": "chart.html"}))
            .unwrap();
        let (_, spec, title, _) = last_app(&r);
        assert_eq!(title, "Mini-app");
        let page = std::fs::read_to_string(spec.folder.join(app::PAGE)).unwrap();
        assert!(page.contains("<p id=g>from a file</p>"));
        assert!(page.contains("Content-Security-Policy"));
        for (p, want) in [
            ("not-listed.html", code::NOT_FOUND),
            ("gone.html", code::NOT_FOUND),
            ("big.html", code::BAD_PARAMS),
            ("bin.html", code::BAD_PARAMS),
            ("empty.html", code::BAD_PARAMS),
        ] {
            assert_eq!(
                code_of(r.d.call(&c, Method::ViewHtml, json!({"path": p}))),
                want,
                "{p}"
            );
        }
        // A relative path asks the UI thread for the tab's folder first; an absolute one does not.
        let before = r.ops.lock().unwrap().len();
        let _ =
            r.d.call(&c, Method::ViewHtml, json!({"path": "C:\\abs\\x.html"}));
        assert_eq!(r.ops.lock().unwrap().len(), before);
    }

    #[test]
    fn view_diagram_writes_the_page_and_the_library_beside_it() {
        let dir = tempfile::tempdir().unwrap();
        let r = app_rig(dir.path(), true, &[]);
        let c = agent_in_tab();
        r.d.call(
            &c,
            Method::ViewDiagram,
            json!({"mermaid": "graph TD\nA-->B", "title": "Flow"}),
        )
        .unwrap();
        let (tab, spec, title, diagram) = last_app(&r);
        assert_eq!(
            (tab.as_str(), title.as_str(), diagram),
            ("tab-1", "Flow", true)
        );
        // No network for a diagram, whatever the switch says.
        assert!(!spec.allow_network);
        let page = std::fs::read_to_string(spec.folder.join(app::PAGE)).unwrap();
        assert!(page.contains("Content-Security-Policy"));
        assert!(page.contains("securityLevel: 'strict'"));
        assert!(page.contains("A-->B"));
        assert_eq!(
            std::fs::read(spec.folder.join(app::MERMAID_FILE)).unwrap(),
            FAKE_MERMAID
        );
        let default_title = r.d.call(
            &c,
            Method::ViewDiagram,
            json!({"mermaid": "graph TD\nA-->B"}),
        );
        assert!(default_title.is_ok());
        assert_eq!(last_app(&r).2, "Diagram");
    }

    /// An `AppRig` whose resolver knows `speech.mp3` (the sound file `real`) and `notes.txt`, and with a hub where the
    /// plugin `radio` holds `network` and `mute` does not.
    fn audio_rig(root: &std::path::Path, real: &std::path::Path) -> (AppRig, Arc<Hub>) {
        let mut r = app_rig(root, false, &[]);
        let real = real.to_string_lossy().into_owned();
        r.d.resolve = Arc::new(move |_base, text, _home| match text {
            "speech.mp3" | "C:\\work\\speech.mp3" => Some((text.to_string(), real.clone())),
            "notes.txt" => Some((text.to_string(), "C:\\work\\notes.txt".to_string())),
            _ => None,
        });
        let plugins = root.join("plugins");
        for (id, perms) in [("radio", r#"["view","network"]"#), ("mute", r#"["view"]"#)] {
            let f = plugins.join(id);
            std::fs::create_dir_all(&f).unwrap();
            std::fs::write(
                f.join("plugin.json"),
                format!(
                    r#"{{"id":"{id}","name":"P {id}","version":"1.0.0","protocol":2,"description":"d","author":"me","homepage":"https://example.com/{id}","permissions":{perms},"background":"bg.html"}}"#
                ),
            )
            .unwrap();
            std::fs::write(f.join("bg.html"), "<p>bg</p>").unwrap();
        }
        let hub = Arc::new(Hub::new(flare_mods::hub::Config {
            dir: plugins,
            old_mods_dir: None,
            state_file: root.join("mods.json"),
        }));
        hub.scan();
        hub.set_enabled("radio", true).unwrap();
        hub.set_enabled("mute", true).unwrap();
        r.d.mods = Some(Arc::clone(&hub));
        (r, hub)
    }

    #[test]
    fn view_audio_copies_a_sound_file_beside_flares_own_player_page() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("speech.mp3");
        std::fs::write(&real, b"ID3 fake").unwrap();
        let (r, _hub) = audio_rig(dir.path(), &real);
        let v =
            r.d.call(
                &agent_in_tab(),
                Method::ViewAudio,
                json!({"path": "speech.mp3", "title": "Reply", "autoplay": true}),
            )
            .unwrap();
        assert_eq!(v["panel"]["kind"], "app");
        let (tab, spec, title, diagram) = last_app(&r);
        assert_eq!(
            (tab.as_str(), title.as_str(), diagram),
            ("tab-1", "Reply", false)
        );
        assert!(!spec.allow_network, "a file needs no network");
        let page = std::fs::read_to_string(spec.folder.join(app::PAGE)).unwrap();
        assert!(page.contains(&flare_surfaces::audio::csp(None)));
        assert!(page.find("Content-Security-Policy") < page.find("<audio"));
        assert!(page.contains("src=\"track.mp3\""));
        assert!(page.contains("if(true&&!once)"), "autoplay once, as asked");
        assert_eq!(
            std::fs::read(spec.folder.join("track.mp3")).unwrap(),
            b"ID3 fake"
        );
        // No title: the default one.
        r.d.call(
            &agent_in_tab(),
            Method::ViewAudio,
            json!({"path": "speech.mp3"}),
        )
        .unwrap();
        assert_eq!(last_app(&r).2, flare_surfaces::audio::DEFAULT_TITLE);
    }

    #[test]
    fn view_audio_refuses_what_is_not_sound_local_addresses_and_a_plugin_without_network() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("speech.mp3");
        std::fs::write(&real, b"x").unwrap();
        let (r, _hub) = audio_rig(dir.path(), &real);
        let c = agent_in_tab();
        for (params, want) in [
            (json!({}), code::BAD_PARAMS),
            (
                json!({"path": "speech.mp3", "url": "https://example.com/a.mp3"}),
                code::BAD_PARAMS,
            ),
            (json!({"path": "notes.txt"}), code::BAD_PARAMS),
            (json!({"path": "missing.mp3"}), code::NOT_FOUND),
            (
                json!({"path": "speech.mp3", "loop": true}),
                code::BAD_PARAMS,
            ),
            (json!({"url": "http://example.com/a.mp3"}), code::BAD_PARAMS),
            (json!({"url": "https://localhost/a.mp3"}), code::BAD_PARAMS),
            (
                json!({"url": "https://192.168.1.4/a.mp3"}),
                code::BAD_PARAMS,
            ),
            (
                json!({"url": "https://printer.local/a.mp3"}),
                code::BAD_PARAMS,
            ),
            (json!({"url": "https://[::1]/a.mp3"}), code::BAD_PARAMS),
        ] {
            let e = r.d.call(&c, Method::ViewAudio, params.clone()).unwrap_err();
            assert_eq!(e.code, want, "{params}: {}", e.message);
        }
        assert!(
            r.ops
                .lock()
                .unwrap()
                .iter()
                .all(|o| !matches!(o, CtlOp::ShowApp { .. }))
        );
        // A public https address: the networked player, its policy lets in that origin's media only.
        r.d.call(
            &c,
            Method::ViewAudio,
            json!({"url": "https://cdn.example.com/a.ogg"}),
        )
        .unwrap();
        let (_, spec, _, _) = last_app(&r);
        assert!(spec.allow_network);
        let page = std::fs::read_to_string(spec.folder.join(app::PAGE)).unwrap();
        assert!(
            page.contains("media-src https://cdn.example.com;"),
            "{page}"
        );
        assert!(page.contains("connect-src 'none'"));
        assert_eq!(std::fs::read_dir(&spec.folder).unwrap().count(), 1);
        // A plugin plays an address only with `network`; a file it may play with `view` alone.
        let plugin = |id: &str, perms: Vec<Permission>| {
            ctx(
                Some("tab-1"),
                Grant::Mod {
                    id: id.into(),
                    permissions: perms,
                    plugin: true,
                },
            )
        };
        let url = json!({"url": "https://cdn.example.com/a.ogg"});
        let e =
            r.d.call(
                &plugin("mute", vec![Permission::View]),
                Method::ViewAudio,
                url.clone(),
            )
            .unwrap_err();
        assert_eq!(e.code, code::FORBIDDEN, "{}", e.message);
        r.d.call(
            &plugin("radio", vec![Permission::View, Permission::Network]),
            Method::ViewAudio,
            url,
        )
        .unwrap();
        r.d.call(
            &plugin("mute", vec![Permission::View]),
            Method::ViewAudio,
            json!({"path": "speech.mp3"}),
        )
        .unwrap();
    }

    #[test]
    fn view_diagram_checks_its_params() {
        let dir = tempfile::tempdir().unwrap();
        let r = app_rig(dir.path(), false, &[]);
        let c = agent_in_tab();
        let big = "x".repeat(app::DIAGRAM_MAX + 1);
        for bad in [
            json!({}),
            json!({"mermaid": ""}),
            json!({"mermaid": " \n "}),
            json!({"mermaid": 3}),
            json!({"mermaid": big}),
            json!({"mermaid": "x", "title": 1}),
        ] {
            assert_eq!(
                code_of(r.d.call(&c, Method::ViewDiagram, bad)),
                code::BAD_PARAMS
            );
        }
        assert!(r.ops.lock().unwrap().is_empty());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn without_a_folder_to_write_to_mini_apps_are_unavailable_and_not_listed() {
        let r = rig(ok_all, false, FILES);
        let c = agent_in_tab();
        assert_eq!(
            code_of(r.d.call(&c, Method::ViewHtml, json!({"html": "<p>x"}))),
            code::UNAVAILABLE
        );
        assert_eq!(
            code_of(r.d.call(
                &c,
                Method::ViewDiagram,
                json!({"mermaid": "graph TD\nA-->B"})
            )),
            code::UNAVAILABLE
        );
        let caps = r.d.capabilities();
        assert!(!caps.contains(&Method::ViewHtml) && !caps.contains(&Method::ViewDiagram));
        let dir = tempfile::tempdir().unwrap();
        let with = app_rig(dir.path(), false, &[]);
        let caps = with.d.capabilities();
        assert!(caps.contains(&Method::ViewHtml) && caps.contains(&Method::ViewDiagram));
    }

    #[test]
    fn the_bundled_mermaid_is_the_pinned_build() {
        use sha2::{Digest, Sha256};
        // assets/mermaid/README.md and the notices name this exact file (mermaid 11.17.2, dist/mermaid.min.js).
        assert_eq!(MERMAID.len(), 3_572_661);
        assert_eq!(
            format!("{:x}", Sha256::digest(MERMAID)),
            "581ed7d74bd9048d0e3a91363927d72ef22942d7722546b27f7cc29e35390eb8"
        );
        assert!(
            MERMAID.ends_with(b"default;\n"),
            "it sets globalThis.mermaid last"
        );
    }

    #[test]
    fn the_debug_form_of_a_mini_app_request_prints_its_kind_only() {
        let (tx, _rx) = mpsc::channel();
        let b = CtlRequestBox(CtlRequest {
            op: CtlOp::ShowApp {
                tab: "t".into(),
                spec: AppSpec {
                    host: "flare-app-1.local".into(),
                    folder: "C:\\SECRET".into(),
                    url: "https://flare-app-1.local/index.html".into(),
                    allow_network: false,
                    module: None,
                },
                title: "SECRET".into(),
                diagram: false,
            },
            deadline: Instant::now(),
            reply: tx,
        });
        assert_eq!(format!("{b:?}"), "CtlOp::ShowApp");
    }

    #[test]
    fn the_debug_form_of_a_request_never_prints_a_path_or_text() {
        let (tx, _rx) = mpsc::channel();
        let b = CtlRequestBox(CtlRequest {
            op: CtlOp::ShowText {
                tab: "t".into(),
                text: "SECRET".into(),
                title: None,
            },
            deadline: Instant::now(),
            reply: tx,
        });
        assert_eq!(format!("{b:?}"), "CtlOp::ShowText");
    }
}
