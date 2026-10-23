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
//! A tab is named by its secret (`FLARE_TAB_TOKEN`), never by an id: see `docs/surfaces/protocol.md`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use flare_ctl::{CallContext, CtlError, Dispatcher, Grant, Method, Permission, code};
use flare_surfaces::PanelKind;
use flare_surfaces::agent_job::{Op as AgentOp, Target};
use flare_surfaces::agent_web::{self as agent_web, Button, WaitFor};
use flare_surfaces::app::{self, AppSpec};
use flare_surfaces::preview;
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
            CtlOp::TabCwd { .. } => "CtlOp::TabCwd",
            CtlOp::PanelState { .. } => "CtlOp::PanelState",
            CtlOp::PanelClose { .. } => "CtlOp::PanelClose",
            CtlOp::ShowFile { .. } => "CtlOp::ShowFile",
            CtlOp::ShowText { .. } => "CtlOp::ShowText",
            CtlOp::ShowApp { .. } => "CtlOp::ShowApp",
            CtlOp::BrowserOpen { .. } => "CtlOp::BrowserOpen",
            CtlOp::BrowserNavigate { .. } => "CtlOp::BrowserNavigate",
            CtlOp::BrowserAgent { .. } => "CtlOp::BrowserAgent",
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
}

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
        }
    }

    /// Where mini-apps are written and the user's network switch for them (stage 5).
    pub fn with_apps(mut self, apps: AppsCfg) -> Self {
        self.apps = Some(apps);
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
                "Flare shows pictures (png, jpg, gif, webp, bmp, ico) and text or code files only",
            )
        })?;
        if kind != want || !preview::loadable(&real, &kind) {
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
    /// No mod has a token yet (stage 8): every mod token is unknown.
    fn resolve_token(&self, _token: &str) -> Option<Grant> {
        None
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
        ];
        // Mini-apps are listed only when this run can write them.
        if self.apps.is_some() {
            all.extend([Method::ViewHtml, Method::ViewDiagram]);
        }
        all
    }

    fn call(&self, ctx: &CallContext, method: Method, params: Value) -> Result<Value, CtlError> {
        match method {
            Method::TabsList => {
                // The agent sees its own tab only; a mod that may drive panels sees every tab's id
                // and title (never a screen: S3).
                let all = matches!(&ctx.grant, Grant::Mod { permissions, .. }
                    if permissions.contains(&Permission::Panel));
                let own = if all {
                    None
                } else {
                    match &ctx.tab {
                        Some(t) => Some(t.clone()),
                        None => return Ok(json!({"tabs": []})),
                    }
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
                let tab = Self::tab_of_call(ctx)?;
                match str_param(&params, "kind")? {
                    None => Err(CtlError::new(code::BAD_PARAMS, "kind is required")),
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
                    Some(_) => Err(CtlError::new(
                        code::BAD_PARAMS,
                        "panel.open takes kind web; use view.image, view.file or view.text",
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
    ) -> std::io::Result<Self> {
        let stopping = Arc::new(AtomicBool::new(false));
        let dispatcher = Arc::new(
            AppDispatcher::new(post, tab_of, real_resolve(), home, Arc::clone(&stopping))
                .with_eval(eval)
                .with_apps(apps),
        );
        // sync-ok: startup, creates the pipe and its accept thread
        let server =
            flare_ctl::Server::start(pipe, flare_ctl::ServerConfig::default(), dispatcher)?;
        Ok(Self {
            server: Arc::new(std::sync::Mutex::new(Some(server))),
            stopping,
        })
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
        // The agent's hands on the page (stage 4) are built; the conversation reader is not.
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
        assert!(!caps.contains(&Method::ConvoState));
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
    fn a_mod_that_may_drive_panels_sees_every_tab_and_one_that_may_not_sees_its_own() {
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
                },
            )
        };
        r.d.call(&m(vec![Permission::Panel]), Method::TabsList, json!({}))
            .unwrap();
        r.d.call(&m(vec![Permission::View]), Method::TabsList, json!({}))
            .unwrap();
        assert_eq!(
            *seen.lock().unwrap(),
            [None, Some("tab-1".to_string())],
            "panel permission sees all, view alone sees its own"
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
    fn methods_this_build_does_not_have_are_unavailable() {
        let r = rig(ok_all, false, FILES);
        for m in [Method::ConvoState, Method::ToolsRespond] {
            assert_eq!(
                code_of(r.d.call(&agent_in_tab(), m, json!({}))),
                code::UNAVAILABLE,
                "{}",
                m.name()
            );
        }
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
