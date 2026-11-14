//! AgentHub: one object per window that owns the agent indicator (Task 28).
//!
//! It holds the `Indicator`, the arm state per tab (whether an agent's keys are armed), the process
//! poll worker, one question watch per tab, the Sonara posts and the diagnostics. It lives on the UI
//! thread and holds no Slint types. The glue feeds it the drained `HostEvent`s in order, tells it
//! when a tab is born or closed, and asks it for marks, kinds and the attention count.
//!
//! Source: Prism Terminal's `useAgentIndicator.ts` (the wiring), the OSC handler and `moveArm` of
//! `TerminalPanel.tsx`, and `pollAgentsSoon` of `ipc.ts`.

#![allow(dead_code)] // wired by AppCtl (Tasks 29 and 30); remove this line then

use crate::tab_host::HostEvent;
use flare_agent::activity::SUPPRESS_MS;
use flare_agent::arm::{AgentArm, ArmStep, arm_at_start, arm_on_poll, arm_on_prompt, arm_on_title};
use flare_agent::badge::{Attention, AttentionIn, attention_count};
use flare_agent::detect::ProcRow;
use flare_agent::indicator::{Effect, Event, Indicator, Screen};
use flare_agent::kinds::{AgentKind, DetectedAgent};
use flare_agent::osc7501::parse_program_status;
use flare_agent::poll::{AGENT_POLL_MIN, AgentSend, PollDeps, PollHandle, PollWorker};
use flare_agent::question_watch::{QuestionWatch, RowSource};
use flare_agent::question_widget::ShownQuestion;
use flare_agent::resume::valid_resume;
use flare_agent::reveal::agent_of_resume;
use flare_agent::signal::{is_session_id, parse_agent_signal};
use flare_agent::sonara::post_question_shown;
use flare_agent::tab_look::Switches;
use flare_diag::agent_diag::{AgentDiag, DiagSink, DiagValue, MarkSets};
use flare_dictation::route::SendTab;
use flare_settings::kv::PrefStore;
use flare_term::handle::TermHandle;
use flare_term::session::SessionManager;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

/// Output must settle this long before the question widget is read (PT `questionWatch`).
const QUESTION_SETTLE_MS: u64 = 150;
/// A warm look after a shell really started.
const START_LOOK_MS: u64 = 300;
/// How long the output a remote tab shows as it attaches (the host's screen, then what it missed) is not scored as
/// work (rule 289).
const REMOTE_ATTACH_QUIET_MS: u64 = 3000;
/// Sonara's connect, write and read each get this long (PT: 1500 ms).
const SONARA_TIMEOUT: Duration = Duration::from_millis(1500);

/// `TabHost::handle`.
pub type HandleOf = dyn Fn(&str) -> Option<Arc<TermHandle>>;
/// A tab's `FLARE_TAB_ID`.
pub type TokenOf = dyn Fn(&str) -> Option<String>;
/// An environment lookup.
pub type EnvOf = dyn Fn(&str) -> Option<String> + Send + Sync;

/// What the hub needs from the outside. Every closure is the window's own.
pub struct HubDeps {
    pub prefs: PrefStore,
    /// Milliseconds, monotonic.
    pub now: Rc<dyn Fn() -> u64>,
    /// `TabHost::handle`.
    pub handle: Rc<HandleOf>,
    /// `SessionManager::tab_token`: the tab's `FLARE_TAB_ID`.
    pub token: Rc<TokenOf>,
    /// Production: `sonara_post()`, which spawns a short-lived thread. Tests: records.
    pub post_question: Arc<dyn Fn(String, ShownQuestion) + Send + Sync>,
    pub diag: Arc<dyn DiagSink>,
    /// The poll worker wakes the UI thread (into `UiMsg::HubWake`).
    pub wake: Arc<dyn Fn() + Send + Sync>,
    /// An agent known to be here has left, and whatever it asked the terminal for (theme reports)
    /// goes with it.
    pub reports_off: Rc<dyn Fn(&str)>,
}

/// Why the agent's badges and progress must leave a tab (plugins phase 1): they belong to one Claude session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecorGone {
    /// The tab's session changed from one id to another (a first id after none is not a change).
    SessionChanged,
    /// The agent left the tab (its kind is none now): Claude and Codex alike.
    AgentLeft,
}

/// One answer of the poll, queued for the UI thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PollMsg {
    pub id: String,
    pub present: bool,
    pub kind: Option<DetectedAgent>,
}

/// Builds the poll's world on the worker thread (tests inject one).
pub type PollWorkerFactory = Box<dyn FnOnce() -> PollDeps + Send>;

pub enum PollSource {
    Real(Arc<SessionManager>),
    Fake(PollWorkerFactory),
}

/// What the hub asks of the poll. `PollWorker` is the real one; tests record.
pub trait PollCtl {
    fn look_now(&self);
    fn soon(&self, ms: u64);
    fn again(&self, id: &str);
}

impl PollCtl for PollWorker {
    fn look_now(&self) {
        PollWorker::look_now(self);
    }
    fn soon(&self, ms: u64) {
        PollWorker::soon(self, ms);
    }
    fn again(&self, id: &str) {
        PollWorker::again(self, id);
    }
}

/// The indicator's Screen over the live handles. A remote tab's screen is the host's, partly a replay: it reads as
/// blank, so nothing on it raises a question or a background mark here (rule 289).
struct HandleScreen<'a> {
    handle: &'a dyn Fn(&str) -> Option<Arc<TermHandle>>,
    blind: &'a HashSet<String>,
}

impl Screen for HandleScreen<'_> {
    fn tail(&self, tab: &str, rows: usize) -> Vec<String> {
        if self.blind.contains(tab) {
            return Vec::new();
        }
        (self.handle)(tab)
            .map(|h| h.screen_tail(rows))
            .unwrap_or_default()
    }
}

/// The question watch's rows, read from one handle.
struct HandleRows<'a>(&'a TermHandle);

impl RowSource for HandleRows<'_> {
    fn live_rows(&self) -> Vec<String> {
        self.0.question_rows()
    }
}

fn kind_word(k: DetectedAgent) -> String {
    match k {
        DetectedAgent::Claude => "claude",
        DetectedAgent::Codex => "codex",
        DetectedAgent::Agy => "agy",
        DetectedAgent::Other => "other",
    }
    .to_string()
}

/// The production Sonara post: a short-lived thread per post, so the UI thread never waits on the
/// network (rule 34). `env` is how a test points it at a fake `runtime.json`.
pub fn threaded_post(
    env: Arc<EnvOf>,
    timeout: Duration,
) -> Arc<dyn Fn(String, ShownQuestion) + Send + Sync> {
    threaded_post_counted(env, timeout, Arc::new(AtomicUsize::new(0)))
}

/// How many posts may wait for the worker; a Sonara that hangs cannot grow the backlog past this.
const POST_BACKLOG: usize = 8;

/// [`threaded_post`] with a count of the worker threads it started (a test reads it: it is one, once,
/// however many posts come; audit L22).
pub fn threaded_post_counted(
    env: Arc<EnvOf>,
    timeout: Duration,
    started: Arc<AtomicUsize>,
) -> Arc<dyn Fn(String, ShownQuestion) + Send + Sync> {
    let (tx, rx) = std::sync::mpsc::sync_channel::<(String, ShownQuestion)>(POST_BACKLOG);
    let worker = std::thread::Builder::new()
        .name("flare-sonara-post".into())
        .spawn(move || {
            while let Ok((token, shown)) = rx.recv() {
                // sync-ok: the Sonara post worker, never the UI thread
                post_question_shown(&token, &shown, &|k| env(k), timeout);
            }
        });
    match worker {
        Ok(_) => {
            started.fetch_add(1, Ordering::SeqCst);
        }
        Err(e) => log::warn!("could not start the Sonara post thread: {e}"),
    }
    Arc::new(move |token, shown| {
        // Full or gone: the post is dropped, quietly, like every other Sonara failure.
        let _ = tx.try_send((token, shown));
    })
}

/// The post the app uses: the real environment and PT's 1500 ms.
pub fn sonara_post() -> Arc<dyn Fn(String, ShownQuestion) + Send + Sync> {
    threaded_post(Arc::new(|k| std::env::var(k).ok()), SONARA_TIMEOUT)
}

/// The environment of a `--e2e` run's post: the process's `SONARA_HOME` (the test points it at a
/// sandbox) and nothing else, so a run with no sandbox never reaches the owner's `%LOCALAPPDATA%\Sonara`.
pub fn e2e_sonara_env(real: impl Fn(&str) -> Option<String>) -> impl Fn(&str) -> Option<String> {
    move |k| if k == "SONARA_HOME" { real(k) } else { None }
}

/// The Sonara post of a `--e2e` run: [`e2e_sonara_env`] over the real environment, PT's 1500 ms.
pub fn e2e_sonara_post() -> Arc<dyn Fn(String, ShownQuestion) + Send + Sync> {
    threaded_post(
        Arc::new(e2e_sonara_env(|k| std::env::var(k).ok())),
        SONARA_TIMEOUT,
    )
}

pub struct AgentHub {
    deps: HubDeps,
    ind: Indicator,
    arms: ArmMap,
    watches: HashMap<String, QuestionWatch>,
    /// Tabs the hub has heard of and not yet been told closed.
    tabs: HashSet<String>,
    /// The same tabs in the order the hub first heard of them (the dictation picker's order).
    order: Vec<String>,
    /// Each tab's folder, from its spawn and its prompt (OSC 7), for the dictation list.
    folders: HashMap<String, String>,
    /// Each tab's Claude session (a lower-case UUID): named by the turn's own hooks, or by the
    /// resume id of a restored tab; gone with the agent (N25).
    sessions: HashMap<String, String>,
    /// Each tab's agent folder: the folder its Claude session works in, as its hooks last named it (`panel.state`
    /// with `cwd`, changes after review 2026-11-11). A relative file link is read from it. Gone with the agent.
    agent_folders: HashMap<String, String>,
    /// Each tab's agent start folder: the first folder its session's hooks named since the agent came or the session
    /// changed (rule 1240). A relative link the agent folder lacks is tried there next. Gone with the agent.
    agent_start_folders: HashMap<String, String>,
    /// Tabs the process poll has answered for at least once (the e2e probe `agent.state` says so).
    polled: HashSet<String>,
    /// Tabs whose agent decor must go, and why, in the order it happened; the window takes them
    /// ([`AgentHub::take_decor_gone`]). A closed tab is not here: the window clears it on close.
    decor_gone: Vec<(String, DecorGone)>,
    /// Remote tabs (remote sessions PR 3): their processes are the host's, so the poll is blind to them, and their
    /// screen is not read for a question (the host's question text crosses the link as words in PR 4).
    remote: HashSet<String>,
    /// The state word a remote tab's host said last, as applied (remote sessions PR 4): only a new word moves a mark.
    remote_said: HashMap<String, String>,
    /// The words of a remote tab's question, as its host read them off its screen (rule 294).
    remote_question: HashMap<String, String>,
    diag: AgentDiag,
    poll: Option<Box<dyn PollCtl>>,
    tx: Sender<PollMsg>,
    rx: Receiver<PollMsg>,
    changed: bool,
}

impl AgentHub {
    pub fn new(deps: HubDeps) -> Self {
        let (tx, rx) = mpsc::channel();
        let diag = AgentDiag::new(Arc::clone(&deps.diag));
        Self {
            deps,
            ind: Indicator::new(),
            arms: ArmMap::default(),
            watches: HashMap::new(),
            tabs: HashSet::new(),
            order: Vec::new(),
            folders: HashMap::new(),
            sessions: HashMap::new(),
            agent_folders: HashMap::new(),
            agent_start_folders: HashMap::new(),
            polled: HashSet::new(),
            decor_gone: Vec::new(),
            remote: HashSet::new(),
            remote_said: HashMap::new(),
            remote_question: HashMap::new(),
            diag,
            poll: None,
            tx,
            rx,
            changed: false,
        }
    }

    fn now(&self) -> u64 {
        (self.deps.now)()
    }

    /// Starts the poll worker. Its answers are queued and the window is woken; `drain_poll`
    /// applies them on the UI thread.
    pub fn start_poll(&mut self, src: PollSource) {
        let tx = self.tx.clone();
        let wake = Arc::clone(&self.deps.wake);
        let diag = Arc::clone(&self.deps.diag);
        let worker = PollWorker::start(
            Duration::from_millis(AGENT_POLL_MIN),
            move |_handle: PollHandle| {
                let send: AgentSend = Box::new(move |id, present, kind| {
                    let _ = tx.send(PollMsg {
                        id: id.to_string(),
                        present,
                        kind,
                    });
                    wake();
                });
                let deps = match src {
                    PollSource::Real(mgr) => real_poll_deps(mgr, diag),
                    PollSource::Fake(make) => make(),
                };
                (send, deps)
            },
        );
        self.poll = Some(Box::new(worker));
    }

    /// Replaces the poll with a recorder.
    #[cfg(test)]
    fn set_poll_ctl(&mut self, ctl: Box<dyn PollCtl>) {
        self.poll = Some(ctl);
    }

    /// A poll answer as the worker would queue it (the tests and the headless scenes).
    pub(crate) fn queue_poll_answer(&self, id: &str, present: bool, kind: Option<DetectedAgent>) {
        let _ = self.tx.send(PollMsg {
            id: id.to_string(),
            present,
            kind,
        });
    }

    /// Window flow tests record what Sonara would be sent and what the diagnostics would write.
    #[cfg(test)]
    pub(crate) fn set_sinks(
        &mut self,
        post_question: Arc<dyn Fn(String, ShownQuestion) + Send + Sync>,
        diag: Arc<dyn DiagSink>,
    ) {
        self.deps.post_question = post_question;
        self.deps.diag = Arc::clone(&diag);
        self.diag = AgentDiag::new(diag);
    }

    fn arm_of(&self, tab: &str) -> AgentArm {
        self.arms.get(tab).unwrap_or_else(|| arm_at_start(None))
    }

    fn apply_arm(&mut self, tab: &str, step: ArmStep) {
        self.arms.insert(tab.to_string(), step.arm);
        if step.gone {
            self.sessions.remove(tab);
            self.agent_folders.remove(tab);
            self.agent_start_folders.remove(tab);
            (self.deps.reports_off)(tab);
            self.decor_gone
                .push((tab.to_string(), DecorGone::AgentLeft));
        }
        if step.recheck
            && let Some(p) = &self.poll
        {
            p.again(tab);
        }
    }

    /// True for a tab whose shell hides its processes from the poll (a WSL distro, a remote tab).
    fn poll_blind(&self, tab: &str) -> bool {
        self.remote.contains(tab)
            || (self.deps.handle)(tab)
                .and_then(|h| h.shell_id())
                .is_some_and(|s| s.to_ascii_lowercase().starts_with("wsl"))
    }

    fn run_effects(&self, effects: Vec<Effect>) {
        for e in effects {
            match e {
                Effect::AskLook { tab } => {
                    if let Some(p) = &self.poll {
                        // The poll sends only answers that changed: a tab it already reported "no agent"
                        // would never hear it was wrong without being asked about by name.
                        // A WSL tab is the exception: the poll cannot see an agent inside it (arm.rs keeps a
                        // titled agent there for the same reason), so its "no" must not undo a real claim.
                        if !self.poll_blind(&tab) {
                            p.again(&tab);
                        }
                        p.look_now();
                    }
                }
            }
        }
    }

    fn ind_handle(&mut self, now: u64, ev: Event) {
        let screen = HandleScreen {
            handle: &*self.deps.handle,
            blind: &self.remote,
        };
        let effects = self.ind.handle(now, ev, &screen);
        self.run_effects(effects);
    }

    /// Diagnostics for what the indicator just logged, and the mark moves.
    fn settle(&mut self, now: u64) {
        for item in self.ind.drain_log() {
            match item {
                flare_agent::indicator::LogItem::Hook { tab, state, kind } => {
                    self.diag.log_hook(&tab, &state, kind.as_deref(), now);
                }
                flare_agent::indicator::LogItem::Title { tab, state, agent } => {
                    self.diag.log_title(&tab, &state, agent.as_deref(), now);
                }
                flare_agent::indicator::LogItem::Why { tab, why } => {
                    self.diag.note_why(&tab, &why, now);
                }
            }
        }
        if self.ind.take_changed() {
            let sets = MarkSets {
                working: self.ind.working_ids(),
                question: self.ind.question_ids(),
                failed: self.ind.failed_ids(),
                done: self.ind.done_ids(),
            };
            let ind = &self.ind;
            self.diag
                .log_marks(&sets, &|id| ind.agent_kind(id).map(kind_word), now);
            self.changed = true;
        }
    }

    /// The text of the question the tab's agent shows on screen, when the widget is up and the screen was read.
    pub fn question_text(&self, tab: &str) -> Option<String> {
        // A remote tab's screen is the host's: its question's words are the ones the host read (rule 294).
        if self.remote.contains(tab) {
            return self.remote_question.get(tab).cloned();
        }
        match self.watches.get(tab)?.current()? {
            ShownQuestion::Text(t) => Some(t.clone()),
            ShownQuestion::Review => None,
        }
    }

    /// A question box is up in the tab: the agent's hooks say so, or the screen's watch has read one.
    pub fn question_up(&self, tab: &str) -> bool {
        self.ind.question_ids().contains(tab)
            || self.watches.get(tab).is_some_and(|w| w.current().is_some())
    }

    /// The tabs whose agent has an open question now (the mark), in the order the hub heard of them.
    pub fn question_tabs(&self) -> Vec<String> {
        self.order
            .iter()
            .filter(|t| self.ind.question_ids().contains(*t) && self.ind.agent_ids().contains(*t))
            .cloned()
            .collect()
    }

    /// The whole drained batch, in order.
    pub fn on_host_events(&mut self, evs: &[HostEvent]) {
        let now = self.now();
        // A flood of Output for one tab costs one pass: an Output straight after an Output of the
        // same tab (nothing between) adds nothing.
        let mut after_output: HashSet<&str> = HashSet::new();
        for ev in evs {
            if let HostEvent::Output { id } = ev {
                if !after_output.insert(id.as_str()) {
                    continue;
                }
            } else if let Some(id) = event_tab(ev) {
                after_output.remove(id);
            }
            if let Some(id) = event_tab(ev) {
                self.note_tab(id);
            }
            match ev {
                HostEvent::Title { id, text } => {
                    self.ind_handle(
                        now,
                        Event::Title {
                            tab: id.clone(),
                            text: text.clone(),
                        },
                    );
                    let step = arm_on_title(&self.arm_of(id), text);
                    self.apply_arm(id, step);
                }
                HostEvent::Osc777 { id, data } => {
                    if let Some(signal) = parse_agent_signal(data) {
                        if let Some(session) = &signal.session {
                            let old = self.sessions.insert(id.clone(), session.clone());
                            // Another session in the same tab: what the old one put on the tab goes.
                            if old.is_some_and(|o| o != *session) {
                                self.agent_start_folders.remove(id);
                                self.decor_gone
                                    .push((id.clone(), DecorGone::SessionChanged));
                            }
                        }
                        self.ind_handle(
                            now,
                            Event::Signal {
                                tab: id.clone(),
                                signal,
                            },
                        );
                    }
                }
                HostEvent::Osc7501 { id, data } => {
                    if let Some(status) = parse_program_status(data) {
                        self.ind_handle(
                            now,
                            Event::Status {
                                tab: id.clone(),
                                status,
                            },
                        );
                    }
                }
                HostEvent::Cwd { id, path } => {
                    self.folders.insert(id.clone(), path.clone());
                    self.ind.activity.mark_prompt(id, now);
                    self.ind_handle(now, Event::Prompt { tab: id.clone() });
                    let step = arm_on_prompt(&self.arm_of(id));
                    self.apply_arm(id, step);
                }
                HostEvent::Output { id } => {
                    self.ind_handle(now, Event::Output { tab: id.clone() });
                    // A remote tab's screen is the host's: its question is not read here (PR 4 sends its words).
                    if !self.remote.contains(id) {
                        self.watches
                            .entry(id.clone())
                            .or_insert_with(|| QuestionWatch::new(QUESTION_SETTLE_MS))
                            .on_output(now);
                    }
                }
                HostEvent::Typed { id } => self.ind.activity.mark_touched(id, now),
                HostEvent::AnswerKey { id, key } => {
                    self.ind_handle(
                        now,
                        Event::Key {
                            tab: id.clone(),
                            key: key.clone(),
                        },
                    );
                }
                HostEvent::Resized { id } => self.ind.activity.suppress(id, now, SUPPRESS_MS),
                HostEvent::Started { .. } => {
                    if let Some(p) = &self.poll {
                        p.soon(START_LOOK_MS);
                    }
                }
                HostEvent::Exited { .. }
                | HostEvent::StartFailed { .. }
                | HostEvent::Bell { .. }
                | HostEvent::ClipboardWrite { .. }
                | HostEvent::Revealed { .. } => {}
            }
        }
        self.settle(now);
    }

    /// Called right after `TabHost::spawn`, before any output can be drained.
    pub fn on_spawned(&mut self, tab: &str, cwd: &str, resume: Option<&str>) {
        let now = self.now();
        self.note_tab(tab);
        self.folders.insert(tab.to_string(), cwd.to_string());
        self.agent_folders.remove(tab);
        self.agent_start_folders.remove(tab);
        let resumed = valid_resume(resume);
        // A restored Claude tab already has its session: the resume id (only a UUID).
        let old = match resumed {
            Some(r) if agent_of_resume(r) == AgentKind::Claude && is_session_id(r) => {
                self.sessions.insert(tab.to_string(), r.to_string())
            }
            _ => self.sessions.remove(tab),
        };
        // A shell started again in a tab that had a session: that session's decor goes.
        if let Some(old) = old
            && self.sessions.get(tab) != Some(&old)
        {
            let why = if self.sessions.contains_key(tab) {
                DecorGone::SessionChanged
            } else {
                DecorGone::AgentLeft
            };
            self.decor_gone.push((tab.to_string(), why));
        }
        self.arms.insert(
            tab.to_string(),
            arm_at_start(resumed.map(|r| DetectedAgent::from(agent_of_resume(r)))),
        );
        if resumed.is_some() {
            // "A claude session from the first moment"; and a resume is not new work (rule 38).
            self.ind.activity.mark_touched(tab, now);
            self.diag.log_restore(tab, Some(cwd), now);
        }
    }

    /// A remote tab (remote sessions PR 3), right after `on_spawned`: the poll is blind to it (its processes are the
    /// host's), its screen is not read for a question, and what it shows when it attaches (a new one or a restored one)
    /// is not new work (rules 38, 289): it counts as touched, as a resumed tab does, and the first output it shows
    /// (the snapshot it attaches with) is not scored as work.
    pub fn on_remote_tab(&mut self, tab: &str) {
        let now = self.now();
        self.note_tab(tab);
        self.remote.insert(tab.to_string());
        if let Some(mut w) = self.watches.remove(tab) {
            w.dispose();
        }
        self.ind.activity.mark_touched(tab, now);
        self.ind.activity.suppress(tab, now, REMOTE_ATTACH_QUIET_MS);
    }

    /// Whether the hub knows `tab` as a remote tab.
    pub fn is_remote(&self, tab: &str) -> bool {
        self.remote.contains(tab)
    }

    /// A remote tab's host said what its agent does (remote sessions PR 4, rule 294): the question's words, the
    /// session, and one state word. The word last applied is skipped while the marks still show it, so a status read
    /// again after a reconnect raises nothing; done and failed are never copied (`Indicator::remote_mark`).
    pub fn on_remote_status(&mut self, tab: &str, st: &flare_term::remote_pty::HostStatus) {
        if !self.remote.contains(tab) {
            return;
        }
        let now = self.now();
        let question = st
            .question
            .as_deref()
            .filter(|q| !q.trim().is_empty())
            .map(str::to_string);
        let asked_before = self.remote_question.get(tab).cloned();
        match &question {
            Some(q) => {
                self.remote_question.insert(tab.to_string(), q.clone());
            }
            None => {
                self.remote_question.remove(tab);
            }
        }
        if asked_before != question {
            // The notifier reads the words: a change is worth a look.
            self.changed = true;
        }
        if let Some(s) = st.session.as_deref().filter(|s| is_session_id(s)) {
            let old = self.sessions.insert(tab.to_string(), s.to_string());
            if old.is_some_and(|o| o != s) {
                self.agent_start_folders.remove(tab);
                self.decor_gone
                    .push((tab.to_string(), DecorGone::SessionChanged));
            }
        }
        // The same word again is skipped only while the marks still show it: a mark this end took down on its own (an
        // answer key read the box gone) is raised again by the host's next word (rule 298).
        if self.remote_said.get(tab) == Some(&st.state) && self.remote_marks_show(tab, &st.state) {
            return;
        }
        let Some(mark) = crate::remote_status::mark_of(st) else {
            return;
        };
        self.remote_said.insert(tab.to_string(), st.state.clone());
        let kind = st.agent.as_deref().and_then(|k| match k {
            "claude" => Some(DetectedAgent::Claude),
            "codex" => Some(DetectedAgent::Codex),
            "agy" => Some(DetectedAgent::Agy),
            _ => None,
        });
        self.ind.remote_mark(now, tab, mark, kind);
        self.settle(now);
    }

    /// Whether the remote tab's marks still show the host word `state`: work and a question as their own mark, any
    /// other word as neither.
    fn remote_marks_show(&self, tab: &str, state: &str) -> bool {
        let working = self.ind.working_ids().contains(tab);
        let asking = self.ind.question_ids().contains(tab);
        match state {
            "working" => working && !asking,
            "question" => asking && !working,
            _ => !working && !asking,
        }
    }

    /// PT `forget`: every record of the tab goes, and its question watch with it.
    pub fn on_closed(&mut self, tab: &str) {
        let now = self.now();
        self.remote.remove(tab);
        self.remote_said.remove(tab);
        self.remote_question.remove(tab);
        self.ind.forget(tab);
        self.arms.remove(tab);
        if let Some(mut w) = self.watches.remove(tab) {
            w.dispose();
        }
        self.tabs.remove(tab);
        self.order.retain(|t| t != tab);
        self.folders.remove(tab);
        self.sessions.remove(tab);
        self.agent_folders.remove(tab);
        self.agent_start_folders.remove(tab);
        self.polled.remove(tab);
        self.decor_gone.retain(|(t, _)| t != tab);
        self.diag.forget(tab, now);
        self.settle(now);
    }

    fn note_tab(&mut self, tab: &str) {
        if self.tabs.insert(tab.to_string()) {
            self.order.push(tab.to_string());
        }
    }

    /// Every tab the hub knows, in the order it heard of them, as the dictation send rules see
    /// them: the folder, the agent the tab runs, its state (`working`, `question`, `done` or
    /// `failed`) and its Claude session (N25, M8 Task 11). The label is the folder's name here;
    /// the window may put the tab's own title over it. `orchestrator` is always false (D9) and
    /// `copy` is the link's to fill.
    pub fn send_tabs(&self) -> Vec<SendTab> {
        self.order
            .iter()
            .map(|tab| {
                let cwd = self.folders.get(tab).cloned().unwrap_or_default();
                let label = cwd
                    .trim_end_matches(['\\', '/'])
                    .rsplit(['\\', '/'])
                    .next()
                    .filter(|name| !name.is_empty())
                    .unwrap_or(tab)
                    .to_string();
                let agent = self.ind.agent_kind(tab).map(kind_word).or_else(|| {
                    flare_agent::keys::AgentView::armed(&self.arms, tab)
                        .map(|k| k.as_str().to_string())
                });
                let state = tab_state(&self.ind, tab);
                SendTab {
                    id: tab.clone(),
                    tab: tab.clone(),
                    label,
                    cwd,
                    agent,
                    state: state.map(str::to_string),
                    orchestrator: false,
                    session: self.sessions.get(tab).cloned(),
                    copy: None,
                }
            })
            .collect()
    }

    pub fn set_looking(&mut self, active: Option<&str>, focused: bool) {
        let now = self.now();
        self.ind.set_looking(now, active, focused);
        self.settle(now);
    }

    /// Applies the poll worker's answers.
    pub fn drain_poll(&mut self) {
        let now = self.now();
        while let Ok(msg) = self.rx.try_recv() {
            if !self.tabs.contains(&msg.id) {
                continue;
            }
            self.polled.insert(msg.id.clone());
            // The poll sends changes only, so this is rare: the trail behind what `tabs.json` says about the
            // tab's agent (2026-10-25: two Claude tabs came back as plain shells and the log could not say why).
            let kind = msg
                .kind
                .map_or("none".to_string(), |k| format!("{k:?}").to_lowercase());
            self.deps.diag.record(
                "agent-poll",
                &[
                    ("id", DiagValue::Str(msg.id.clone())),
                    ("present", DiagValue::Bool(msg.present)),
                    ("kind", DiagValue::Str(kind)),
                ],
                epoch_ms(),
            );
            self.ind_handle(
                now,
                Event::Poll {
                    tab: msg.id.clone(),
                    present: msg.present,
                    kind: msg.kind,
                },
            );
            let step = arm_on_poll(&self.arm_of(&msg.id), msg.present, msg.kind);
            self.apply_arm(&msg.id, step);
        }
        self.settle(now);
    }

    /// Every deadline that is due: the indicator, the question watches, the diagnostics sweep.
    pub fn tick(&mut self) {
        let now = self.now();
        let screen = HandleScreen {
            handle: &*self.deps.handle,
            blind: &self.remote,
        };
        let effects = self.ind.tick(now, &screen);
        self.run_effects(effects);

        let due: Vec<String> = self
            .watches
            .iter()
            .filter(|(_, w)| w.next_due().is_some_and(|d| d <= now))
            .map(|(id, _)| id.clone())
            .collect();
        for tab in due {
            // Not Claude (yet): the screen is not read, and nothing is remembered as shown, so a
            // question that is still up when Claude is recognised is posted then.
            if !self.is_claude(&tab) {
                self.watches.remove(&tab);
                continue;
            }
            let Some(handle) = (self.deps.handle)(&tab) else {
                self.watches.remove(&tab);
                continue;
            };
            let shown = self
                .watches
                .get_mut(&tab)
                .and_then(|w| w.poll(now, &HandleRows(&handle)));
            if let Some(q) = shown
                && let Some(token) = (self.deps.token)(&tab)
            {
                (self.deps.post_question)(token, q);
            }
        }
        self.diag.tick(now);
        self.settle(now);
    }

    /// A tab that runs Claude Code (known by the poll or the title, or armed): only Claude asks through
    /// the question widget, so only its screen is believed (audit L22).
    fn is_claude(&self, tab: &str) -> bool {
        self.ind.agent_kind(tab) == Some(DetectedAgent::Claude)
            || flare_agent::keys::AgentView::armed(&self.arms, tab) == Some(AgentKind::Claude)
    }

    pub fn next_due(&self) -> Option<u64> {
        [
            self.ind.next_due(),
            self.watches.values().filter_map(|w| w.next_due()).min(),
            self.diag.next_due(),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// True once after the marks, an agent kind or a failed kind changed.
    /// The arm state as the keyboard path's `AgentView` (it follows the hub without borrowing it).
    pub fn view(&self) -> Rc<dyn flare_agent::keys::AgentView> {
        Rc::new(self.arms.clone())
    }

    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    /// The tabs whose agent badges and progress must go since the last take: the session changed or the agent
    /// left (plugins phase 1).
    pub fn take_decor_gone(&mut self) -> Vec<(String, DecorGone)> {
        std::mem::take(&mut self.decor_gone)
    }

    pub fn indicator(&self) -> &Indicator {
        &self.ind
    }

    pub fn agent_kind(&self, tab: &str) -> Option<DetectedAgent> {
        self.ind.agent_kind(tab)
    }

    /// The agent `tabs.json` saves for the tab: present AND resumable.
    pub fn saved_agent(&self, tab: &str) -> Option<AgentKind> {
        if !self.agent_here(tab) {
            return None;
        }
        self.ind.agent_kind(tab).and_then(DetectedAgent::resumable)
    }

    /// The Claude session the tab's agent is in, when the hooks (or a resume) named one and the tab's agent is not
    /// Codex or agy. Only Claude's `flare-status` hooks carry a session, so a session here is Claude's. Gone with the agent.
    pub fn claude_session(&self, tab: &str) -> Option<&str> {
        if matches!(
            self.ind.agent_kind(tab),
            Some(DetectedAgent::Codex | DetectedAgent::Agy)
        ) {
            return None;
        }
        self.sessions.get(tab).map(String::as_str)
    }

    /// The agent's hooks named the folder its session works in: the latest wins.
    pub fn set_agent_folder(&mut self, tab: &str, folder: &str) {
        // The first folder named is the session's start folder (rule 1240).
        self.agent_start_folders
            .entry(tab.to_string())
            .or_insert_with(|| folder.to_string());
        self.agent_folders
            .insert(tab.to_string(), folder.to_string());
    }

    /// The folder the tab's agent works in, while its hooks have named one and the agent has not left the tab.
    pub fn agent_folder(&self, tab: &str) -> Option<&str> {
        self.agent_folders.get(tab).map(String::as_str)
    }

    /// The folder the tab's agent session first named (rule 1240): a relative file link its agent folder lacks is
    /// tried there next. Gone with the agent, and forgotten when another session starts in the tab.
    pub fn agent_start_folder(&self, tab: &str) -> Option<&str> {
        self.agent_start_folders.get(tab).map(String::as_str)
    }

    /// True once the process poll has answered for the tab (the first word is said once).
    pub fn polled(&self, tab: &str) -> bool {
        self.polled.contains(tab)
    }

    pub fn agent_here(&self, tab: &str) -> bool {
        self.ind.agent_ids().contains(tab)
    }

    pub fn working(&self, tab: &str) -> bool {
        self.ind.working_ids().contains(tab)
    }

    pub fn working_for(&self, tab: &str) -> Option<u64> {
        self.ind.working_for(tab, self.now())
    }

    pub fn failed_kind(&self, tab: &str) -> Option<&str> {
        self.ind.failed_kind(tab)
    }

    pub fn attention(&self, sw: &Switches) -> Attention {
        attention_count(&AttentionIn {
            done: self.ind.done_ids(),
            question: self.ind.question_ids(),
            working: self.ind.working_ids(),
            failed: Some(self.ind.failed_ids()),
            done_on: sw.done_on,
            question_on: sw.question_on,
            failed_on: sw.failed_on,
        })
    }

    /// Tabs whose agent is working, for the window-close question.
    pub fn working_count(&self) -> usize {
        self.ind
            .working_ids()
            .iter()
            .filter(|id| self.ind.agent_ids().contains(*id))
            .count()
    }
}

impl flare_agent::keys::AgentView for AgentHub {
    fn armed(&self, tab: &str) -> Option<AgentKind> {
        flare_agent::keys::AgentView::armed(&self.arms, tab)
    }
}

/// The arm state of every tab. Shared (the keyboard path holds a clone as its `AgentView`), so it
/// reads the hub's state without borrowing the hub.
#[derive(Clone, Default)]
pub struct ArmMap(Rc<RefCell<HashMap<String, AgentArm>>>);

impl ArmMap {
    fn get(&self, tab: &str) -> Option<AgentArm> {
        self.0.borrow().get(tab).cloned()
    }

    fn insert(&self, tab: String, arm: AgentArm) {
        self.0.borrow_mut().insert(tab, arm);
    }

    fn remove(&self, tab: &str) {
        self.0.borrow_mut().remove(tab);
    }

    #[cfg(test)]
    fn contains_key(&self, tab: &str) -> bool {
        self.0.borrow().contains_key(tab)
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.0.borrow().is_empty()
    }
}

impl flare_agent::keys::AgentView for ArmMap {
    fn armed(&self, tab: &str) -> Option<AgentKind> {
        let arm = self.get(tab)?;
        if !arm.here {
            return None;
        }
        arm.kind.and_then(DetectedAgent::resumable)
    }
}

/// The tab an event is about.
fn event_tab(ev: &HostEvent) -> Option<&str> {
    match ev {
        HostEvent::Cwd { id, .. }
        | HostEvent::Exited { id, .. }
        | HostEvent::Started { id }
        | HostEvent::StartFailed { id }
        | HostEvent::Title { id, .. }
        | HostEvent::Bell { id }
        | HostEvent::ClipboardWrite { id, .. }
        | HostEvent::Osc777 { id, .. }
        | HostEvent::Osc7501 { id, .. }
        | HostEvent::Output { id }
        | HostEvent::Typed { id }
        | HostEvent::AnswerKey { id, .. }
        | HostEvent::Resized { id }
        | HostEvent::Revealed { id } => Some(id.as_str()),
    }
}

/// Milliseconds since the Unix epoch: the stamp a diag record carries.
fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// BLOCKING (a worker). The process rows the agent detection reads for the tabs whose shells are
/// `shells`: the native snapshot, command lines read under those shells only. On macOS each tab's
/// terminal foreground job (`tcgetpgrp`) is read too, and adopted under its shell when its parent is
/// gone (`flare_agent::detect::adopt_foreground`); Windows has no foreground job to ask (ConPTY), so
/// there it is the snapshot alone. None when the snapshot cannot be taken.
pub(crate) fn tab_process_rows(mgr: &SessionManager, shells: &[u32]) -> Option<Vec<ProcRow>> {
    // (shell, foreground leader) of every tab whose shell is asked about and whose foreground is another
    // process. Always empty on Windows (`foreground_pid` is None there).
    let fg: Vec<(u32, u32)> = mgr
        .live_pids()
        .into_iter()
        .filter(|(_, pid)| shells.contains(pid))
        .filter_map(|(id, pid)| mgr.foreground_pid(&id).map(|f| (pid, f)))
        .filter(|(pid, f)| *f != 0 && f != pid)
        .collect();
    let mut roots = shells.to_vec();
    roots.extend(fg.iter().map(|(_, f)| *f).filter(|f| !shells.contains(f)));
    let mut rows: Vec<ProcRow> =
        flare_win::procs::snapshot_processes(&roots, &flare_agent::detect::prefilter)?
            .into_iter()
            .map(|e| ProcRow {
                pid: e.pid,
                ppid: e.ppid,
                cmd: e.cmd,
            })
            .collect();
    for (shell, f) in fg {
        flare_agent::detect::adopt_foreground(&mut rows, shell, f);
    }
    Some(rows)
}

/// The poll's world over the real session manager and the native process snapshot. Built on the
/// worker thread.
fn real_poll_deps(mgr: Arc<SessionManager>, diag: Arc<dyn DiagSink>) -> PollDeps {
    let started = Instant::now();
    let for_pids = Arc::clone(&mgr);
    let for_query = Arc::clone(&mgr);
    let for_ticks = mgr;
    PollDeps {
        pids: Box::new(move || for_pids.live_pids()),
        ticks: Box::new(move || for_ticks.output_ticks()),
        query: Box::new(move |roots| tab_process_rows(&for_query, roots)),
        now: Box::new(move || started.elapsed().as_millis() as u64),
        failing: Some(Box::new(move || {
            log::warn!("the agent poll's process query failed three times");
            // The log reader's problems list shows it: stderr has no console in a GUI app, so without
            // this record nothing could say why the agent marks went missing (review #103).
            diag.record(
                "agent-poll-failing",
                &[("fails", DiagValue::Num(3))],
                epoch_ms(),
            );
        })),
    }
}

/// The state word a send tab reports, in Prism Terminal's `ledgerState` order: question, failed,
/// working, done. The sets can overlap (a Background phase keeps `working` while a question
/// mark is set), and a tab with an open question must never read as working, or dictated text
/// would land in the question box as its answer.
fn tab_state(ind: &Indicator, tab: &str) -> Option<&'static str> {
    state_word(
        ind.question_ids().contains(tab),
        ind.failed_ids().contains(tab),
        ind.working_ids().contains(tab),
        ind.done_ids().contains(tab),
    )
}

fn state_word(question: bool, failed: bool, working: bool, done: bool) -> Option<&'static str> {
    if question {
        Some("question")
    } else if failed {
        Some("failed")
    } else if working {
        Some("working")
    } else if done {
        Some("done")
    } else {
        None
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tab_host::TabHost;
    use flare_agent::keys::AgentView;
    use flare_agent::tab_mark::Indicator as MarkWord;
    use flare_diag::agent_diag::{DiagValue, MemorySink};
    use flare_term::fake::{FakeShells, FakeSpawner};
    use serde_json::Value;
    use std::cell::{Cell, RefCell};
    use std::sync::Mutex;

    const CLAUDE_TITLE: &str = "✳ Claude Code";

    #[derive(Default)]
    struct Rec {
        looks: Cell<usize>,
        soons: RefCell<Vec<u64>>,
        agains: RefCell<Vec<String>>,
    }

    struct RecPoll(Rc<Rec>);

    impl PollCtl for RecPoll {
        fn look_now(&self) {
            self.0.looks.set(self.0.looks.get() + 1);
        }
        fn soon(&self, ms: u64) {
            self.0.soons.borrow_mut().push(ms);
        }
        fn again(&self, id: &str) {
            self.0.agains.borrow_mut().push(id.to_string());
        }
    }

    struct Rig {
        host: Rc<RefCell<TabHost>>,
        sp: Arc<FakeSpawner>,
        hub: AgentHub,
        clock: Rc<Cell<u64>>,
        sink: Arc<MemorySink>,
        posts: Arc<Mutex<Vec<(String, ShownQuestion)>>>,
        off: Rc<RefCell<Vec<String>>>,
        rec: Rc<Rec>,
        handle_calls: Rc<Cell<usize>>,
        no_token: Rc<RefCell<HashSet<String>>>,
        remote: Arc<flare_term::fake::FakeRemote>,
    }

    fn sw() -> Switches {
        Switches {
            indicator: MarkWord::Ring,
            done_on: true,
            question_on: true,
            failed_on: true,
            rainbow: true,
        }
    }

    impl Rig {
        fn new() -> Self {
            Self::build(false)
        }

        /// The saved shell is a WSL distro.
        fn new_with_wsl() -> Self {
            Self::build(true)
        }

        fn build(wsl: bool) -> Self {
            let sp = FakeSpawner::new();
            let prefs = PrefStore::memory();
            if wsl {
                prefs.set("flare.term.shell", "wsl-Ubuntu");
            }
            let waker: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
            let shells = if wsl {
                FakeShells::with(vec![
                    flare_shells::list::ShellDef {
                        id: "pwsh".into(),
                        name: "PowerShell 7".into(),
                        exe: "pwsh.exe".into(),
                        args: Vec::new(),
                    },
                    flare_shells::list::ShellDef {
                        id: "wsl-Ubuntu".into(),
                        name: "Ubuntu".into(),
                        exe: "wsl.exe".into(),
                        args: Vec::new(),
                    },
                ])
            } else {
                FakeShells::new()
            };
            let remote = flare_term::fake::FakeRemote::new();
            let spawner = Arc::new(flare_term::pty::RoutingSpawner {
                local: sp.clone(),
                remote: remote.clone(),
            });
            let mut host = TabHost::new(spawner, shells, prefs.clone(), waker, None);
            host.set_dims(200, 60, None);
            let mgr = host.manager();
            let host = Rc::new(RefCell::new(host));
            let clock = Rc::new(Cell::new(1_000_000u64));
            let sink = Arc::new(MemorySink::default());
            let posts: Arc<Mutex<Vec<(String, ShownQuestion)>>> = Arc::new(Mutex::new(Vec::new()));
            let off = Rc::new(RefCell::new(Vec::new()));
            let handle_calls = Rc::new(Cell::new(0usize));
            let no_token: Rc<RefCell<HashSet<String>>> = Rc::new(RefCell::new(HashSet::new()));
            let (h2, c2, nt) = (
                Rc::clone(&host),
                Rc::clone(&handle_calls),
                Rc::clone(&no_token),
            );
            let posts2 = Arc::clone(&posts);
            let off2 = Rc::clone(&off);
            let clock2 = Rc::clone(&clock);
            let deps = HubDeps {
                prefs,
                now: Rc::new(move || clock2.get()),
                handle: Rc::new(move |id| {
                    c2.set(c2.get() + 1);
                    h2.borrow().handle(id)
                }),
                token: Rc::new(move |id| {
                    if nt.borrow().contains(id) {
                        None
                    } else {
                        mgr.tab_token(id)
                    }
                }),
                post_question: Arc::new(move |t, q| posts2.lock().unwrap().push((t, q))),
                diag: sink.clone(),
                wake: Arc::new(|| {}),
                reports_off: Rc::new(move |id| off2.borrow_mut().push(id.to_string())),
            };
            let mut hub = AgentHub::new(deps);
            let rec = Rc::new(Rec::default());
            hub.set_poll_ctl(Box::new(RecPoll(Rc::clone(&rec))));
            Self {
                host,
                sp,
                hub,
                clock,
                sink,
                posts,
                off,
                rec,
                handle_calls,
                no_token,
                remote,
            }
        }

        /// A remote tab, as the window opens one (remote sessions PR 3).
        fn open_remote(&mut self, id: &str) {
            self.host.borrow_mut().spawn_remote(
                id,
                flare_term::remote_pty::RemoteSpec {
                    machine: "mac".into(),
                    folder: "/Users/max/proj".into(),
                    resume: None,
                    live: flare_term::remote_pty::RemoteLive::default(),
                },
            );
            self.hub.on_spawned(id, "/Users/max/proj", None);
            self.hub.on_remote_tab(id);
            wait("the remote spawn", || {
                self.host.borrow().manager().is_remote(id)
            });
        }

        /// What the link pushes into a remote tab, pumped until its Output was handled.
        fn push_remote(&mut self, bytes: &[u8]) {
            let io = self.remote.made().last().expect("a remote tab").1.clone();
            io.push_output(bytes);
            let t = Instant::now();
            loop {
                let evs = self.host.borrow().drain_events();
                let got = evs.iter().any(|e| matches!(e, HostEvent::Output { .. }));
                self.hub.on_host_events(&evs);
                if got {
                    return;
                }
                assert!(t.elapsed() < Duration::from_secs(3), "no output event");
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        fn open(&mut self, id: &str, resume: Option<&str>) {
            self.host.borrow_mut().spawn(id, "C:\\proj", resume);
            self.hub.on_spawned(id, "C:\\proj", resume);
            let n = self.host.borrow().live_ids().len();
            wait("the spawn", || self.sp.made().len() >= n);
            wait("started", || {
                self.host.borrow().manager().live_pids().len() >= n
            });
        }

        fn pty(&self, id: &str) -> Arc<flare_term::fake::FakeState> {
            let idx = self
                .host
                .borrow()
                .live_ids()
                .iter()
                .position(|x| x == id)
                .unwrap();
            self.sp.made()[idx].clone()
        }

        /// Drains the host and feeds the hub; true when something came.
        fn pump(&mut self) -> bool {
            let evs = self.host.borrow().drain_events();
            let any = !evs.is_empty();
            self.hub.on_host_events(&evs);
            any
        }

        fn pump_until(&mut self, what: &str, f: impl Fn(&AgentHub) -> bool) {
            let t = Instant::now();
            while t.elapsed() < Duration::from_secs(3) {
                self.pump();
                if f(&self.hub) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            panic!("timed out waiting for {what}");
        }

        /// Feeds bytes and pumps until the Output event has come and been handled.
        fn print(&mut self, id: &str, bytes: &[u8]) {
            self.pty(id).feed(bytes);
            let t = Instant::now();
            loop {
                let evs = self.host.borrow().drain_events();
                let got = evs.iter().any(|e| matches!(e, HostEvent::Output { .. }));
                self.hub.on_host_events(&evs);
                if got {
                    return;
                }
                assert!(t.elapsed() < Duration::from_secs(3), "no output event");
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        fn advance(&mut self, ms: u64) {
            self.clock.set(self.clock.get() + ms);
            self.hub.tick();
        }

        fn ev(&mut self, ev: HostEvent) {
            self.hub.on_host_events(&[ev]);
        }

        fn title(&mut self, id: &str, text: &str) {
            self.ev(HostEvent::Title {
                id: id.into(),
                text: text.into(),
            });
        }

        fn lines(&self, k: &str) -> Vec<flare_diag::agent_diag::Line> {
            self.sink.lines().into_iter().filter(|l| l.k == k).collect()
        }
    }

    fn wait(what: &str, mut f: impl FnMut() -> bool) {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(3) {
            if f() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out waiting for {what}");
    }

    fn osc(state: &str) -> Vec<u8> {
        format!("\x1b]777;flare-agent;state={state}\x07").into_bytes()
    }

    fn screens(name: &str) -> Vec<String> {
        let all: Value = serde_json::from_str(include_str!(
            "../../flare-agent/tests/fixtures/question_widget_screens.json"
        ))
        .expect("fixture parses");
        all[name]
            .as_array()
            .expect("array")
            .iter()
            .map(|v| v.as_str().expect("row").to_string())
            .collect()
    }

    fn paint(rows: &[String]) -> Vec<u8> {
        // Clear the screen and home, then the rows.
        let mut s = String::from("\x1b[2J\x1b[H");
        s.push_str(&rows.join("\r\n"));
        s.into_bytes()
    }

    fn box_rows() -> Vec<String> {
        [
            "● Which colour do you prefer?",
            "",
            "  ❯ 1. Red",
            "    2. Blue",
            "    3. Type something.",
            "",
            "Enter to select · ↑/↓ to navigate · Esc to cancel",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    const ID: &str = "5a1d0c2e-1111-4222-8333-94445555a666";

    #[test]
    fn a_hook_signal_marks_a_background_tab_and_nothing_else_does() {
        let mut r = Rig::new();
        r.open("a", None);
        r.open("b", None);
        r.hub.set_looking(Some("b"), true);
        r.print("a", &osc("working"));
        r.print("a", &osc("done"));
        r.pump_until("a done", |h| h.indicator().done_ids().contains("a"));
        assert!(r.hub.agent_here("a"));
        assert!(!r.hub.indicator().done_ids().contains("b"));

        // Another program's OSC 777 changes nothing.
        r.print("b", b"\x1b]777;notify;Build;done\x07");
        assert!(!r.hub.agent_here("b"));
        assert!(r.hub.indicator().done_ids().len() == 1);
    }

    #[test]
    fn an_agy_tab_is_marked_from_its_osc_7501_reports() {
        let mut r = Rig::new();
        r.open("a", None);
        r.open("b", None);
        r.hub.set_looking(Some("b"), true);
        r.hub.queue_poll_answer("a", true, Some(DetectedAgent::Agy));
        r.hub.drain_poll();
        assert_eq!(r.hub.saved_agent("a"), Some(AgentKind::Agy));
        r.print("a", b"\x1b]7501;state=working:app=agy\x1b\\");
        r.pump_until("a working", |h| h.working("a"));
        r.print("a", b"\x1b]7501;app=agy:kind=permission:state=blocked\x07");
        r.pump_until("a asking", |h| h.question_tabs() == ["a"]);
        r.print("a", b"\x1b]7501;state=done:app=agy\x1b\\");
        r.pump_until("a done", |h| h.indicator().done_ids().contains("a"));
        assert!(r.hub.question_tabs().is_empty());
        assert_eq!(tab_of(&r.hub, "a").agent.as_deref(), Some("agy"));
        assert_eq!(r.hub.claude_session("a"), None);
        // A plain shell that reports lights nothing until something finds an agent there.
        r.print("b", b"\x1b]7501;state=working:app=cargo\x1b\\");
        r.advance(50);
        assert!(!r.hub.agent_here("b"));
    }

    const SID: &str = "b44e954e-a9cf-4ba9-a3c0-99ec6795d1d9";

    fn osc_session(state: &str, session: &str) -> Vec<u8> {
        format!("\x1b]777;flare-agent;state={state};session={session}\x07").into_bytes()
    }

    fn tab_of(hub: &AgentHub, id: &str) -> flare_dictation::route::SendTab {
        hub.send_tabs()
            .into_iter()
            .find(|t| t.tab == id)
            .unwrap_or_else(|| panic!("no send tab {id}"))
    }

    /// N25: the turn's own two hooks name the Claude session; the hub keeps it per tab and
    /// exposes it to the dictation rules.
    #[test]
    fn a_tab_learns_its_session_from_the_turn_hooks() {
        let mut r = Rig::new();
        r.open("a", None);
        r.open("b", None);
        assert_eq!(tab_of(&r.hub, "a").session, None);
        r.print("a", &osc_session("working", SID));
        let a = tab_of(&r.hub, "a");
        assert_eq!(a.session.as_deref(), Some(SID));
        assert_eq!(a.state.as_deref(), Some("working"));
        assert_eq!(a.cwd, "C:\\proj");
        assert!(!a.orchestrator);
        assert_eq!(a.copy, None);
        // The neighbour is untouched.
        assert_eq!(tab_of(&r.hub, "b").session, None);
        // A hook with no session (a tool's) keeps the one the turn named.
        r.print("a", &osc("working"));
        assert_eq!(tab_of(&r.hub, "a").session.as_deref(), Some(SID));
        // A session that is not a UUID is never taken, the state stands.
        r.print("b", &osc_session("working", "..\\x"));
        let b = tab_of(&r.hub, "b");
        assert_eq!(b.session, None);
        assert_eq!(b.state.as_deref(), Some("working"));
        // The session of a later turn replaces the old one.
        let next = "11111111-2222-4333-8444-555555555555";
        r.print("a", &osc_session("done", next));
        let a = tab_of(&r.hub, "a");
        assert_eq!(a.session.as_deref(), Some(next));
        assert_eq!(a.state.as_deref(), Some("done"));
    }

    #[test]
    fn a_session_goes_when_the_agent_leaves_or_the_tab_closes() {
        let mut r = Rig::new();
        r.open("a", None);
        r.open("b", None);
        r.title("a", CLAUDE_TITLE);
        r.print("a", &osc_session("done", SID));
        r.print("b", &osc_session("done", SID));
        assert_eq!(tab_of(&r.hub, "a").session.as_deref(), Some(SID));
        // The title goes empty: the agent left the tab.
        r.title("a", "");
        assert_eq!(tab_of(&r.hub, "a").session, None);
        r.hub.on_closed("b");
        assert!(r.hub.send_tabs().iter().all(|t| t.tab != "b"));
    }

    /// Rule 1240: the start folder is the first folder the session named; a later one moves only the agent folder.
    /// Another session, the agent leaving and the tab closing forget it.
    #[test]
    fn the_start_folder_is_the_first_folder_named_and_goes_with_the_agent() {
        let mut r = Rig::new();
        r.open("a", None);
        r.title("a", CLAUDE_TITLE);
        r.print("a", &osc_session("working", SID));
        assert_eq!(r.hub.agent_start_folder("a"), None);
        r.hub.set_agent_folder("a", "C:\\repo");
        r.hub.set_agent_folder("a", "C:\\repo\\sub");
        assert_eq!(r.hub.agent_folder("a"), Some("C:\\repo\\sub"));
        assert_eq!(r.hub.agent_start_folder("a"), Some("C:\\repo"));
        // Another session: its first folder is the new start.
        let next = "11111111-2222-4333-8444-555555555555";
        r.print("a", &osc_session("working", next));
        assert_eq!(r.hub.agent_start_folder("a"), None);
        r.hub.set_agent_folder("a", "C:\\other");
        assert_eq!(r.hub.agent_start_folder("a"), Some("C:\\other"));
        // The agent leaves.
        r.title("a", "");
        assert_eq!(r.hub.agent_start_folder("a"), None);
        // The tab closes.
        r.hub.set_agent_folder("a", "C:\\x");
        r.hub.on_closed("a");
        assert_eq!(r.hub.agent_start_folder("a"), None);
    }

    /// Plugins phase 1: the agent's decor belongs to its session. A first session is no change; another session,
    /// the agent leaving and a shell started again over a session each report the tab once.
    #[test]
    fn the_agent_decor_goes_when_the_session_changes_or_the_agent_leaves() {
        let mut r = Rig::new();
        r.open("a", None);
        r.title("a", CLAUDE_TITLE);
        r.print("a", &osc_session("working", SID));
        assert!(
            r.hub.take_decor_gone().is_empty(),
            "a first session clears nothing"
        );
        r.print("a", &osc_session("done", SID));
        assert!(
            r.hub.take_decor_gone().is_empty(),
            "the same session again clears nothing"
        );
        let next = "11111111-2222-4333-8444-555555555555";
        r.print("a", &osc_session("working", next));
        assert_eq!(
            r.hub.take_decor_gone(),
            [("a".to_string(), DecorGone::SessionChanged)]
        );
        // The title goes empty: the agent left the tab.
        r.title("a", "");
        assert_eq!(
            r.hub.take_decor_gone(),
            [("a".to_string(), DecorGone::AgentLeft)]
        );
        assert!(r.hub.take_decor_gone().is_empty(), "taken once");
        // A closed tab's reports go with it (the window clears a closed tab itself).
        r.open("b", None);
        r.title("b", CLAUDE_TITLE);
        r.print("b", &osc_session("working", SID));
        r.print("b", &osc_session("working", next));
        r.hub.on_closed("b");
        assert!(r.hub.take_decor_gone().is_empty());
    }

    #[test]
    fn a_restored_tab_takes_its_session_from_the_resume_id() {
        let mut r = Rig::new();
        r.open("a", Some(ID));
        let a = tab_of(&r.hub, "a");
        assert_eq!(a.session.as_deref(), Some(ID));
        assert_eq!(a.agent.as_deref(), Some("claude"));
        // Only a UUID, and not Codex's marker.
        r.open("b", Some("abcdef12"));
        assert_eq!(tab_of(&r.hub, "b").session, None);
        r.open("c", Some(flare_agent::kinds::CODEX_RESUME));
        assert_eq!(tab_of(&r.hub, "c").session, None);
        // The tabs come in the order they opened.
        let order: Vec<String> = r.hub.send_tabs().into_iter().map(|t| t.tab).collect();
        assert_eq!(order, vec!["a", "b", "c"]);
    }

    #[test]
    fn a_polled_claude_tab_names_its_agent_and_a_question_its_state() {
        let mut r = Rig::new();
        r.open("a", None);
        assert_eq!(tab_of(&r.hub, "a").agent, None);
        r.hub
            .queue_poll_answer("a", true, Some(DetectedAgent::Claude));
        r.hub.drain_poll();
        r.print("a", &osc("question"));
        let a = tab_of(&r.hub, "a");
        assert_eq!(a.agent.as_deref(), Some("claude"));
        assert_eq!(a.state.as_deref(), Some("question"));
    }

    #[test]
    fn a_tab_in_several_sets_reports_question_then_failed_then_working_then_done() {
        // PT ledgerState order; the sets overlap (Background keeps working under a question).
        assert_eq!(state_word(true, false, true, false), Some("question"));
        assert_eq!(state_word(true, true, true, true), Some("question"));
        assert_eq!(state_word(false, true, true, false), Some("failed"));
        assert_eq!(state_word(false, true, false, true), Some("failed"));
        assert_eq!(state_word(false, false, true, true), Some("working"));
        assert_eq!(state_word(false, false, false, true), Some("done"));
        assert_eq!(state_word(false, false, false, false), None);
    }

    #[test]
    fn a_failed_turn_reports_failed_though_it_is_also_done() {
        let mut r = Rig::new();
        r.open("a", None);
        r.open("b", None);
        r.hub.set_looking(Some("b"), true);
        r.print("a", &osc("working"));
        r.print("a", &osc("failed"));
        r.pump_until("a failed", |h| h.indicator().failed_ids().contains("a"));
        assert_eq!(tab_of(&r.hub, "a").state.as_deref(), Some("failed"));
    }
    #[test]
    fn the_neighbours_agent_signal_is_not_accepted() {
        let mut r = Rig::new();
        r.open("a", None);
        r.open("b", None);
        r.hub.set_looking(Some("b"), true);
        let other = ["pri", "sm"].concat();
        r.print(
            "a",
            format!("\x1b]777;{other}-agent;state=working\x07").as_bytes(),
        );
        r.print(
            "a",
            format!("\x1b]777;{other}-agent;state=done\x07").as_bytes(),
        );
        assert!(!r.hub.agent_here("a"));
        assert!(r.hub.indicator().working_ids().is_empty());
        assert!(r.hub.indicator().done_ids().is_empty());
    }

    #[test]
    fn a_claude_title_asks_the_poll_to_look() {
        let mut r = Rig::new();
        r.open("a", None);
        r.title("a", CLAUDE_TITLE);
        assert_eq!(r.rec.looks.get(), 1);
        r.advance(5_000);
        r.title("a", "✳ Claude Code ");
        r.title("a", CLAUDE_TITLE);
        assert_eq!(r.rec.looks.get(), 1);
    }

    #[test]
    fn a_hook_claim_asks_one_look_by_name_and_a_no_agent_answer_takes_it_back() {
        let mut r = Rig::new();
        r.open("a", None);
        // The poll already said "no agent" for this tab; a stray hook line then claims it.
        r.hub.queue_poll_answer("a", false, None);
        r.hub.drain_poll();
        r.rec.agains.borrow_mut().clear();
        let looks = r.rec.looks.get();
        r.print("a", &osc("question"));
        assert_eq!(*r.rec.agains.borrow(), vec!["a".to_string()]);
        assert_eq!(r.rec.looks.get(), looks + 1);
        // A second claim inside the gap asks for nothing more.
        r.print("a", &osc("working"));
        assert_eq!(r.rec.agains.borrow().len(), 1);
        assert_eq!(r.rec.looks.get(), looks + 1);
        // The one look answers "no agent" (forced, so it is sent though unchanged): the claim goes.
        r.hub.queue_poll_answer("a", false, None);
        r.hub.drain_poll();
        assert!(!r.hub.agent_here("a"));
        assert!(r.hub.indicator().question_ids().is_empty());
        assert!(r.hub.indicator().working_ids().is_empty());
    }

    #[test]
    fn a_wsl_tab_keeps_its_claim_because_no_one_asks_the_poll_about_it() {
        let mut r = Rig::new_with_wsl();
        r.open("a", None);
        wait("the shell id", || {
            r.host
                .borrow()
                .handle("a")
                .and_then(|h| h.shell_id())
                .is_some()
        });
        r.hub.queue_poll_answer("a", false, None);
        r.hub.drain_poll();
        r.rec.agains.borrow_mut().clear();
        let looks = r.rec.looks.get();
        r.print("a", &osc("question"));
        assert!(r.rec.agains.borrow().is_empty(), "no forced no for WSL");
        assert_eq!(r.rec.looks.get(), looks + 1);
        assert!(r.hub.agent_here("a"));
        assert!(!r.hub.indicator().question_ids().is_empty());
    }

    #[test]
    fn the_poll_arms_and_disarms_the_keys() {
        let mut r = Rig::new();
        r.open("a", None);
        assert_eq!(r.hub.armed("a"), None);
        r.hub
            .queue_poll_answer("a", true, Some(DetectedAgent::Claude));
        r.hub.drain_poll();
        assert_eq!(r.hub.armed("a"), Some(AgentKind::Claude));
        // The prompt comes back: nothing runs in front of it.
        r.ev(HostEvent::Cwd {
            id: "a".into(),
            path: "C:\\proj".into(),
        });
        assert_eq!(r.hub.armed("a"), None);
        assert_eq!(*r.rec.agains.borrow(), vec!["a".to_string()]);
    }

    #[test]
    fn the_poll_has_said_its_first_word_once_an_answer_is_applied() {
        let mut r = Rig::new();
        r.open("a", None);
        assert!(!r.hub.polled("a"), "nothing was said yet");
        r.hub.queue_poll_answer("a", false, None);
        r.hub.drain_poll();
        assert!(r.hub.polled("a"), "a no-agent verdict is a word too");
        assert!(!r.hub.polled("b"), "another tab has heard nothing");
        r.hub.on_closed("a");
        assert!(!r.hub.polled("a"), "a closed tab is forgotten");
    }

    #[test]
    fn a_title_armed_agent_is_armed_before_the_poll_and_leaves_with_its_title() {
        let mut r = Rig::new();
        r.open("a", None);
        r.title("a", CLAUDE_TITLE);
        assert_eq!(r.hub.armed("a"), Some(AgentKind::Claude));
        assert!(r.off.borrow().is_empty());
        r.title("a", "");
        assert_eq!(r.hub.armed("a"), None);
        assert_eq!(*r.off.borrow(), vec!["a".to_string()]);
        // Asked about by name twice: once when the title claimed the tab (review #47), once when it left.
        assert_eq!(
            *r.rec.agains.borrow(),
            vec!["a".to_string(), "a".to_string()]
        );
    }

    #[test]
    fn a_resume_arms_at_start_and_the_first_no_agent_is_doubted() {
        let mut r = Rig::new();
        r.open("a", Some(ID));
        assert_eq!(r.hub.armed("a"), Some(AgentKind::Claude));
        r.hub.queue_poll_answer("a", false, None);
        r.hub.drain_poll();
        assert_eq!(r.hub.armed("a"), Some(AgentKind::Claude));
        assert_eq!(*r.rec.agains.borrow(), vec!["a".to_string()]);
        r.hub.queue_poll_answer("a", false, None);
        r.hub.drain_poll();
        assert_eq!(r.hub.armed("a"), None);
    }

    #[test]
    fn a_resume_is_not_new_work() {
        let mut r = Rig::new();
        r.open("front", None);
        r.open("a", Some(ID));
        r.hub.set_looking(Some("front"), true);
        r.title("a", CLAUDE_TITLE);
        r.hub
            .queue_poll_answer("a", true, Some(DetectedAgent::Claude));
        r.hub.drain_poll();
        r.advance(3_000);
        let ind = r.hub.indicator();
        assert!(ind.done_ids().is_empty());
        assert!(ind.question_ids().is_empty());
        assert!(ind.failed_ids().is_empty());
        let restores = r.lines("agent-restore");
        assert_eq!(restores.len(), 1);
        assert_eq!(restores[0].text("cwd"), Some("C:\\proj"));
    }

    #[test]
    fn a_resume_painting_its_startup_is_not_work_and_a_real_prompt_is() {
        let mut r = Rig::new();
        r.open("front", None);
        r.open("a", Some(ID));
        r.open("b", None);
        r.hub.set_looking(Some("front"), true);
        // The real resume timeline in the background tab: the bare name, a loading spinner, then
        // the idle star, with the agent's paint (output) around each.
        r.title("a", "claude");
        r.ev(HostEvent::Output { id: "a".into() });
        r.advance(400);
        r.title("a", "\u{2802} Claude Code");
        r.ev(HostEvent::Output { id: "a".into() });
        r.advance(900);
        r.ev(HostEvent::Output { id: "a".into() });
        r.title("a", CLAUDE_TITLE);
        r.hub
            .queue_poll_answer("a", true, Some(DetectedAgent::Claude));
        r.hub.drain_poll();
        r.advance(3_000);
        let ind = r.hub.indicator();
        assert!(
            !ind.working_ids().contains("a"),
            "a startup paint is not work"
        );
        assert!(ind.done_ids().is_empty(), "nothing finished");
        assert!(ind.question_ids().is_empty());
        assert!(ind.failed_ids().is_empty());
        // The control: the same shape of tab, one that really answered a prompt, does finish.
        r.print("b", &osc("working"));
        r.print("b", &osc("done"));
        r.pump_until("b done", |h| h.indicator().done_ids().contains("b"));
        assert!(!r.hub.indicator().done_ids().contains("a"));
    }

    #[test]
    fn typing_and_a_refit_shape_the_scoring() {
        let mut r = Rig::new();
        r.open("front", None);
        r.open("a", None);
        r.hub.set_looking(Some("front"), true);
        r.hub
            .queue_poll_answer("a", true, Some(DetectedAgent::Claude));
        r.hub.drain_poll();
        // Typed then output right behind it: an echo, not work.
        r.advance(10_000);
        r.ev(HostEvent::Typed { id: "a".into() });
        r.advance(100);
        r.ev(HostEvent::Output { id: "a".into() });
        r.advance(1_300);
        r.ev(HostEvent::Output { id: "a".into() });
        assert!(!r.hub.working("a"));
        // A refit, then output inside 800 ms: not work either.
        r.advance(5_000);
        r.ev(HostEvent::Resized { id: "a".into() });
        r.advance(100);
        r.ev(HostEvent::Output { id: "a".into() });
        r.advance(600);
        r.ev(HostEvent::Output { id: "a".into() });
        assert!(!r.hub.working("a"));
    }

    #[test]
    fn an_answer_key_clears_a_question_when_the_box_is_gone() {
        let mut r = Rig::new();
        r.open("front", None);
        r.open("a", None);
        r.hub.set_looking(Some("front"), true);
        r.print("a", &paint(&box_rows()));
        r.print("a", &osc("question"));
        assert!(r.hub.indicator().question_ids().contains("a"));
        r.ev(HostEvent::AnswerKey {
            id: "a".into(),
            key: "\r".into(),
        });
        // The box is removed from the screen.
        r.print("a", b"\x1b[H\x1b[Jdone");
        r.advance(2_500);
        r.advance(100);
        assert!(!r.hub.indicator().question_ids().contains("a"));
    }

    #[test]
    fn a_plain_shell_cannot_forge_a_question_shown_post() {
        // Anything a program prints is on the screen: a shell tab that paints the question widget is not
        // Claude asking, so nothing reaches Sonara (audit L22).
        let mut r = Rig::new();
        r.open("a", None);
        r.print("a", &paint(&screens("run2-01-question-1")));
        r.advance(150);
        assert!(r.posts.lock().unwrap().is_empty());
        // A Codex tab asks nothing through AskUserQuestion either.
        r.hub
            .queue_poll_answer("a", true, Some(DetectedAgent::Codex));
        r.hub.drain_poll();
        r.print("a", &paint(&screens("run2-02-right")));
        r.advance(150);
        assert!(r.posts.lock().unwrap().is_empty());
    }

    #[test]
    fn the_posts_of_many_questions_share_one_worker_thread() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().to_string_lossy().into_owned();
        let started = Arc::new(AtomicUsize::new(0));
        let post = threaded_post_counted(
            Arc::new(move |k| (k == "SONARA_HOME").then(|| home.clone())),
            Duration::from_millis(50),
            Arc::clone(&started),
        );
        for i in 0..40 {
            post(format!("tab-{i}"), ShownQuestion::Text("Pick?".into()));
        }
        assert_eq!(started.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_question_widget_is_posted_once_per_change_with_the_tab_token() {
        let mut r = Rig::new();
        r.open("a", None);
        r.hub
            .queue_poll_answer("a", true, Some(DetectedAgent::Claude));
        r.hub.drain_poll();
        let token = r.host.borrow().manager().tab_token("a").expect("a token");
        r.print("a", &paint(&screens("run2-01-question-1")));
        r.advance(150);
        r.print("a", &paint(&screens("run2-02-right")));
        r.advance(150);
        // A redraw of the same question posts nothing.
        r.print("a", &paint(&screens("run2-02-right")));
        r.advance(150);
        let posts = r.posts.lock().unwrap().clone();
        assert_eq!(
            posts,
            vec![
                (token.clone(), ShownQuestion::Text("Pick a colour?".into())),
                (token, ShownQuestion::Text("Pick a size?".into())),
            ]
        );
        // A tab with no token posts nothing.
        r.open("b", None);
        r.hub
            .queue_poll_answer("b", true, Some(DetectedAgent::Claude));
        r.hub.drain_poll();
        r.no_token.borrow_mut().insert("b".into());
        r.print("b", &paint(&screens("run2-01-question-1")));
        r.advance(150);
        assert_eq!(r.posts.lock().unwrap().len(), 2);
    }

    #[test]
    fn under_e2e_the_sonara_env_names_only_the_sandbox_home() {
        let real = |k: &str| match k {
            "SONARA_HOME" => Some("C:\\sandbox".to_string()),
            "LOCALAPPDATA" => Some("C:\\owner".to_string()),
            _ => None,
        };
        // The sandbox home is read, so the post reaches the test's Sonara stand-in.
        let read = e2e_sonara_env(real);
        assert_eq!(
            flare_agent::sonara::sonara_home(&read),
            Some(std::path::PathBuf::from("C:\\sandbox"))
        );
        // With no sandbox home nothing falls back to the owner's %LOCALAPPDATA%\Sonara.
        let owner_only = |k: &str| (k == "LOCALAPPDATA").then(|| "C:\\owner".to_string());
        let read = e2e_sonara_env(owner_only);
        assert_eq!(flare_agent::sonara::sonara_home(&read), None);
    }

    #[test]
    fn sonara_is_never_called_on_the_ui_thread() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let dir = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::fs::write(
            dir.path().join("runtime.json"),
            format!(
                "{{\"http_port\":{port},\"token\":\"tok\",\"pid\":{}}}",
                std::process::id()
            ),
        )
        .unwrap();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let (body_tx, body_rx) = mpsc::channel::<String>();
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            // Bounded: a post that never comes fails the test instead of hanging it.
            let end = Instant::now() + Duration::from_secs(10);
            let mut s = loop {
                match listener.accept() {
                    Ok((s, _)) => break s,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < end =>
                    {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(e) => panic!("no post came: {e}"),
                }
            };
            s.set_nonblocking(false).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let mut data = Vec::new();
            let mut buf = [0u8; 1024];
            while !String::from_utf8_lossy(&data).contains("\"text\"") {
                let n = s.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                data.extend_from_slice(&buf[..n]);
            }
            body_tx
                .send(String::from_utf8_lossy(&data).to_string())
                .ok();
            // The server stays silent until the test says so.
            release_rx.recv().ok();
            let _ =
                s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}");
        });
        let home = dir.path().to_string_lossy().into_owned();
        let post = threaded_post(
            Arc::new(move |k| (k == "SONARA_HOME").then(|| home.clone())),
            Duration::from_secs(5),
        );
        let t = Instant::now();
        post("tab-token".into(), ShownQuestion::Text("Pick?".into()));
        assert!(
            t.elapsed() < Duration::from_millis(500),
            "the post waited for the server"
        );
        let got = body_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(got.contains("tab-token") && got.contains("Pick?"));
        release_tx.send(()).unwrap();
        server.join().unwrap();
    }

    #[test]
    fn saved_agent_is_present_and_resumable_only() {
        let mut r = Rig::new();
        r.open("c", None);
        r.open("x", None);
        r.open("o", None);
        r.open("n", None);
        r.hub
            .queue_poll_answer("c", true, Some(DetectedAgent::Claude));
        r.hub
            .queue_poll_answer("x", true, Some(DetectedAgent::Codex));
        r.hub
            .queue_poll_answer("o", true, Some(DetectedAgent::Other));
        r.hub.drain_poll();
        assert_eq!(r.hub.saved_agent("c"), Some(AgentKind::Claude));
        assert_eq!(r.hub.saved_agent("x"), Some(AgentKind::Codex));
        assert_eq!(r.hub.saved_agent("o"), None);
        assert_eq!(r.hub.saved_agent("n"), None);
        assert_eq!(r.hub.agent_kind("o"), Some(DetectedAgent::Other));
    }

    #[test]
    fn the_attention_count_follows_the_switches() {
        let mut r = Rig::new();
        r.open("front", None);
        for id in ["d", "q", "f"] {
            r.open(id, None);
        }
        r.hub.set_looking(Some("front"), true);
        r.print("d", &osc("working"));
        r.print("d", &osc("done"));
        r.print("q", &osc("working"));
        r.print("q", &osc("question"));
        r.print("f", &osc("working"));
        r.print("f", &osc("failed"));
        r.pump_until("three marks", |h| {
            let i = h.indicator();
            i.done_ids().contains("d")
                && i.question_ids().contains("q")
                && i.failed_ids().contains("f")
        });
        assert_eq!(r.hub.attention(&sw()).count, 3);
        let mut off = sw();
        off.done_on = false;
        assert_eq!(r.hub.attention(&off).count, 2);
        assert!(r.hub.attention(&sw()).question);
    }

    #[test]
    fn diagnostics_get_the_four_kinds_and_no_text() {
        let mut r = Rig::new();
        r.open("front", None);
        r.open("a", None);
        r.hub.set_looking(Some("front"), true);
        r.title("a", "✳ SECRET-TITLE-TEXT");
        r.print("a", &paint(&["SECRET-SCREEN-TEXT".to_string()]));
        r.print("a", &osc("working"));
        r.print("a", &osc("done"));
        r.pump_until("done", |h| h.indicator().done_ids().contains("a"));
        r.advance(100);
        let all = r.sink.lines();
        for k in ["agent-hook", "agent-title", "agent-mark"] {
            assert!(all.iter().any(|l| l.k == k), "no {k} line in {all:?}");
        }
        let text = format!("{all:?}");
        assert!(!text.contains("SECRET"), "text leaked: {text}");
        let marks = r.lines("agent-mark");
        assert!(marks.iter().all(|m| m.text("why").is_some()));
        assert!(
            marks
                .iter()
                .all(|m| matches!(m.get("why"), Some(DiagValue::Str(_))))
        );
    }

    #[test]
    fn a_closed_tab_leaves_nothing() {
        let mut r = Rig::new();
        r.open("front", None);
        r.open("a", None);
        r.hub.set_looking(Some("front"), true);
        r.print("a", &osc("working"));
        r.print("a", &osc("done"));
        r.pump_until("done", |h| h.indicator().done_ids().contains("a"));
        r.hub.on_closed("a");
        r.host.borrow_mut().kill("a");
        let i = r.hub.indicator();
        assert!(i.done_ids().is_empty() && i.agent_ids().is_empty());
        assert!(r.hub.arms.is_empty() || !r.hub.arms.contains_key("a"));
        assert!(!r.hub.watches.contains_key("a"));
        assert_eq!(i.activity.output_clocks(), 0);
        // A late poll answer for the closed tab is dropped.
        r.hub
            .queue_poll_answer("a", true, Some(DetectedAgent::Claude));
        r.hub.drain_poll();
        assert!(!r.hub.agent_here("a"));
        r.advance(120_000);
        assert_eq!(r.hub.next_due(), None);
    }

    #[test]
    fn an_output_flood_costs_one_pass_per_batch() {
        let mut r = Rig::new();
        r.open("front", None);
        r.open("a", None);
        r.hub.set_looking(Some("front"), true);
        r.print("a", &osc("working"));
        r.pump_until("hooked", |h| h.indicator().is_hooked("a"));
        let before = r.handle_calls.get();
        let flood: Vec<HostEvent> = (0..10_000)
            .map(|_| HostEvent::Output { id: "a".into() })
            .collect();
        r.hub.on_host_events(&flood);
        assert_eq!(
            r.handle_calls.get(),
            before,
            "the screen of a hooked session was read"
        );
    }

    #[test]
    fn the_poll_worker_is_stopped_with_the_hub() {
        let mut r = Rig::new();
        r.hub.start_poll(PollSource::Fake(Box::new(|| PollDeps {
            pids: Box::new(Vec::new),
            ticks: Box::new(|| 0),
            query: Box::new(|_| Some(Vec::new())),
            now: Box::new(|| 0),
            failing: None,
        })));
        let t = Instant::now();
        drop(r.hub);
        assert!(t.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn agent_view_is_the_arm_state_not_the_indicator() {
        let mut r = Rig::new();
        r.open("front", None);
        r.open("a", None);
        r.hub.set_looking(Some("front"), true);
        r.print("a", &osc("working"));
        r.pump_until("hooked", |h| h.agent_here("a"));
        assert!(r.hub.agent_here("a"));
        assert_eq!(r.hub.armed("a"), None);
        r.title("a", CLAUDE_TITLE);
        assert_eq!(r.hub.armed("a"), Some(AgentKind::Claude));
    }

    #[test]
    fn started_asks_for_a_warm_look() {
        let mut r = Rig::new();
        r.open("a", None);
        r.ev(HostEvent::Started { id: "a".into() });
        assert_eq!(*r.rec.soons.borrow(), vec![300]);
    }

    /// Rule 289: a remote tab's attach shows the host's screen (a snapshot, here with a question box on it) and what it
    /// missed (a catch-up whose signals were taken out): neither raises Finished or Question, nothing is posted, and the
    /// poll is never asked about the tab. Live output still marks it.
    #[test]
    fn a_remote_tabs_replay_raises_no_mark_and_live_output_does() {
        let mut r = Rig::new();
        r.open("b", None);
        r.open_remote("r");
        r.hub.set_looking(Some("b"), true);
        r.title("r", CLAUDE_TITLE);
        assert!(r.hub.is_remote("r") && r.hub.poll_blind("r"));
        let shot = [
            &b"\x1b[H\x1b[2J\x1b[3J"[..],
            box_rows().join("\r\n").as_bytes(),
        ]
        .concat();
        r.push_remote(&flare_term::remote_pty::plain_snapshot(&shot));
        let mut strip = flare_term::remote_pty::SignalStrip::new();
        let caught = strip.feed(&[osc("working"), osc("done")].concat(), true);
        r.push_remote(&[caught, b"caught up".to_vec()].concat());
        for _ in 0..40 {
            r.advance(250);
            r.pump();
        }
        assert!(r.hub.indicator().done_ids().is_empty(), "no Finished");
        assert!(r.hub.question_tabs().is_empty(), "no Question");
        assert!(!r.hub.question_up("r"));
        assert!(r.posts.lock().unwrap().is_empty(), "nothing posted");
        assert!(!r.rec.agains.borrow().iter().any(|t| t == "r"));
        r.push_remote(&osc("working"));
        r.pump_until("r working", |h| h.working("r"));
        r.hub.on_closed("r");
        assert!(!r.hub.is_remote("r"));
    }

    /// Rule 294: a remote tab's host status gives it the work and question marks, the question's words and the session;
    /// the end of its work is this viewer's own Finished mark, raised once, and the same status read again after a
    /// reconnect raises nothing. A local tab takes no host status.
    #[test]
    fn a_remote_tabs_host_status_marks_it_and_a_reconnect_raises_nothing_twice() {
        use flare_term::remote_pty::HostStatus;
        const SID: &str = "44444444-4444-4444-8444-444444444444";
        let st = |state: &str, question: Option<&str>| HostStatus {
            state: state.into(),
            agent: Some("claude".into()),
            session: Some(SID.into()),
            question: question.map(str::to_string),
            failed: None,
            host_sized: false,
        };
        let mut r = Rig::new();
        r.open("b", None);
        r.open_remote("r");
        r.hub.set_looking(Some("b"), true);
        r.hub.on_remote_status("r", &st("working", None));
        assert!(r.hub.working("r") && r.hub.agent_here("r"));
        assert_eq!(r.hub.claude_session("r"), Some(SID));
        assert_eq!(r.hub.working_count(), 1);
        r.hub
            .on_remote_status("r", &st("question", Some("Overwrite main.rs?")));
        assert_eq!(r.hub.question_tabs(), ["r"]);
        assert_eq!(
            r.hub.question_text("r").as_deref(),
            Some("Overwrite main.rs?")
        );
        r.hub.on_remote_status("r", &st("working", None));
        assert_eq!(r.hub.question_text("r"), None);
        r.hub.on_remote_status("r", &st("done", None));
        assert!(r.hub.indicator().done_ids().contains("r"));
        assert!(!r.hub.working("r"));
        // The person looks at it here: the mark goes. The link drops and attaches again and the host still says done.
        r.hub.set_looking(Some("r"), true);
        r.hub.set_looking(Some("b"), true);
        assert!(!r.hub.indicator().done_ids().contains("r"));
        r.hub.on_remote_status("r", &st("done", None));
        assert!(
            !r.hub.indicator().done_ids().contains("r"),
            "a reconnect does not raise done again"
        );
        // A local tab is the hub's own business.
        r.hub.on_remote_status("b", &st("working", None));
        assert!(!r.hub.working("b"));
    }

    /// Rule 298: an answer key here reads the mirrored box gone and takes the question down while the host's last word
    /// is still `question`. The host's next question (new words, no `working` seen between) raises the mark again.
    #[test]
    fn a_remote_question_answered_here_is_raised_again_by_the_hosts_next_question() {
        use flare_term::remote_pty::HostStatus;
        let st = |question: &str| HostStatus {
            state: "question".into(),
            agent: Some("claude".into()),
            session: None,
            question: Some(question.into()),
            failed: None,
            host_sized: false,
        };
        let mut r = Rig::new();
        r.open("b", None);
        r.open_remote("r");
        r.hub.set_looking(Some("b"), true);
        r.hub.on_remote_status("r", &st("Allow the edit?"));
        assert_eq!(r.hub.question_tabs(), ["r"]);
        r.push_remote(&paint(&box_rows()));
        r.ev(HostEvent::AnswerKey {
            id: "r".into(),
            key: "\r".into(),
        });
        r.push_remote(b"\x1b[H\x1b[Jok");
        r.advance(2_500);
        r.advance(100);
        assert!(
            r.hub.question_tabs().is_empty(),
            "the answer key took the question down here"
        );
        r.hub.on_remote_status("r", &st("Allow the command?"));
        assert_eq!(
            r.hub.question_tabs(),
            ["r"],
            "the host's next question is shown"
        );
        assert_eq!(
            r.hub.question_text("r").as_deref(),
            Some("Allow the command?")
        );
    }
}
