//! The engines: whisper-server, the Parakeet runner and the resident host (Tasks 13, 14).
//!
//! This is the engine of Tasks 13 and 14, ported from Prism Terminal's `dictationEngine.ts` (e5848cb).
//! Nothing runs until the first `transcribe`; after that ONE whisper-server is kept, replaced when the
//! model, the language or the engine kind it was started for is no longer what is being asked for, and
//! stood down after `idle` with no request (never, when `idle` is `None`, N5) or by `stop()`.
//!
//! PARAKEET is one process per pass (`parakeet.rs`) unless the host gives a RESIDENT host (`host.rs`, N6, N7):
//! one `flare-parakeet-host.exe` keeps the model loaded and answers every pass. A host that will not start
//! is not asked again this session (`host_broken`): parakeet-cli answers. A Whisper pass, `stop()` and a
//! pass timeout end the host; a stand-down ends it too.
//!
//! ENGINE CHOICE has no switch to get wrong: the GPU pack when it is installed AND an NVIDIA adapter is
//! present, else the bundled CPU engine. A GPU server that exits or never becomes ready at start is passed
//! over FOR THE LIFE OF THE ENGINE (`gpu_fell_back`), the CPU one is started in its place and answers the
//! same request: one attempt each, never a loop.
//!
//! PASSES ARE SERIALISED to one in flight, because the server is one model on one device. A final is
//! never dropped and goes ahead of any partial; a partial waiting when anything arrives is answered
//! `Superseded` without ever being sent (see `queue.rs`).
//!
//! The engine is a worker thread plus a state lock. `transcribe` decides under the lock which job is
//! next, so a pass that is asked for while the worker is idle is in flight before `transcribe` returns,
//! exactly as in the single-threaded original. The audio is never written to disk: it goes out as bytes
//! over a loopback socket to a port nobody else was told.

pub mod host;
pub mod http;
pub mod parakeet;
pub mod queue;
pub mod whisper;

use std::net::{Ipv4Addr, TcpListener};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::catalog::Runner;
use crate::job::Job;
use host::Host;
use parakeet::Pass;
use queue::Queue;
use whisper::{Answer, Server};

/// The first CUDA run compiles its kernels for about 9 s (MEASURED), so a start gets a minute.
pub const START_TIMEOUT: Duration = Duration::from_millis(60_000);
/// Five minutes idle and the server stands down (the default; Flare passes `None`, D11).
pub const IDLE: Duration = Duration::from_millis(300_000);
/// A pass that has not answered by now never will.
pub const REQUEST_TIMEOUT: Duration = Duration::from_millis(120_000);
/// The GPU engine's own kernel cache, with room (MEASURED: 31.8 s without cached kernels, 0.26 s with).
pub const CUDA_CACHE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// How long a server that would not start is not started again for a partial.
pub const LAUNCH_FAIL: Duration = Duration::from_millis(30_000);
/// How often a starting server is asked whether it is up yet.
const READY_POLL: Duration = Duration::from_millis(100);

/// Which engine folder answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineKind {
    Cpu,
    Gpu,
}

/// Why there is no text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    NoModel,
    NoEngine,
    EngineFailed,
    Superseded,
    Warming,
}

/// What a pass came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscribeResult {
    Ok(String),
    No {
        reason: Reason,
        detail: Option<String>,
    },
}

impl TranscribeResult {
    fn no(reason: Reason) -> Self {
        TranscribeResult::No {
            reason,
            detail: None,
        }
    }

    fn failed(detail: impl Into<String>) -> Self {
        TranscribeResult::No {
            reason: Reason::EngineFailed,
            detail: Some(detail.into()),
        }
    }

    /// The pass produced text.
    pub fn is_ok(&self) -> bool {
        matches!(self, TranscribeResult::Ok(_))
    }

    /// The text of a pass that produced some.
    pub fn text(&self) -> Option<&str> {
        match self {
            TranscribeResult::Ok(t) => Some(t),
            TranscribeResult::No { .. } => None,
        }
    }
}

/// One pass asked for.
#[derive(Debug, Clone)]
pub struct TranscribeRequest {
    pub wav: Vec<u8>,
    pub model_id: String,
    pub language: String,
    /// A final is what gets pasted; a partial is only the pill's live text.
    pub final_: bool,
}

/// What Settings may ask about the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineInfo {
    /// A server, a Parakeet pass or the resident Parakeet host is up.
    pub running: bool,
    /// The kind of the server that is up, or of the last one that was.
    pub engine: Option<EngineKind>,
    /// A GPU engine failed this session and the CPU one answers from now on.
    pub gpu_fell_back: bool,
}

/// A folder lookup the engine asks again on every pass (a pack finishes downloading mid-session).
pub type DirFn = Box<dyn Fn() -> Option<PathBuf> + Send + Sync>;

/// Where a model is, by id.
pub type ModelFn = Box<dyn Fn(&str) -> Option<PathBuf> + Send + Sync>;

/// Everything the engine is told, and the seams the tests use.
pub struct EngineConfig {
    /// The folder of the bundled CPU engine, or `None` when there is none.
    pub cpu_dir: DirFn,
    /// The folder of the unzipped GPU pack, or `None`.
    pub gpu_dir: DirFn,
    /// Where a model is, by id, or `None` when it is not installed.
    pub model_path: ModelFn,
    /// Which runner a model needs.
    pub runner_for: Box<dyn Fn(&str) -> Runner + Send + Sync>,
    /// An NVIDIA adapter is present (a question that cannot be answered reads as no).
    pub has_nvidia: Box<dyn Fn() -> bool + Send + Sync>,
    /// Stand down after this long with no request; `None` never stands down (D11, N5).
    pub idle: Option<Duration>,
    pub start_timeout: Duration,
    pub request_timeout: Duration,
    /// Where the GPU engine keeps its compiled kernels; `None` is the driver's shared cache.
    pub cuda_cache: Option<PathBuf>,
    /// `flare-parakeet-host.exe`; `None` runs one parakeet-cli per pass.
    pub parakeet_host: Option<PathBuf>,
    /// Threads for a Parakeet pass on the CPU.
    pub threads: u32,
    /// What to actually launch for an engine exe and its argv; the default is the exe itself.
    #[allow(clippy::type_complexity)]
    pub command: Option<Box<dyn Fn(&Path, &[String]) -> (PathBuf, Vec<String>) + Send + Sync>>,
}

impl EngineConfig {
    /// The engine as the app runs it, bar the folder lookups: the catalogue's runners, the real NVIDIA
    /// probe and the shipped timeouts. `idle` starts as the 300 s default; Flare sets `None` (D11).
    pub fn new(cpu_dir: DirFn, gpu_dir: DirFn, model_path: ModelFn) -> Self {
        EngineConfig {
            cpu_dir,
            gpu_dir,
            model_path,
            runner_for: Box::new(|id| {
                crate::catalog::model(id).map_or(Runner::Whisper, |m| m.runner)
            }),
            has_nvidia: Box::new(crate::gpu::has_nvidia),
            idle: Some(IDLE),
            start_timeout: START_TIMEOUT,
            request_timeout: REQUEST_TIMEOUT,
            cuda_cache: None,
            parakeet_host: None,
            threads: parakeet_threads(),
            command: None,
        }
    }
}

/// Parakeet's CPU threads: one per physical core (half the logical ones), at least 1, at most 16.
pub fn parakeet_threads() -> u32 {
    let logical = std::thread::available_parallelism().map_or(2, std::num::NonZeroUsize::get);
    u32::try_from((logical / 2).clamp(1, 16)).unwrap_or(1)
}

struct PassJob {
    req: TranscribeRequest,
    tx: Sender<TranscribeResult>,
    /// The silent pass that warms a server; nobody reads its text.
    warmup: bool,
}

struct LaunchFailed {
    key: String,
    until: Instant,
    detail: String,
}

struct State {
    queue: Queue<PassJob>,
    /// A job has been handed to the worker (or is in flight).
    running: bool,
    handoff: Option<PassJob>,
    /// A warm-up pass is queued or running.
    warming: bool,
    /// The last warm-up failed: partials then go the ordinary way, so the failure is reported.
    warm_failed: bool,
    launch_failed: Option<LaunchFailed>,
    gpu_fell_back: bool,
    last_kind: Option<EngineKind>,
    current: Option<Arc<Server>>,
    /// The resident Parakeet host, when one is up.
    phost: Option<Arc<Host>>,
    /// The resident host would not start: parakeet-cli for the rest of the session.
    host_broken: bool,
    /// The Parakeet pass in flight, so `stop()` can end it.
    pass: Option<Arc<Pass>>,
    /// The Parakeet model (by id) a pass last answered for: there is no resident process to be warm, but
    /// the first GPU pass compiles kernels (16 s, MEASURED) and a partial must not queue behind that.
    parakeet_warm: Option<String>,
    last_used: Instant,
    shutdown: bool,
}

struct Inner {
    cfg: EngineConfig,
    job: Arc<Job>,
    /// Bumped by `stop()`. Everything that waits compares it before and after: a server that went
    /// away because it was TOLD to is not a failed engine.
    epoch: AtomicU64,
    st: Mutex<State>,
    cv: Condvar,
    pids: Mutex<Vec<u32>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn stopped_result() -> TranscribeResult {
    TranscribeResult::failed("dictation was stopped")
}

fn launch_key(kind: EngineKind, model: &Path, language: &str) -> String {
    format!(
        "{kind:?}
{}
{language}",
        model.display()
    )
}

/// A port nobody is using: bind 0, read what Windows handed out, let go.
fn free_port() -> std::io::Result<u16> {
    let probe = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    Ok(probe.local_addr()?.port())
}

/// The Whisper engine. Dropping it stops it.
pub struct Engine {
    inner: Arc<Inner>,
    worker: Option<JoinHandle<()>>,
}

impl Engine {
    /// Starts the engine's worker thread. No child process exists until the first pass; every child
    /// joins `job`.
    pub fn start(cfg: EngineConfig, job: Arc<Job>) -> Self {
        let inner = Arc::new(Inner {
            cfg,
            job,
            epoch: AtomicU64::new(0),
            st: Mutex::new(State {
                queue: Queue::new(),
                running: false,
                handoff: None,
                warming: false,
                warm_failed: false,
                launch_failed: None,
                gpu_fell_back: false,
                last_kind: None,
                current: None,
                phost: None,
                host_broken: false,
                pass: None,
                parakeet_warm: None,
                last_used: Instant::now(),
                shutdown: false,
            }),
            cv: Condvar::new(),
            pids: Mutex::new(Vec::new()),
        });
        let for_worker = Arc::clone(&inner);
        let worker = std::thread::Builder::new()
            .name("flare-dictation-engine".to_string())
            .spawn(move || for_worker.work())
            .ok();
        Engine { inner, worker }
    }

    /// Asks for a pass. Returns at once; the answer arrives on the receiver. A partial on a cold
    /// engine is answered `Warming` at once and the warm-up is started, so the pill can say so.
    pub fn transcribe(&self, req: TranscribeRequest) -> Receiver<TranscribeResult> {
        let inner = &self.inner;
        let (tx, rx) = channel();
        let parakeet = (inner.cfg.runner_for)(&req.model_id) == Runner::Parakeet;
        let (warm_now, warm_failed) = {
            let st = lock(&inner.st);
            let warm_now = if parakeet {
                st.parakeet_warm.as_deref() == Some(req.model_id.as_str())
            } else {
                st.current.as_ref().is_some_and(|s| s.warm() && !s.exited())
            };
            (warm_now, st.warm_failed)
        };
        if !req.final_ && !warm_failed && !warm_now {
            drop(self.warm(&req.model_id, &req.language));
            let _ = tx.send(TranscribeResult::no(Reason::Warming));
            return rx;
        }
        let final_ = req.final_;
        let superseded = {
            let mut st = lock(&inner.st);
            let old = st.queue.push(
                PassJob {
                    req,
                    tx,
                    warmup: false,
                },
                final_,
            );
            inner.dispatch(&mut st);
            old
        };
        if let Some(old) = superseded {
            let _ = old.tx.send(TranscribeResult::no(Reason::Superseded));
        }
        rx
    }

    /// Queues the silent pass that warms a server for this model and language, unless one is already on
    /// its way (`Warming` at once) or the server up now is warm for it (`Ok` at once). It goes in the
    /// finals queue, so a real final asked for after it runs on the server it warmed.
    pub fn warm(&self, model_id: &str, language: &str) -> Receiver<TranscribeResult> {
        let inner = &self.inner;
        let (tx, rx) = channel();
        let lang = if language.is_empty() {
            "auto"
        } else {
            language
        };
        let parakeet = (inner.cfg.runner_for)(model_id) == Runner::Parakeet;
        let mut st = lock(&inner.st);
        if parakeet {
            if st.parakeet_warm.as_deref() == Some(model_id) {
                let _ = tx.send(TranscribeResult::Ok(String::new()));
                return rx;
            }
        } else if let Some(cur) = &st.current {
            if cur.warm() && !cur.exited() && cur.model_id == model_id && cur.language == lang {
                let _ = tx.send(TranscribeResult::Ok(String::new()));
                return rx;
            }
        }
        if st.warming {
            let _ = tx.send(TranscribeResult::no(Reason::Warming));
            return rx;
        }
        st.warming = true;
        st.queue.push_final_keeping_partial(PassJob {
            req: TranscribeRequest {
                wav: crate::wav::silent_warmup_wav(),
                model_id: model_id.to_string(),
                language: language.to_string(),
                final_: true,
            },
            tx,
            warmup: true,
        });
        inner.dispatch(&mut st);
        drop(st);
        rx
    }

    /// Ends everything now: the queue fails, the server is killed, a pass in flight fails. The engine
    /// stays usable; the next pass starts afresh.
    pub fn stop(&self) {
        self.inner.stop();
    }

    /// What Settings may ask.
    pub fn info(&self) -> EngineInfo {
        let st = lock(&self.inner.st);
        EngineInfo {
            running: st.current.as_ref().is_some_and(|s| !s.exited())
                || st.pass.is_some()
                || st.phost.is_some(),
            engine: st.last_kind,
            gpu_fell_back: st.gpu_fell_back,
        }
    }

    /// Every server pid this engine ever launched, in order (a test hook: leak checks).
    #[doc(hidden)]
    pub fn launched_pids(&self) -> Vec<u32> {
        lock(&self.inner.pids).clone()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        lock(&self.inner.st).shutdown = true;
        self.inner.stop();
        self.inner.cv.notify_all();
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

enum Want {
    Dir(EngineKind, PathBuf),
    Nothing,
}

impl Inner {
    /// Hands the next waiting job to the worker when it is free. Called under the state lock, so the
    /// decision "this pass is in flight" is made before the caller returns.
    fn dispatch(&self, st: &mut State) {
        if st.running || st.handoff.is_some() {
            return;
        }
        if let Some(job) = st.queue.pop() {
            st.running = true;
            st.handoff = Some(job);
            self.cv.notify_all();
        }
    }

    fn stopped_since(&self, mine: u64) -> bool {
        self.epoch.load(Ordering::SeqCst) != mine
    }

    /// The worker: takes a job, runs it, answers it, stands the server down when idle.
    fn work(&self) {
        loop {
            let job = {
                let mut st = lock(&self.st);
                loop {
                    if st.shutdown {
                        return;
                    }
                    if let Some(job) = st.handoff.take() {
                        break job;
                    }
                    let up = st.current.is_some() || st.phost.is_some();
                    let due = match (self.cfg.idle, up, st.running) {
                        (Some(idle), true, false) => Some(st.last_used + idle),
                        _ => None,
                    };
                    match due {
                        Some(due) if Instant::now() >= due => {
                            if let Some(cur) = st.current.take() {
                                cur.kill();
                            }
                            if let Some(host) = st.phost.take() {
                                host.kill();
                            }
                        }
                        Some(due) => {
                            let wait = due.saturating_duration_since(Instant::now());
                            st = self
                                .cv
                                .wait_timeout(st, wait)
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .0;
                        }
                        None => {
                            st = self
                                .cv
                                .wait(st)
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                        }
                    }
                }
            };
            let began = self.epoch.load(Ordering::SeqCst);
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| self.run(&job.req)))
                .unwrap_or_else(|_| {
                    TranscribeResult::failed("the dictation engine hit an internal error")
                });
            {
                let mut st = lock(&self.st);
                st.running = false;
                if job.warmup {
                    st.warming = false;
                    // A warm-up that a stop() cut short says nothing about the engine.
                    if !self.stopped_since(began) {
                        st.warm_failed = !result.is_ok();
                    }
                }
                st.last_used = Instant::now();
            }
            let _ = job.tx.send(result);
            let mut st = lock(&self.st);
            self.dispatch(&mut st);
        }
    }

    fn stop(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        let (jobs, current, pass, host) = {
            let mut st = lock(&self.st);
            st.warming = false;
            st.warm_failed = false;
            st.launch_failed = None;
            // Off means off for Parakeet too: the next arm warms again.
            st.parakeet_warm = None;
            let mut jobs = st.queue.drain();
            if let Some(handed) = st.handoff.take() {
                st.running = false;
                jobs.push(handed);
            }
            (jobs, st.current.take(), st.pass.take(), st.phost.take())
        };
        for job in jobs {
            let _ = job.tx.send(stopped_result());
        }
        // The pass in flight answers for itself: its server going away wakes it, and it sees the
        // epoch has moved.
        if let Some(cur) = current {
            cur.kill();
        }
        // A Parakeet pass running now is ended, not left to finish.
        if let Some(pass) = pass {
            pass.kill();
        }
        if let Some(host) = host {
            host.kill();
        }
    }

    /// Ends the resident Parakeet host, if there is one.
    fn kill_phost(&self) {
        let host = lock(&self.st).phost.take();
        if let Some(host) = host {
            host.kill();
        }
    }

    fn kill_host(&self, h: &Arc<Host>) {
        {
            let mut st = lock(&self.st);
            if st.phost.as_ref().is_some_and(|c| Arc::ptr_eq(c, h)) {
                st.phost = None;
            }
        }
        h.kill();
    }

    /// What to actually launch for an engine exe and its argv.
    fn command_for(&self, exe: &Path, args: &[String]) -> (PathBuf, Vec<String>) {
        match &self.cfg.command {
            Some(command) => command(exe, args),
            None => (exe.to_path_buf(), args.to_vec()),
        }
    }

    fn kill_server(&self, s: &Arc<Server>) {
        {
            let mut st = lock(&self.st);
            if st.current.as_ref().is_some_and(|c| Arc::ptr_eq(c, s)) {
                st.current = None;
            }
        }
        s.kill();
    }

    /// The GPU pack when it is installed AND an NVIDIA adapter is present, else the bundled CPU engine.
    fn pick(&self) -> Want {
        let fell_back = lock(&self.st).gpu_fell_back;
        if !fell_back {
            if let Some(dir) = (self.cfg.gpu_dir)() {
                if (self.cfg.has_nvidia)() {
                    return Want::Dir(EngineKind::Gpu, dir);
                }
            }
        }
        match (self.cfg.cpu_dir)() {
            Some(dir) => Want::Dir(EngineKind::Cpu, dir),
            None => Want::Nothing,
        }
    }

    /// The child's environment. The C++ runtime is beside the CPU engine; the GPU pack is the official
    /// zip and carries none, so the CPU engine's folder goes on the front of PATH. The GPU engine also
    /// gets its own kernel cache.
    fn child_env(&self, kind: EngineKind, dir: &Path) -> Vec<(String, String)> {
        let mut env = Vec::new();
        if let Some(runtime) = (self.cfg.cpu_dir)() {
            if runtime != dir {
                let old = std::env::var("PATH").unwrap_or_default();
                env.push(("PATH".to_string(), format!("{};{old}", runtime.display())));
            }
        }
        if kind == EngineKind::Gpu {
            if let Some(cache) = &self.cfg.cuda_cache {
                env.push((
                    "CUDA_CACHE_PATH".to_string(),
                    cache.to_string_lossy().into_owned(),
                ));
                env.push((
                    "CUDA_CACHE_MAXSIZE".to_string(),
                    CUDA_CACHE_BYTES.to_string(),
                ));
            }
        }
        env
    }

    /// One pass: the whole decision of PT's `run`.
    fn run(&self, req: &TranscribeRequest) -> TranscribeResult {
        let mine = self.epoch.load(Ordering::SeqCst);
        let stopped = || self.stopped_since(mine);

        let Some(model_path) = (self.cfg.model_path)(&req.model_id) else {
            return TranscribeResult::no(Reason::NoModel);
        };
        if (self.cfg.runner_for)(&req.model_id) == Runner::Parakeet {
            return self.run_parakeet(req, &model_path, mine);
        }
        let language = if req.language.is_empty() {
            "auto".to_string()
        } else {
            req.language.clone()
        };
        // A resident Parakeet host holds a model nobody uses now.
        self.kill_phost();

        let Want::Dir(want_kind, want_dir) = self.pick() else {
            return TranscribeResult::no(Reason::NoEngine);
        };
        if stopped() {
            return stopped_result();
        }

        let mut server = lock(&self.st).current.clone();
        if let Some(s) = &server {
            if s.exited()
                || s.kind != want_kind
                || s.model_path != model_path
                || s.language != language
            {
                // The server holds ONE model and ONE language for its whole life: both are argv.
                self.kill_server(s);
                server = None;
            }
        }
        let server = match server {
            Some(s) => s,
            None => {
                if !req.final_ {
                    let st = lock(&self.st);
                    if let Some(failed) = &st.launch_failed {
                        if failed.key == launch_key(want_kind, &model_path, &language)
                            && Instant::now() < failed.until
                        {
                            return TranscribeResult::failed(failed.detail.clone());
                        }
                    }
                }
                let mut started = self.launch(
                    want_kind,
                    &want_dir,
                    &model_path,
                    &req.model_id,
                    &language,
                    mine,
                );
                if stopped() {
                    return stopped_result();
                }
                if started.is_err() && want_kind == EngineKind::Gpu {
                    // Once, and for good: a pack that will not start on this machine now will not start
                    // on the next press either, and each try can cost the whole start timeout.
                    lock(&self.st).gpu_fell_back = true;
                    let Some(cpu_dir) = (self.cfg.cpu_dir)() else {
                        return TranscribeResult::failed(
                            started
                                .err()
                                .unwrap_or_else(|| "the server did not start".into()),
                        );
                    };
                    started = self.launch(
                        EngineKind::Cpu,
                        &cpu_dir,
                        &model_path,
                        &req.model_id,
                        &language,
                        mine,
                    );
                    if stopped() {
                        return stopped_result();
                    }
                }
                match started {
                    Ok(s) => {
                        lock(&self.st).launch_failed = None;
                        s
                    }
                    Err(detail) => {
                        let mut st = lock(&self.st);
                        // Keyed by what the NEXT pick will ask for: after a GPU fallback, the CPU.
                        let kind = if want_kind == EngineKind::Gpu && st.gpu_fell_back {
                            EngineKind::Cpu
                        } else {
                            want_kind
                        };
                        st.launch_failed = Some(LaunchFailed {
                            key: launch_key(kind, &model_path, &language),
                            until: Instant::now() + LAUNCH_FAIL,
                            detail: detail.clone(),
                        });
                        return TranscribeResult::failed(detail);
                    }
                }
            }
        };
        lock(&self.st).last_kind = Some(server.kind);

        let answer = self.post(&server, &req.wav);
        if stopped() {
            return stopped_result();
        }
        match answer {
            Answer::Text(text) => {
                server.set_warm();
                TranscribeResult::Ok(text)
            }
            Answer::Failed { detail, died } => {
                // A GPU server that started and then died UNDER A PASS (a model too big for the card's
                // memory does exactly this) would die under the next one too. This request is lost
                // either way; the next goes to the CPU engine.
                if died && server.kind == EngineKind::Gpu {
                    lock(&self.st).gpu_fell_back = true;
                }
                TranscribeResult::failed(detail)
            }
        }
    }

    /// A pass for a Parakeet model: the same choice of engine and the same once-and-for-good fall back to
    /// the CPU as a Whisper server gets (PT's `runParakeet`).
    fn run_parakeet(
        &self,
        req: &TranscribeRequest,
        model_path: &Path,
        mine: u64,
    ) -> TranscribeResult {
        let stopped = || self.stopped_since(mine);
        // A resident Whisper server holds another model in memory nobody uses now.
        let server = lock(&self.st).current.take();
        if let Some(server) = server {
            server.kill();
        }
        let Want::Dir(mut kind, dir) = self.pick() else {
            return TranscribeResult::no(Reason::NoEngine);
        };
        if stopped() {
            return stopped_result();
        }
        let mut answer = self.parakeet_pass(kind, &dir, model_path, &req.wav, mine);
        if stopped() {
            return stopped_result();
        }
        // Only a GPU engine that DIED falls back, Whisper's own rule: an "error:" about the audio or a
        // timeout is not the card's fault, and the switch holds for the session and for Whisper too.
        if let (Answer::Failed { detail, died }, EngineKind::Gpu) = (&answer, kind) {
            if !died {
                lock(&self.st).last_kind = Some(kind);
                return TranscribeResult::failed(detail.clone());
            }
            lock(&self.st).gpu_fell_back = true;
            let Some(cpu_dir) = (self.cfg.cpu_dir)() else {
                return TranscribeResult::failed(detail.clone());
            };
            kind = EngineKind::Cpu;
            answer = self.parakeet_pass(kind, &cpu_dir, model_path, &req.wav, mine);
            if stopped() {
                return stopped_result();
            }
        }
        let mut st = lock(&self.st);
        st.last_kind = Some(kind);
        match answer {
            Answer::Text(text) => {
                st.parakeet_warm = Some(req.model_id.clone());
                TranscribeResult::Ok(text)
            }
            Answer::Failed { detail, .. } => TranscribeResult::failed(detail),
        }
    }

    /// One pass, on the resident host when there is one, else a fresh parakeet-cli.
    fn parakeet_pass(
        &self,
        kind: EngineKind,
        dir: &Path,
        model: &Path,
        wav: &[u8],
        mine: u64,
    ) -> Answer {
        if let Some(answer) = self.host_once(kind, dir, model, wav, mine) {
            return answer;
        }
        let args = parakeet::pass_args(model, self.cfg.threads, kind);
        let (file, args) = self.command_for(&dir.join("parakeet-cli.exe"), &args);
        let env = self.child_env(kind, dir);
        let spec = parakeet::Spec {
            file: &file,
            args: &args,
            dir,
            env: &env,
            wav,
            timeout: self.cfg.request_timeout,
        };
        // sync-ok: runs on the engine's worker thread, never the UI thread
        let answer = parakeet::run(&spec, &self.job, &|pass| {
            lock(&self.st).pass = Some(Arc::clone(pass));
            lock(&self.pids).push(pass.pid);
            if self.stopped_since(mine) {
                // stop() ran before this pass was findable: it did not see this one.
                pass.kill();
            }
        });
        let mut st = lock(&self.st);
        st.pass = None;
        answer
    }

    /// One pass on the resident host: started (or restarted for another model, folder or engine kind) as
    /// needed. `None` when there is no host to use: parakeet-cli then (PT's `hostOnce`).
    fn host_once(
        &self,
        kind: EngineKind,
        dir: &Path,
        model: &Path,
        wav: &[u8],
        mine: u64,
    ) -> Option<Answer> {
        let exe = self.cfg.parakeet_host.as_deref()?;
        let current = {
            let st = lock(&self.st);
            if st.host_broken {
                return None;
            }
            st.phost.clone()
        };
        let mut host = current;
        if let Some(h) = &host {
            if h.exited() || h.kind != kind || h.dir != dir || h.model_path != model {
                self.kill_host(h);
                host = None;
            }
        }
        let host = match host {
            Some(h) => h,
            None => {
                let args = [
                    dir.to_string_lossy().into_owned(),
                    model.to_string_lossy().into_owned(),
                    if kind == EngineKind::Gpu { "1" } else { "0" }.to_string(),
                    self.cfg.threads.to_string(),
                ];
                let (file, args) = self.command_for(exe, &args);
                let env = self.child_env(kind, dir);
                // sync-ok: runs on the engine's worker thread, never the UI thread
                let spawned = Host::spawn(
                    &file,
                    &args,
                    dir,
                    &env,
                    &self.job,
                    (kind, dir.to_path_buf(), model.to_path_buf()),
                );
                let Ok(h) = spawned else {
                    // The host cannot be launched at all (or cannot join the job): parakeet-cli answers
                    // from now on, and says why if it cannot either.
                    lock(&self.st).host_broken = true;
                    return None;
                };
                // Current from the moment it exists, so a stop() during the start kills it.
                lock(&self.st).phost = Some(Arc::clone(&h));
                lock(&self.pids).push(h.pid);
                if self.stopped_since(mine) {
                    self.kill_host(&h);
                    return Some(Answer::Failed {
                        detail: "dictation was stopped".to_string(),
                        died: false,
                    });
                }
                if !h.wait_ready(self.cfg.start_timeout) {
                    self.kill_host(&h);
                    if self.stopped_since(mine) {
                        // A host that went away because it was TOLD to is not a broken host.
                        return Some(Answer::Failed {
                            detail: "dictation was stopped".to_string(),
                            died: false,
                        });
                    }
                    // Never ready: the host itself or the engine. The per-pass path says which, and the
                    // host is not tried again.
                    lock(&self.st).host_broken = true;
                    return None;
                }
                h
            }
        };
        let answer = host.pass(wav, self.cfg.request_timeout);
        if host.was_killed() {
            // A pass that timed out ended the host: the next pass starts a fresh one.
            self.kill_host(&host);
        }
        Some(answer)
    }

    /// Starts a server and waits until it answers GET /. `Err` carries the reason, in the words the
    /// user sees.
    fn launch(
        &self,
        kind: EngineKind,
        dir: &Path,
        model_path: &Path,
        model_id: &str,
        language: &str,
        mine: u64,
    ) -> Result<Arc<Server>, String> {
        let port = free_port().map_err(|e| format!("no free port: {e}"))?;
        if self.stopped_since(mine) {
            return Err("stopped".to_string());
        }
        let args: Vec<String> = [
            "-m",
            &model_path.to_string_lossy(),
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "-l",
            language,
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
        let (file, args) = self.command_for(&dir.join("whisper-server.exe"), &args);
        let env = self.child_env(kind, dir);
        // sync-ok: runs on the engine's worker thread, never the UI thread
        let server = Server::spawn(
            &file,
            &args,
            dir,
            &env,
            &self.job,
            (
                kind,
                model_path.to_path_buf(),
                model_id.to_string(),
                language.to_string(),
                port,
            ),
        )
        .map_err(|e| e.to_string())?;
        // Current from the moment it exists, so a stop() during the start kills it.
        lock(&self.st).current = Some(Arc::clone(&server));
        lock(&self.pids).push(server.pid);
        if self.stopped_since(mine) {
            // stop() ran between the spawn and the line above: it did not see this one.
            self.kill_server(&server);
            return Err("stopped".to_string());
        }

        let deadline = Instant::now() + self.cfg.start_timeout;
        loop {
            if server.exited() {
                return Err(server.why_it_died());
            }
            if http::get_status(port, Duration::from_secs(1)).is_ok_and(|code| code < 500) {
                // Something answered; make sure it was not the last thing it did.
                if server.exited() {
                    return Err(server.why_it_died());
                }
                return Ok(server);
            }
            if self.stopped_since(mine) {
                self.kill_server(&server);
                return Err("stopped".to_string());
            }
            if Instant::now() >= deadline {
                self.kill_server(&server);
                let said = server.stderr();
                let ms = self.cfg.start_timeout.as_millis();
                return Err(if said.is_empty() {
                    format!("the server was not ready after {ms} ms")
                } else {
                    format!("the server was not ready after {ms} ms: {said}")
                });
            }
            server.wait_exit(READY_POLL);
        }
    }

    /// One pass on a running server: POST /inference.
    fn post(&self, s: &Arc<Server>, wav: &[u8]) -> Answer {
        if s.exited() {
            return Answer::Failed {
                detail: s.why_it_died(),
                died: true,
            };
        }
        let (body, content_type) = whisper::multipart(wav);
        let deadline = Instant::now() + self.cfg.request_timeout;
        match http::post(
            s.port,
            "/inference",
            &content_type,
            &body,
            &|| s.exited(),
            deadline,
        ) {
            Ok(resp) => whisper::read_answer(resp.status, &resp.body),
            Err(http::PostError::Aborted) => Answer::Failed {
                detail: s.why_it_died(),
                died: true,
            },
            Err(http::PostError::Timeout) => {
                self.kill_server(s);
                Answer::Failed {
                    detail: format!(
                        "no answer after {} ms",
                        self.cfg.request_timeout.as_millis()
                    ),
                    died: false,
                }
            }
            Err(http::PostError::Io(e)) => {
                // A dying server is seen TWICE: the socket resets at once, and the process is reported
                // gone a moment later with its stderr read out. The second is the one worth telling the
                // user, so a socket error waits a second for it. A socket error from a server that stays
                // up means it cannot be trusted with the next pass either, so it is replaced.
                if s.wait_exit(Duration::from_secs(1)) {
                    Answer::Failed {
                        detail: s.why_it_died(),
                        died: true,
                    }
                } else {
                    self.kill_server(s);
                    Answer::Failed {
                        detail: e.to_string(),
                        died: false,
                    }
                }
            }
        }
    }
}
