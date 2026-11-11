//! M9 Task 31: DiagCtl, the diagnostics log, the watchdog and the crumbs.
//!
//! One place builds the running app's diagnostics: the log (`<profile>\logs\diag.jsonl`), the session
//! line, the panic hooks, the stall watchdog (a native thread; the UI thread only beats a 50 ms timer),
//! the memory watch, the work timer and the crumb ring. The rest of the app reaches them through four
//! plain functions: [`crumb`], [`timed`], [`session_end`] and [`quit`]. Before [`start`] ran, or after
//! [`quit`], every one of them does nothing (the work simply runs), so no caller needs a guard.
//!
//! Rules: the log never throws and never waits on the UI thread (rule 74); the quit line is the last line
//! of the file (rule 110); a sleep is not a stall (rule 111); the log owns `diag.json` (rule 85).

use std::path::Path;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

use flare_diag::agent_diag::{DiagSink, DiagValue};
use flare_diag::crash::CrashHooks;
use flare_diag::mem::{MemWatch, Metrics, MetricsSource};
use flare_diag::session::{Diagnostics, SessionInfo};
use flare_diag::stack::NoSampler;
#[cfg(any(windows, all(target_os = "macos", target_arch = "aarch64")))]
use flare_diag::stack::{FrameSource, MAX_FRAMES, Win32Sampler};
use flare_diag::stall::{
    CrumbRing, StackSampler, StallDeps, StallWatch, SystemClock, WorkClass, WorkTable, WorkTimer,
};
use flare_diag::summary::{clamp_time, sink_allows};
use flare_diag::writer::{DiagLog, DiagLogOptions, LineSink};
use serde_json::{Map, Value};

/// What `crumb` is given: a name and the fields that go with it.
pub type CrumbFields<'a> = &'a [(&'a str, Value)];

/// The facts of this PC that the `session` line carries.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SystemFacts {
    pub windows: String,
    pub cpus: u32,
    pub cpu: String,
    pub ram_gb: u32,
    pub pid: u32,
    pub runtime: String,
}

impl SystemFacts {
    /// Reads them now: the registry (read-only) for the CPU name, `GlobalMemoryStatusEx` for the memory,
    /// `available_parallelism` for the cores. Microseconds; run at startup, before the window.
    pub fn gather() -> Self {
        let os = flare_win::os_build::os_version();
        let cpu = flare_win::registry::LocalMachineRead::get_string(
            "HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0",
            "ProcessorNameString",
        )
        .map(|s| s.trim().to_owned())
        .unwrap_or_default();
        SystemFacts {
            windows: format!("{}.{}.{}", os.major, os.minor, os.build),
            cpus: std::thread::available_parallelism().map_or(1, |n| n.get() as u32),
            cpu,
            ram_gb: installed_ram_gb(),
            pid: std::process::id(),
            runtime: format!("rust {}", std::env::consts::ARCH),
        }
    }
}

/// Physical memory in whole GB (rounded); 0 when the call fails.
#[cfg(windows)]
fn installed_ram_gb() -> u32 {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: a zeroed MEMORYSTATUSEX with its length set is the documented input of the call.
    unsafe {
        let mut m: MEMORYSTATUSEX = std::mem::zeroed();
        m.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
        if GlobalMemoryStatusEx(&mut m) == 0 {
            return 0;
        }
        ((m.ullTotalPhys + (1 << 29)) >> 30) as u32
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The agent code's diagnostics sink over the log (M6's [`DiagSink`]): only the kinds an agent may say,
/// stamped `app`, with a sane time.
pub struct SinkAdapter(pub DiagLog);

impl DiagSink for SinkAdapter {
    fn record(&self, kind: &str, fields: &[(&str, DiagValue)], at_ms: u64) {
        if !sink_allows(kind, self.0.verbose()) {
            return;
        }
        let mut m = Map::new();
        for (k, v) in fields {
            m.insert((*k).to_owned(), value_of(v));
        }
        let now = epoch_ms();
        self.0.write_at("app", kind, clamp_time(at_ms, now), m);
    }
}

fn value_of(v: &DiagValue) -> Value {
    match v {
        DiagValue::Str(s) => Value::String(s.clone()),
        DiagValue::Num(n) => Value::from(*n),
        DiagValue::Bool(b) => Value::Bool(*b),
        DiagValue::List(l) => Value::Array(l.iter().cloned().map(Value::String).collect()),
    }
}

fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// The sink the agent hub writes to: the running app's log (the null log before `start`).
pub fn agent_sink() -> Arc<dyn DiagSink> {
    Arc::new(SinkAdapter(flare_diag::writer::main()))
}

/// The pids of the live shells, asked of the tab host on a worker.
pub type ShellPids = Arc<dyn Fn() -> Vec<u32> + Send + Sync>;

/// This process's and the shells' memory, for the memory watch.
struct ProcMetrics {
    pids: Mutex<Option<ShellPids>>,
}

impl MetricsSource for ProcMetrics {
    fn read(&self) -> Option<Metrics> {
        let mine = crate::memory::process_memory()?;
        let pids = lock(&self.pids).clone();
        let children: u64 = pids
            .map(|f| f().into_iter().filter_map(private_bytes_of).sum())
            .unwrap_or(0);
        Some(Metrics {
            app_mb: (mine.private as u64) >> 20,
            gpu_mb: None,
            children_mb: children >> 20,
            working_set_mb: Some((mine.working_set as u64) >> 20),
        })
    }
}

/// Private bytes of another process of ours (a shell), or None when it cannot be opened.
#[cfg(windows)]
fn private_bytes_of(pid: u32) -> Option<u64> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: OpenProcess takes plain values; a null handle is checked; the counters are a zeroed
    // plain-data struct with `cb` set; the handle is closed once.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut c: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        c.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        let ok = GetProcessMemoryInfo(h, (&mut c as *mut PROCESS_MEMORY_COUNTERS_EX).cast(), c.cb);
        CloseHandle(h);
        (ok != 0).then_some(c.PrivateUsage as u64)
    }
}

/// The UI thread's stack through `flare-win`'s sampler (the Mac one on Apple silicon, rule 1116).
#[cfg(any(windows, all(target_os = "macos", target_arch = "aarch64")))]
struct UiFrames(flare_win::thread_stack::ThreadSampler);

#[cfg(any(windows, all(target_os = "macos", target_arch = "aarch64")))]
impl FrameSource for UiFrames {
    fn raw_frames(&self) -> Option<([u64; MAX_FRAMES], usize)> {
        self.0.raw_frames()
    }

    fn describe(&self, addr: u64) -> String {
        flare_win::thread_stack::describe(addr)
    }
}

/// A flag a worker sleeps on, so `stop` wakes it at once.
#[derive(Default)]
struct Stopper {
    stopped: Mutex<bool>,
    wake: Condvar,
}

impl Stopper {
    /// Waits `ms` or until `stop`; true when stopped.
    fn pause(&self, ms: u64) -> bool {
        let g = lock(&self.stopped);
        if *g {
            return true;
        }
        let (g, _) = self
            .wake
            .wait_timeout(g, Duration::from_millis(ms)) // sync-ok: the hung-window poll thread waits for its next look
            .unwrap_or_else(PoisonError::into_inner);
        *g
    }

    fn stop(&self) {
        *lock(&self.stopped) = true;
        self.wake.notify_all();
    }
}

/// What the free functions reach: one per running app.
struct Globals {
    log: DiagLog,
    diagnostics: Arc<Diagnostics>,
    watch: StallWatch,
    crumbs: Arc<CrumbRing>,
    work: Arc<WorkTimer>,
    hwnd: Arc<AtomicIsize>,
    metrics: Arc<ProcMetrics>,
}

static GLOBAL: RwLock<Option<Arc<Globals>>> = RwLock::new(None);
static RUNNING: Mutex<Option<DiagCtl>> = Mutex::new(None);

fn globals() -> Option<Arc<Globals>> {
    GLOBAL
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// The running app's diagnostics. The handles it keeps are the ones the tests (and a later owner of
/// the value) reach; the app talks to the free functions.
#[cfg_attr(not(test), allow(dead_code))]
pub struct DiagCtl {
    pub log: DiagLog,
    diagnostics: Arc<Diagnostics>,
    crash: Option<CrashHooks>,
    watch: Arc<StallWatch>,
    mem: Arc<MemWatch>,
    crumbs: Arc<CrumbRing>,
    work: Arc<WorkTimer>,
    hung: Arc<Stopper>,
    threads: Vec<JoinHandle<()>>,
}

/// Builds the diagnostics for the profile folder `profile`: `<profile>\logs` is the log folder (a null
/// log when it cannot be made), the session line is written first, the panic hooks are installed, the
/// watchdog and the memory watch run on their own threads. `backend` is the renderer's name, `e2e` marks
/// the session as a test run. Also makes the free functions live.
pub fn start(profile: &Path, facts: SystemFacts, backend: &str, e2e: bool) -> DiagCtl {
    let dir = crate::startup::logs_dir_of(profile);
    // sync-ok: startup, before any window; a folder that cannot be made gives the null log
    let log = if std::fs::create_dir_all(&dir).is_ok() {
        DiagLog::start(DiagLogOptions::new(dir))
    } else {
        DiagLog::null()
    };
    flare_diag::writer::set_main(Some(log.clone()));
    let info = SessionInfo {
        version: crate::app_facts::version().to_owned(),
        runtime: facts.runtime,
        gpu: backend.to_owned(),
        windows: facts.windows,
        pid: facts.pid,
        verbose: log.verbose(),
        cpus: facts.cpus,
        cpu: facts.cpu,
        ram_gb: facts.ram_gb,
        e2e,
    };
    let diagnostics = Arc::new(Diagnostics::start(log.clone(), &info));
    let crash = CrashHooks::install(log.clone());

    let sink: Arc<dyn LineSink> = Arc::new(log.clone());
    let clock = Arc::new(SystemClock);
    let table = Arc::new(WorkTable::new());
    let verbose: Arc<dyn Fn() -> bool + Send + Sync> = {
        let log = log.clone();
        Arc::new(move || log.verbose())
    };
    let work = Arc::new(WorkTimer::new(
        Arc::clone(&table),
        clock.clone(),
        Arc::clone(&sink),
        verbose,
    ));
    let crumbs = Arc::new(CrumbRing::new());
    // The canary measures how long one small disk call takes (a stalled disk shows as `fs-slow`).
    let probe = profile.to_path_buf();
    let canary: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        let _ = std::fs::metadata(&probe); // sync-ok: the canary thread exists to time this call
    });
    let mut deps = StallDeps::new(Arc::clone(&sink), table, clock, Arc::clone(&crumbs), canary);
    deps.sampler = Some(ui_sampler());
    let watch = StallWatch::new(deps);
    let mut threads = vec![watch.clone().spawn()];
    let watch = Arc::new(watch);

    let metrics = Arc::new(ProcMetrics {
        pids: Mutex::new(None),
    });
    let mem = Arc::new(MemWatch::new(Arc::clone(&sink), metrics.clone()));
    threads.push(Arc::clone(&mem).spawn());

    let hwnd = Arc::new(AtomicIsize::new(0));
    let hung = Arc::new(Stopper::default());
    {
        let (hwnd, hung, watch) = (hwnd.clone(), hung.clone(), watch.clone());
        let spawned = std::thread::Builder::new()
            .name("flare-hung-poll".into())
            .spawn(move || {
                while !hung.pause(500) {
                    let h = hwnd.load(Ordering::SeqCst);
                    if h != 0 {
                        watch.hung(is_hung(h));
                    }
                }
            });
        if let Ok(t) = spawned {
            threads.push(t);
        }
    }

    *GLOBAL.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(Globals {
        log: log.clone(),
        diagnostics: Arc::clone(&diagnostics),
        watch: (*watch).clone(),
        crumbs: Arc::clone(&crumbs),
        work: Arc::clone(&work),
        hwnd,
        metrics,
    }));
    if let Some(v) = crate::ship_ctl::FIRST_RUN.get() {
        crumb("first-run", &[("version", Value::from(v.as_str()))]);
    }
    DiagCtl {
        log,
        diagnostics,
        crash: Some(crash),
        watch,
        mem,
        crumbs,
        work,
        hung,
        threads,
    }
}

/// The UI thread's sampler, or the safe empty one when the thread cannot be sampled. Made on the UI
/// thread (the thread that calls `start`).
#[cfg(windows)]
fn ui_sampler() -> Arc<dyn StackSampler> {
    match flare_win::thread_stack::ThreadSampler::for_current_thread() {
        Ok(s) => Arc::new(Win32Sampler(UiFrames(s))),
        Err(e) => {
            log::debug!("no ui stack sampler: {e}");
            Arc::new(NoSampler)
        }
    }
}

#[cfg(windows)]
fn is_hung(hwnd: isize) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::IsHungAppWindow;
    // SAFETY: IsHungAppWindow accepts any handle value and only reads window state.
    unsafe { IsHungAppWindow(hwnd as _) != 0 }
}

/// macOS: physical memory in whole GB (rounded), from `sysctl hw.memsize`; 0 when it cannot be read.
#[cfg(not(windows))]
fn installed_ram_gb() -> u32 {
    flare_proc::installed_ram().map_or(0, |b| {
        u32::try_from((b + (1 << 29)) >> 30).unwrap_or(u32::MAX)
    })
}

/// macOS: the physical footprint of another process of ours (a shell), or None when it is gone.
#[cfg(not(windows))]
fn private_bytes_of(pid: u32) -> Option<u64> {
    flare_proc::memory(pid).map(|m| m.footprint)
}

/// macOS on Apple silicon (rule 1116): the UI thread's frame-pointer walk, as on Windows.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn ui_sampler() -> Arc<dyn StackSampler> {
    match flare_win::thread_stack::ThreadSampler::for_current_thread() {
        Ok(s) => Arc::new(Win32Sampler(UiFrames(s))),
        Err(e) => {
            log::debug!("no ui stack sampler: {e}");
            Arc::new(NoSampler)
        }
    }
}

/// An Intel Mac keeps no sampler: the stall line names the work in flight alone.
#[cfg(all(not(windows), not(all(target_os = "macos", target_arch = "aarch64"))))]
fn ui_sampler() -> Arc<dyn StackSampler> {
    Arc::new(NoSampler)
}

/// macOS: phase 3 of the port plan. A window is never reported hung until then.
#[cfg(not(windows))]
fn is_hung(_hwnd: isize) -> bool {
    false
}

impl DiagCtl {
    /// Runs `f` as the work item `name` (the global [`timed`], for a control that owns the value).
    #[cfg(test)]
    pub fn timed<R>(&self, name: &str, f: impl FnOnce() -> R) -> R {
        self.work.time(name, true, WorkClass::Normal, f)
    }

    /// The watchdog, for the tests that beat it by hand.
    #[cfg(test)]
    pub fn watch(&self) -> &StallWatch {
        &self.watch
    }

    /// The crumb ring.
    #[cfg(test)]
    pub fn crumbs(&self) -> &CrumbRing {
        &self.crumbs
    }

    /// The end of the run: the watchers stop, then `quit` is the LAST line and the log closes.
    pub fn stop(mut self) {
        self.watch.stop();
        self.mem.stop();
        self.hung.stop();
        // The watchers end within a tick; waiting for them keeps a late line from landing after `quit`.
        for t in self.threads.drain(..) {
            let _ = t.join(); // sync-ok: the end of life, each worker wakes at once on stop
        }
        {
            let mut g = GLOBAL.write().unwrap_or_else(PoisonError::into_inner);
            *g = None;
        }
        self.diagnostics.stop();
        if let Some(c) = self.crash.take() {
            c.uninstall();
        }
        flare_diag::writer::set_main(None);
    }
}

/// Parks the running diagnostics where [`quit`] finds it (the app's one instance).
pub fn install(ctl: DiagCtl) {
    *lock(&RUNNING) = Some(ctl);
}

/// The end of life: AppCtl calls it LAST, after the shells are gone, so the `quit` line closes the file.
/// Idempotent; nothing running is nothing to do.
pub fn quit() {
    let ctl = lock(&RUNNING).take();
    if let Some(c) = ctl {
        c.stop();
    }
}

/// Windows is ending the session: `session-end`, written and flushed at once.
pub fn session_end() {
    if let Some(g) = globals() {
        g.diagnostics.session_end();
    }
}

/// Runs `f` as the work item `name` on the UI thread (slow from 100 ms, panics logged and resumed). A
/// plain call before `start` and after `quit`.
pub fn timed<R>(name: &str, f: impl FnOnce() -> R) -> R {
    match globals() {
        Some(g) => g.work.time(name, true, WorkClass::Normal, f),
        None => f(),
    }
}

/// `timed` for work that waits on the user or a download (the folder picker, an update install): never
/// slow, never in `inflight`.
pub fn timed_waits<R>(name: &str, f: impl FnOnce() -> R) -> R {
    match globals() {
        Some(g) => g.work.time(name, true, WorkClass::Waits, f),
        None => f(),
    }
}

/// A breadcrumb: kept in the ring (the watchdog names the last ones beside a slow frame) and written to
/// the log as kind `crumb` with `a` first.
pub fn crumb(a: &str, fields: CrumbFields<'_>) {
    crumb_with(a, fields, false);
}

/// A crumb that can fire many times a second (a tab switch with a key held): in the ring always, in the
/// log only while verbose.
pub fn crumb_often(a: &str, fields: CrumbFields<'_>) {
    crumb_with(a, fields, true);
}

fn crumb_with(a: &str, fields: CrumbFields<'_>, often: bool) {
    let Some(g) = globals() else { return };
    let mut m = Map::new();
    m.insert("a".to_owned(), Value::String(a.to_owned()));
    for (k, v) in fields {
        m.insert((*k).to_owned(), v.clone());
    }
    g.crumbs
        .push(flare_diag::stall::Clock::now_ms(&SystemClock), a, m.clone());
    if !often || g.log.verbose() {
        g.log.write("app", "crumb", m);
    }
}

/// A text field of a crumb.
pub fn txt(v: &str) -> Value {
    Value::String(v.to_owned())
}

/// The window exists: the hung-window poll starts looking at it.
pub fn set_window(hwnd: isize) {
    if let Some(g) = globals() {
        g.hwnd.store(hwnd, Ordering::SeqCst);
    }
}

/// The tab host's live shell pids, for the memory watch's `childrenMb`.
pub fn set_shell_pids(pids: ShellPids) {
    if let Some(g) = globals() {
        *lock(&g.metrics.pids) = Some(pids);
    }
}

/// The 50 ms timer on the UI thread that tells the watchdog the thread is alive. Keep the timer alive.
pub fn beat_timer() -> Option<slint::Timer> {
    let g = globals()?;
    let watch = g.watch.clone();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(50),
        move || watch.beat(),
    );
    Some(timer)
}

/// What the machine's sleep and its screen tell the watchdog (a sleep or a screen that is off is not a stall).
pub fn power_events() -> Option<flare_win::session_end::PowerEvents> {
    let g = globals()?;
    let (a, b, c) = (g.watch.clone(), g.watch.clone(), g.watch.clone());
    Some(flare_win::session_end::PowerEvents {
        on_suspend: Box::new(move || a.power_suspend()),
        on_resume: Box::new(move || b.power_resume()),
        on_screen: {
            // Windows says the current state when the watch registers: only a change is a crumb.
            let last = std::sync::atomic::AtomicU8::new(u8::MAX);
            Box::new(move |on| {
                c.screen(on);
                let was = last.swap(u8::from(on), Ordering::SeqCst);
                if was != u8::MAX && was != u8::from(on) {
                    crumb("screen", &[("on", Value::Bool(on))]);
                }
            })
        },
    })
}

/// The first-run, kill and shell crumbs that come from crates that do not know the log.
pub fn install_kill_hook() {
    let _ = flare_term::session::KILL_STUCK_HOOK.set(|pid, ms| {
        crumb(
            "kill-stuck",
            &[("pid", Value::from(pid)), ("ms", Value::from(ms))],
        );
    });
    // A child that stopped reading drops every write after the first: one crumb a second says so.
    let _ = flare_term::session::WRITE_DROP_HOOK.set(|pid, len| {
        static LAST: Mutex<Option<std::time::Instant>> = Mutex::new(None);
        let mut last = LAST.lock().unwrap_or_else(PoisonError::into_inner);
        let now = std::time::Instant::now();
        if last.is_some_and(|t| now.duration_since(t) < Duration::from_secs(1)) {
            return;
        }
        *last = Some(now);
        crumb(
            "pty-write-dropped",
            &[("pid", Value::from(pid)), ("len", Value::from(len))],
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_diag::writer::DIAG_FILE;
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;

    /// `start` makes process-wide state (the globals, the panic hook): one test at a time.
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

    fn facts() -> SystemFacts {
        SystemFacts {
            windows: "10.0.26300".into(),
            cpus: 8,
            cpu: "Test CPU".into(),
            ram_gb: 32,
            pid: 4242,
            runtime: "rust test".into(),
        }
    }

    fn lines(profile: &Path) -> Vec<Value> {
        std::fs::read_to_string(profile.join("logs").join(DIAG_FILE)) // sync-ok: test code
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).expect("every line is one JSON object"))
            .collect()
    }

    fn kinds(lines: &[Value]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l["k"].as_str().unwrap_or_default().to_owned())
            .collect()
    }

    fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(5) {
            if ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20)); // sync-ok: test code
        }
        panic!("timed out waiting for {what}");
    }

    #[test]
    fn start_writes_a_session_line_and_the_last_line_is_quit() {
        let _g = lock(&ONE_AT_A_TIME);
        let dir = tempfile::tempdir().unwrap();
        let ctl = start(dir.path(), facts(), "software", true);
        crumb("tab-open", &[("id", txt("a"))]);
        ctl.log.flush();
        ctl.stop();
        let l = lines(dir.path());
        let k = kinds(&l);
        assert_eq!(k.first().map(String::as_str), Some("session"));
        assert_eq!(k.last().map(String::as_str), Some("quit"));
        assert!(k.contains(&"crumb".to_owned()));
        assert_eq!(l[0]["gpu"], "software");
        assert_eq!(l[0]["e2e"], true);
        assert!(l.iter().all(|x| x["src"] == "app" || x["src"] == "main"));
    }

    #[test]
    fn an_idle_start_writes_nothing_else_for_three_seconds() {
        let _g = lock(&ONE_AT_A_TIME);
        let dir = tempfile::tempdir().unwrap();
        let ctl = start(dir.path(), facts(), "software", false);
        wait_for("the memory crumb", || {
            ctl.log.flush();
            kinds(&lines(dir.path())).contains(&"crumb".to_owned())
        });
        let first = lines(dir.path()).len();
        std::thread::sleep(Duration::from_secs(3)); // sync-ok: test code, the idle window is the test
        ctl.log.flush();
        let l = lines(dir.path());
        assert_eq!(l.len(), first, "three idle seconds wrote {:?}", kinds(&l));
        // Only the session line and the memory crumb exist at start.
        assert!(kinds(&l).iter().all(|k| k == "session" || k == "crumb"));
        ctl.stop();
    }

    #[test]
    fn a_busy_ui_thread_gives_main_lag_with_the_items_running() {
        let _g = lock(&ONE_AT_A_TIME);
        let dir = tempfile::tempdir().unwrap();
        let ctl = start(dir.path(), facts(), "software", false);
        ctl.watch().beat();
        ctl.timed("x", || {
            std::thread::sleep(Duration::from_millis(300)); // sync-ok: test code, the busy thread
        });
        ctl.watch().beat();
        wait_for("main-lag", || {
            ctl.log.flush();
            kinds(&lines(dir.path())).contains(&"main-lag".to_owned())
        });
        let l = lines(dir.path());
        let lag = l.iter().find(|x| x["k"] == "main-lag").unwrap();
        assert!(lag["ms"].as_u64().unwrap() >= 200);
        assert!(
            lag["inflight"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["name"] == "x"),
            "{lag}"
        );
        ctl.stop();
    }

    #[test]
    fn a_slow_work_item_logs_work_slow_with_its_name() {
        let _g = lock(&ONE_AT_A_TIME);
        let dir = tempfile::tempdir().unwrap();
        let ctl = start(dir.path(), facts(), "software", false);
        timed("on_slow_thing", || {
            std::thread::sleep(Duration::from_millis(150)); // sync-ok: test code
        });
        timed("on_quick_thing", || {});
        ctl.log.flush();
        ctl.stop();
        let l = lines(dir.path());
        let slow: Vec<_> = l.iter().filter(|x| x["k"] == "work-slow").collect();
        assert_eq!(slow.len(), 1);
        assert_eq!(slow[0]["name"], "on_slow_thing");
        assert!(slow[0]["ms"].as_u64().unwrap() >= 100);
    }

    #[test]
    fn a_panic_in_a_worker_logs_panic_with_a_stack() {
        let _g = lock(&ONE_AT_A_TIME);
        let dir = tempfile::tempdir().unwrap();
        let ctl = start(dir.path(), facts(), "software", false);
        let t = std::thread::spawn(|| {
            panic!("a worker broke");
        });
        assert!(t.join().is_err()); // sync-ok: test code
        ctl.log.flush();
        ctl.stop();
        let l = lines(dir.path());
        let p = l.iter().find(|x| x["k"] == "panic").expect("a panic line");
        // Audit L12: the log keeps the payload's length and hash, never its text.
        let text = p.to_string();
        assert!(
            text.contains("panic payload withheld (14 bytes, hash "),
            "{p}"
        );
        assert!(!text.contains("a worker broke"), "{p}");
        assert!(p.get("stack").is_some(), "{p}");
    }

    #[test]
    fn opening_diagnostics_is_a_crumb() {
        let _g = lock(&ONE_AT_A_TIME);
        let dir = tempfile::tempdir().unwrap();
        let ctl = start(dir.path(), facts(), "software", false);
        crumb("settings-page", &[("page", txt("diagnostics"))]);
        ctl.log.flush();
        ctl.stop();
        let l = lines(dir.path());
        let c = l
            .iter()
            .find(|x| x["k"] == "crumb" && x["a"] == "settings-page");
        assert_eq!(c.map(|c| c["page"].clone()), Some(txt("diagnostics")));
        // The settings controller says so when a page is chosen.
        let src = include_str!("settings_ctl.rs");
        let code = src.split("#[cfg(test)]").next().unwrap();
        assert!(code.contains("\"settings-page\""));
    }

    #[test]
    fn mark_says_marked_and_stamps_the_log() {
        let _g = lock(&ONE_AT_A_TIME);
        let dir = tempfile::tempdir().unwrap();
        let ctl = start(dir.path(), facts(), "software", false);
        let control = flare_diag::control::DiagControl::with_log(dir.path(), ctl.log.clone());
        control.mark();
        ctl.log.flush();
        ctl.stop();
        let l = lines(dir.path());
        let marks: Vec<_> = l.iter().filter(|x| x["k"] == "mark").collect();
        assert_eq!(marks.len(), 1);
        let keys: Vec<&String> = marks[0].as_object().unwrap().keys().collect();
        assert_eq!(keys, ["t", "up", "src", "k"]);
    }

    #[test]
    fn verbose_is_off_by_default_on_logs_every_item_and_persists() {
        let _g = lock(&ONE_AT_A_TIME);
        let dir = tempfile::tempdir().unwrap();
        let ctl = start(dir.path(), facts(), "software", false);
        let control = flare_diag::control::DiagControl::with_log(dir.path(), ctl.log.clone());
        assert!(!control.verbose());
        timed("not_logged", || {});
        assert!(control.set_verbose(true));
        timed("logged", || {});
        crumb_often("tab-switch", &[("id", txt("a"))]);
        ctl.log.flush();
        ctl.stop();
        let l = lines(dir.path());
        assert!(!l.iter().any(|x| x["name"] == "not_logged"));
        assert!(l.iter().any(|x| x["k"] == "work" && x["name"] == "logged"));
        assert!(
            l.iter()
                .any(|x| x["k"] == "crumb" && x["a"] == "tab-switch")
        );
        // A new run starts verbose: diag.json holds the switch.
        let again = start(dir.path(), facts(), "software", false);
        assert!(again.log.verbose());
        again.stop();
        let control = flare_diag::control::DiagControl::new(dir.path(), None);
        assert!(control.verbose());
    }

    #[test]
    fn an_often_crumb_stays_out_of_the_log_unless_verbose() {
        let _g = lock(&ONE_AT_A_TIME);
        let dir = tempfile::tempdir().unwrap();
        let ctl = start(dir.path(), facts(), "software", false);
        crumb_often("tab-switch", &[("id", txt("a"))]);
        ctl.log.flush();
        assert!(!lines(dir.path()).iter().any(|x| x["a"] == "tab-switch"));
        assert_eq!(ctl.crumbs().before(u64::MAX, 5).len(), 1);
        ctl.stop();
    }

    #[test]
    fn the_sink_adapter_accepts_agent_kinds_and_refuses_session() {
        let dir = tempfile::tempdir().unwrap();
        let log = DiagLog::start(DiagLogOptions::new(dir.path().join("logs")));
        let sink = SinkAdapter(log.clone());
        sink.record(
            "agent-hook",
            &[
                ("id", DiagValue::Str("t1".into())),
                ("repeats", DiagValue::Num(2)),
            ],
            0,
        );
        sink.record("session", &[], 0);
        sink.record("agent-bogus", &[], 0);
        sink.record("frame-task", &[], 0);
        log.flush();
        let l = lines(dir.path());
        assert_eq!(kinds(&l), ["agent-hook"]);
        assert_eq!(l[0]["src"], "app");
        assert_eq!(l[0]["repeats"], 2);
    }

    #[test]
    fn session_end_is_logged_and_flushed() {
        let _g = lock(&ONE_AT_A_TIME);
        let dir = tempfile::tempdir().unwrap();
        let ctl = start(dir.path(), facts(), "software", false);
        session_end();
        // No flush call: session_end wrote it to the disk itself.
        assert!(kinds(&lines(dir.path())).contains(&"session-end".to_owned()));
        ctl.stop();
    }

    #[test]
    fn suspend_and_resume_write_no_lag() {
        let _g = lock(&ONE_AT_A_TIME);
        let dir = tempfile::tempdir().unwrap();
        let ctl = start(dir.path(), facts(), "software", false);
        let power = power_events().expect("running");
        ctl.watch().beat();
        (power.on_suspend)();
        std::thread::sleep(Duration::from_millis(400)); // sync-ok: test code, the machine asleep
        (power.on_resume)();
        ctl.watch().beat();
        std::thread::sleep(Duration::from_millis(250)); // sync-ok: test code, lets the watchdog step
        ctl.log.flush();
        ctl.stop();
        let k = kinds(&lines(dir.path()));
        assert!(!k.contains(&"main-lag".to_owned()), "{k:?}");
        assert!(!k.contains(&"ui-stack".to_owned()), "{k:?}");
    }

    #[test]
    fn the_free_functions_do_nothing_before_start_and_run_the_work() {
        let _g = lock(&ONE_AT_A_TIME);
        crumb("tab-open", &[]);
        session_end();
        assert_eq!(timed("x", || 7), 7);
        assert_eq!(timed_waits("x", || 8), 8);
    }

    #[test]
    fn the_log_folder_is_under_the_profile() {
        let _g = lock(&ONE_AT_A_TIME);
        let dir = tempfile::tempdir().unwrap();
        for profile in [dir.path().join("default"), dir.path().join("custom data")] {
            let ctl = start(&profile, facts(), "software", false);
            ctl.log.flush();
            assert_eq!(ctl.log.dir(), profile.join("logs"));
            assert!(profile.join("logs").join(DIAG_FILE).is_file());
            ctl.stop();
        }
    }

    #[test]
    fn a_folder_that_cannot_be_made_gives_a_null_log() {
        let _g = lock(&ONE_AT_A_TIME);
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("file");
        std::fs::write(&blocker, "x").unwrap(); // sync-ok: test code
        let ctl = start(&blocker, facts(), "software", false);
        assert!(ctl.log.dir().as_os_str().is_empty());
        crumb("tab-open", &[]);
        ctl.stop();
    }

    #[test]
    fn the_first_run_crumb_comes_from_the_hook_version() {
        let _g = lock(&ONE_AT_A_TIME);
        let src = include_str!("diag_ctl.rs");
        let code = src.split("#[cfg(test)]").next().unwrap();
        assert!(code.contains("\"first-run\""));
        assert!(code.contains("FIRST_RUN.get()"));
    }

    /// The crumbs of Appendix E, with the one whose writer is not in this branch.
    const CRUMBS: &[&str] = &[
        "tab-open",
        "tab-close",
        "tab-switch",
        "settings-page",
        "update-open",
        "update-install",
        "update-check",
        "shell-spawn",
        "shell-exit",
        "shell-spawn-failed",
        "kill-stuck",
        "memory",
        "first-run",
    ];

    fn source_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        }; // sync-ok: test code
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                source_files(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }

    #[test]
    fn every_crumb_in_appendix_e_has_a_call_site() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut files = Vec::new();
        for c in std::fs::read_dir(&crates).unwrap().flatten() {
            // sync-ok: test code
            source_files(&c.path().join("src"), &mut files);
        }
        let mut code = String::new();
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap(); // sync-ok: test code
            code.push_str(
                text.split(
                    "
#[cfg(test)]
mod ",
                )
                .next()
                .unwrap_or(""),
            );
            code.push('\n');
        }
        for c in CRUMBS {
            assert!(
                code.contains(&format!("\"{c}\"")),
                "no call site writes the crumb {c}"
            );
        }
    }

    #[test]
    fn the_sources_beat_and_hook_the_watchdog() {
        let w = include_str!("window.rs");
        for needle in [
            "diag_ctl::beat_timer",
            "diag_ctl::set_window",
            "diag_ctl::session_end",
            "diag_ctl::power_events",
        ] {
            assert!(w.contains(needle), "window.rs lacks {needle}");
        }
        let a = include_str!("appctl.rs");
        assert!(
            a.contains("diag_ctl::quit()"),
            "the end of life stops the log last"
        );
    }

    #[test]
    fn the_kill_hook_writes_a_crumb() {
        let _g = lock(&ONE_AT_A_TIME);
        let dir = tempfile::tempdir().unwrap();
        let ctl = start(dir.path(), facts(), "software", false);
        install_kill_hook();
        let hook = flare_term::session::KILL_STUCK_HOOK.get().expect("set");
        hook(77, 1500);
        let dropped = flare_term::session::WRITE_DROP_HOOK.get().expect("set");
        dropped(78, 4096);
        dropped(78, 4096);
        ctl.log.flush();
        ctl.stop();
        let l = lines(dir.path());
        let c = l.iter().find(|x| x["a"] == "kill-stuck").expect("a crumb");
        assert_eq!(c["pid"], 77);
        assert_eq!(c["ms"], 1500);
        let d: Vec<_> = l.iter().filter(|x| x["a"] == "pty-write-dropped").collect();
        assert_eq!(d.len(), 1, "one crumb a second, however many drops");
        assert_eq!(d[0]["pid"], 78);
        assert_eq!(d[0]["len"], 4096);
    }

    #[test]
    fn beating_and_hung_polling_survive_a_window_handle() {
        let _g = lock(&ONE_AT_A_TIME);
        let dir = tempfile::tempdir().unwrap();
        let ctl = start(dir.path(), facts(), "software", false);
        set_window(0x1234_5678); // not a window: IsHungAppWindow says false
        let n = Arc::new(AtomicUsize::new(0));
        let n2 = n.clone();
        set_shell_pids(Arc::new(move || {
            n2.fetch_add(1, Ordering::SeqCst);
            vec![std::process::id()]
        }));
        std::thread::sleep(Duration::from_millis(600)); // sync-ok: test code, one poll round
        ctl.log.flush();
        let k = kinds(&lines(dir.path()));
        assert!(!k.contains(&"unresponsive".to_owned()));
        ctl.stop();
    }
}
