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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use flare_ctl::{CallContext, CtlError, Dispatcher, Grant, Method, Permission, code};
use flare_surfaces::PanelKind;
use flare_surfaces::agent_job::{Op as AgentOp, Target};
use flare_surfaces::agent_web::{self as agent_web, Button, WaitFor};
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
    /// `panel.open` with kind `changes` or `tasks` (stage 6): the conversation panel on the tab.
    ShowConvo {
        tab: String,
        kind: PanelKind,
    },
    /// `convo.state` (stage 6): a compact summary of the conversation the tab's tracker has read.
    ConvoState {
        tab: String,
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
            CtlOp::ShowConvo { .. } => "CtlOp::ShowConvo",
            CtlOp::ConvoState { .. } => "CtlOp::ConvoState",
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
        }
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
        vec![
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
        ]
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
                    Some(k @ ("changes" | "tasks")) => {
                        // Native panels of the conversation: no file, no address, nothing to wait for.
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
                    Some(_) => Err(CtlError::new(
                        code::BAD_PARAMS,
                        "panel.open takes kind web, changes or tasks; use view.image, view.file or view.text",
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
    ) -> std::io::Result<Self> {
        let stopping = Arc::new(AtomicBool::new(false));
        let dispatcher = Arc::new(
            AppDispatcher::new(post, tab_of, real_resolve(), home, Arc::clone(&stopping))
                .with_eval(eval),
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
        for kind in ["changes", "tasks"] {
            let v =
                r.d.call(&agent_in_tab(), Method::PanelOpen, json!({"kind": kind}))
                    .unwrap();
            assert!(v["panel"].is_object(), "{kind}");
        }
        assert_eq!(
            r.ops.lock().unwrap().as_slice(),
            ["CtlOp::ShowConvo", "CtlOp::ShowConvo"]
        );
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
            code_of(r.d.call(&agent_in_tab(), Method::PanelOpen, json!({"kind": "board"}))),
            code::BAD_PARAMS
        );
        assert_eq!(
            r.ops.lock().unwrap().len(),
            2,
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
