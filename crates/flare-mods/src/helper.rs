//! A plugin's helper program (plugins phase 7, design 4, 10 and 13): started while the plugin is on, stopped when it
//! turns off, and never outliving Flare.
//!
//! - **Which program.** Only the manifest's `helper.command` (on a Mac `helpers.macos.command`, rule 288), an `.exe`
//!   (on a Mac an executable that is not quarantined, rule 289) inside the plugin folder, checked again when it
//!   starts (`paths::program_inside`: no `..`, no absolute path, and it must resolve inside the folder, so a link
//!   swapped in since the scan is refused). It is started by its canonical path, in the plugin folder, with the
//!   manifest's arguments (at most 16 of 200 characters), no console window and stdin closed. Only a plugin that is on
//!   and holds `process` in its approved grant is ever started (the hub decides, [`crate::Hub::helper_of`]).
//! - **Its environment.** Flare's own, minus `FLARE_TAB_TOKEN` and `FLARE_RESUME_ID`, plus `FLARE_PLUGIN_TOKEN` (the
//!   plugin's own grant token, set on the child only, never the agent's) and `FLARE_CTL_PIPE` (the channel's pipe). It
//!   speaks the same channel as a plugin page, with the plugin's grant and budget.
//! - **The job.** It starts suspended, joins a kill-on-close job object of its own, and only then runs: it and every
//!   process it starts end when the plugin is stopped (the job is terminated) or when Flare ends (the handle closes).
//! - **Its output.** stdout and stderr go to `<profile>\plugins-logs\<id>.log`, which is moved to `<id>.log.1` (the one
//!   before is dropped) when it passes [`LOG_MAX`].
//! - **Restarts.** A helper that ends by itself is started again at once, at most [`MAX_RESTARTS`] times in
//!   [`RESTART_WINDOW`]; the next end stops it, and Settings says so ([`STOPPED_HELPER`]).
//!
//! Everything that waits (the start, the reads of its output, the wait for its end) runs on threads of its own; the
//! calls the window makes ([`Host::start`], [`Host::stop`]) return at once.
//!
//! **macOS** (port phase 6, regression rule 284): no job object, the same guarantees. The helper starts in a session
//! and process group of its own (`setsid` between fork and exec), with a guard
//! ([`flare_proc::guard`]): Flare's own executable run as an orphan in that session, in a group of its own, holding one
//! end of a socket pair whose other end only Flare holds. The guard ends the whole tree (the group, the session and
//! every descendant it has seen, so a process that left the group or the session is caught too) when Flare lets its
//! end go (a stop), when the helper ends (a crash, as the end of a job's last handle does), or when Flare is gone (its
//! exit or a crash: the kernel closes Flare's end and the guard also watches Flare's pid). The helper itself is reaped
//! by its own thread, which first waits for its end without reaping it, so a stop never signals a recycled pid, and
//! which ends the session and the group itself before the reap, should the helper have killed its guard.
// Off Windows and macOS the spawn is refused, so the log, the pump and the policy fields are unused there.
#![cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
#[cfg(windows)]
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Child;
#[cfg(any(windows, target_os = "macos"))]
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
#[cfg(windows)]
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
#[cfg(windows)]
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicAccountingInformation, JobObjectBasicProcessIdList,
    JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
    TerminateJobObject,
};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, OpenProcess, OpenThread, PROCESS_SYNCHRONIZE, ResumeThread,
    THREAD_SUSPEND_RESUME, WaitForSingleObject,
};

/// The variable that carries the plugin's own token to its helper.
pub const ENV_TOKEN: &str = "FLARE_PLUGIN_TOKEN";
/// Variables of Flare's own environment a helper never gets: a tab's secret and the session a shell resumes.
pub const ENV_REMOVED: [&str; 2] = ["FLARE_TAB_TOKEN", "FLARE_RESUME_ID"];
/// A log passes this size, it is moved aside and a new one begins.
pub const LOG_MAX: u64 = 1024 * 1024;
/// A helper that ends by itself is started again at most this many times in [`RESTART_WINDOW`].
pub const MAX_RESTARTS: usize = 3;
pub const RESTART_WINDOW: Duration = Duration::from_secs(5 * 60);
/// The sentence Settings shows for a helper that ended too often.
pub const STOPPED_HELPER: &str = "Stopped: its program ended again after 3 restarts in 5 minutes. Turn it off and on to start it again.";

/// How long an update or a removal waits for a stopped helper's processes to end before it goes on anyway.
pub const STOP_WAIT: Duration = Duration::from_secs(5);
/// The most processes of one helper's job [`Host::wait_stopped`] waits on (the job's count covers the rest).
#[cfg(windows)]
const PIDS_MAX: usize = 64;

/// Where a plugin's helper log is: `<logs>\<id>.log`.
pub fn log_file(logs: &Path, id: &str) -> PathBuf {
    logs.join(format!("{id}.log"))
}

/// What starting a helper needs. The token is a secret: [`std::fmt::Debug`] leaves it out.
#[derive(Clone, PartialEq, Eq)]
pub struct Spec {
    pub id: String,
    /// The canonical plugin folder.
    pub folder: PathBuf,
    /// The command of this system's helper (`helper.command` on Windows, `helpers.macos.command` on a Mac), relative
    /// to the folder.
    pub command: String,
    pub args: Vec<String>,
    /// The plugin's own grant token.
    pub token: String,
}

impl std::fmt::Debug for Spec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Spec")
            .field("id", &self.id)
            .field("folder", &self.folder)
            .field("command", &self.command)
            .field("args", &self.args)
            .finish_non_exhaustive()
    }
}

/// When a helper ended by itself, within the window: whether it may start again.
#[derive(Clone, Debug)]
pub struct Restarts {
    max: usize,
    window: Duration,
    times: Vec<Instant>,
}

impl Restarts {
    pub fn new(max: usize, window: Duration) -> Self {
        Self {
            max,
            window,
            times: Vec::new(),
        }
    }

    /// It ended at `now`: true when it may be started again (and that restart is counted), false when it ended too
    /// often and is to stay stopped.
    pub fn ended(&mut self, now: Instant) -> bool {
        let window = self.window;
        self.times
            .retain(|t| now.saturating_duration_since(*t) < window);
        if self.times.len() >= self.max {
            return false;
        }
        self.times.push(now);
        true
    }
}

/// What a helper did, for the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Note {
    /// It runs (started, or started again).
    Started { id: String, pid: u32 },
    /// It ended by itself and was started again.
    Restarted { id: String },
    /// It ended too often, or could not start: it stays stopped, with the sentence for Settings.
    Stopped { id: String, why: String },
}

/// A process of a stopped job, to wait on.
#[cfg(windows)]
type ProcHandle = OwnedHandle;
/// None is kept off Windows: macOS waits on the job's own state ([`Job::active`]).
#[cfg(not(windows))]
type ProcHandle = std::convert::Infallible;

/// Neither Windows nor macOS: no job can be made, so no helper starts.
#[cfg(not(any(windows, target_os = "macos")))]
struct Job(std::convert::Infallible);

#[cfg(not(any(windows, target_os = "macos")))]
impl Job {
    fn terminate(&self) {
        match self.0 {}
    }

    fn processes(&self) -> Vec<ProcHandle> {
        match self.0 {}
    }

    fn active(&self) -> u32 {
        match self.0 {}
    }
}

/// macOS: the helper's group (its pid is the group's and the session's id) and Flare's end of its guard. Dropping
/// it, or [`Job::terminate`], lets the guard end the whole tree, as closing a kill-on-close job does.
#[cfg(target_os = "macos")]
struct Job {
    leader: u32,
    /// The guard process, to wait on (None when it was gone before its pid was read).
    guard_id: Option<flare_proc::guard::GuardId>,
    state: Mutex<MacJob>,
}

#[cfg(target_os = "macos")]
struct MacJob {
    guard: Option<flare_proc::guard::Guard>,
    /// The helper has ended (seen, not yet reaped): its pid may no longer be signalled once it is reaped.
    ended: bool,
    /// Flare's own pass over the session and group ran ([`Job::finish`]).
    swept: bool,
    /// The helper is reaped and nothing of its tree is left: [`Job::active`] is 0.
    done: bool,
}

#[cfg(target_os = "macos")]
impl Job {
    fn new(leader: u32, guard: Option<flare_proc::guard::Guard>) -> Self {
        Self {
            leader,
            guard_id: guard.as_ref().and_then(|g| g.id()),
            state: Mutex::new(MacJob {
                guard,
                ended: false,
                swept: false,
                done: false,
            }),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, MacJob> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Ends every process of the tree (no wait): the guard is let go, and the helper itself is killed while its pid
    /// is still its own (not reaped: its thread reaps only after it marks the end, under this lock).
    fn terminate(&self) {
        let mut s = self.state();
        if let Some(mut g) = s.guard.take() {
            g.release();
        }
        if !s.ended {
            flare_proc::signal(self.leader, flare_proc::Signal::Kill);
        }
    }

    /// Nothing to hold per process: [`Job::active`] reads the job's own state.
    fn processes(&self) -> Vec<ProcHandle> {
        Vec::new()
    }

    /// 1 until the helper's thread has reaped it and ended what was left of its tree ([`Job::finish`]), else 0.
    fn active(&self) -> u32 {
        u32::from(!self.state().done)
    }

    /// The helper's end, on its thread, once `seen_unreaped` says whether its end was seen with its pid still held
    /// (`waitid(WNOWAIT)`). In this order (rule 286): it is marked ended, before anything can reap it, so a stop that
    /// races never signals a recycled pid; its guard is let go and waited for (at most [`STOP_WAIT`]); Flare's own pass
    /// ends the session and the group (a guard the helper killed, or one too slow), while the unreaped helper still
    /// holds the ids, so the pass cannot reach a group or session that reuses them; only then is it reaped by `reap`.
    /// When the end could not be seen unreaped (`waitid` failed, say `ECHILD`), it is reaped first and the pass runs
    /// after, on ids an empty session no longer holds (a window of a moment).
    fn finish<R>(&self, seen_unreaped: bool, reap: impl FnOnce() -> R) -> R {
        self.state().ended = true;
        let out = if seen_unreaped {
            self.end_tree();
            reap()
        } else {
            let out = reap();
            self.end_tree();
            out
        };
        self.state().done = true;
        out
    }

    /// The guard let go and waited for, then Flare's pass over the session and the group.
    fn end_tree(&self) {
        self.terminate();
        if let Some(id) = self.guard_id
            && !id.wait_gone(STOP_WAIT)
        {
            log::warn!("the guard of a plugin helper did not end in time");
        }
        flare_proc::guard::end_session(self.leader);
        self.state().swept = true;
    }
}

/// A kill-on-close job object: closing it, or terminating it, ends every process in it.
#[cfg(windows)]
struct Job(HANDLE);

// SAFETY: a job handle is a kernel object; the job calls are thread-safe on one handle.
#[cfg(windows)]
unsafe impl Send for Job {}
// SAFETY: as above; the struct holds nothing else, so a shared reference only makes calls.
#[cfg(windows)]
unsafe impl Sync for Job {}

#[cfg(windows)]
impl Job {
    fn new() -> std::io::Result<Self> {
        // SAFETY: null attributes and name make an unnamed job with the default security.
        let h = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if h.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let job = Job(h);
        // SAFETY: `info` is zeroed, then one flag is set; it lives for the call and its size is passed.
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: `job.0` is the job just made; the pointer and size describe `info`.
        let ok = unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&info).cast(),
                u32::try_from(std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
                    .unwrap_or(0),
            )
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(job)
    }

    fn assign(&self, child: &Child) -> std::io::Result<()> {
        // SAFETY: both handles are live: the job is ours, the process is the child's.
        let ok = unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle().cast()) };
        if ok == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    /// Ends every process in the job at once (no wait).
    fn terminate(&self) {
        // SAFETY: the job handle is live for as long as `self`.
        unsafe {
            TerminateJobObject(self.0, 1);
        }
    }

    /// A handle to wait on for each process in the job now (at most [`PIDS_MAX`]). Taken before the job is
    /// terminated: a process is signalled only once its handles (its current folder among them) are closed.
    fn processes(&self) -> Vec<ProcHandle> {
        #[repr(C)]
        struct PidList {
            assigned: u32,
            listed: u32,
            ids: [usize; PIDS_MAX],
        }
        let mut list = PidList {
            assigned: 0,
            listed: 0,
            ids: [0; PIDS_MAX],
        };
        // SAFETY: `list` has the layout of JOBOBJECT_BASIC_PROCESS_ID_LIST with room for PIDS_MAX ids; its size is
        // passed.
        let ok = unsafe {
            QueryInformationJobObject(
                self.0,
                JobObjectBasicProcessIdList,
                std::ptr::from_mut(&mut list).cast(),
                u32::try_from(std::mem::size_of::<PidList>()).unwrap_or(0),
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Vec::new();
        }
        let listed = (list.listed as usize).min(PIDS_MAX);
        list.ids[..listed]
            .iter()
            .filter_map(|&pid| {
                let pid = u32::try_from(pid).ok()?;
                // SAFETY: OpenProcess returns a handle or null; a handle is owned (closed once) from here.
                let h = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
                // SAFETY: `h` is a live handle no one else owns.
                (!h.is_null()).then(|| unsafe { OwnedHandle::from_raw_handle(h.cast()) })
            })
            .collect()
    }

    /// How many processes of the job have not ended.
    fn active(&self) -> u32 {
        // SAFETY: a zeroed accounting record is a valid buffer; its size is passed.
        let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: the job handle is live; the pointer and size describe `info`.
        let ok = unsafe {
            QueryInformationJobObject(
                self.0,
                JobObjectBasicAccountingInformation,
                std::ptr::from_mut(&mut info).cast(),
                u32::try_from(std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>())
                    .unwrap_or(0),
                std::ptr::null_mut(),
            )
        };
        if ok == 0 { 0 } else { info.ActiveProcesses }
    }
}

/// The processes of a job [`Host::stop`] terminated, until [`Host::wait_stopped`] sees them end.
struct Ending {
    /// The start it was ([`Run::seq`]).
    seq: u64,
    /// None while that start had no job yet: its own thread terminates what it spawned, and puts the job here.
    job: Option<Arc<Job>>,
    procs: Vec<ProcHandle>,
}

#[cfg(windows)]
impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateJobObjectW and is closed once, here.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

/// Lets the threads of a process started suspended run. Error when none could be resumed.
#[cfg(windows)]
fn resume_process(pid: u32) -> std::io::Result<()> {
    // SAFETY: a snapshot of every thread; the handle is checked and closed below.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snap == INVALID_HANDLE_VALUE || snap.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: a zeroed THREADENTRY32 with its size set is what Thread32First expects.
    let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
    entry.dwSize = u32::try_from(std::mem::size_of::<THREADENTRY32>()).unwrap_or(0);
    let mut resumed = 0;
    // SAFETY: `snap` is a live snapshot and `entry` a valid, sized buffer for every call.
    let mut more = unsafe { Thread32First(snap, &mut entry) } != 0;
    while more {
        if entry.th32OwnerProcessID == pid {
            // SAFETY: OpenThread returns a handle or null; a handle is used once and closed.
            unsafe {
                let t = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                if !t.is_null() {
                    if ResumeThread(t) != u32::MAX {
                        resumed += 1;
                    }
                    CloseHandle(t);
                }
            }
        }
        // SAFETY: as above.
        more = unsafe { Thread32Next(snap, &mut entry) } != 0;
    }
    // SAFETY: the snapshot handle is closed once.
    unsafe {
        CloseHandle(snap);
    }
    if resumed == 0 {
        return Err(std::io::Error::other(
            "no thread of the program could be resumed",
        ));
    }
    Ok(())
}

/// A helper's log: appended to, moved aside past [`LOG_MAX`].
struct Log {
    path: PathBuf,
    file: Option<File>,
    size: u64,
    max: u64,
}

impl Log {
    fn open(path: PathBuf, max: u64) -> Self {
        let mut log = Log {
            path,
            file: None,
            size: 0,
            max,
        };
        log.reopen();
        log
    }

    fn reopen(&mut self) {
        if let Some(dir) = self.path.parent() {
            let _ = fs::create_dir_all(dir); // sync-ok: the helper's own threads
        }
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path) // sync-ok: the helper's own threads
            .ok();
        self.size = self
            .file
            .as_ref()
            .and_then(|f| f.metadata().ok())
            .map_or(0, |m| m.len());
    }

    fn write(&mut self, bytes: &[u8]) {
        if self.size + bytes.len() as u64 > self.max && self.size > 0 {
            self.file = None;
            let mut old = self.path.clone().into_os_string();
            old.push(".1");
            let old = PathBuf::from(old);
            let _ = fs::remove_file(&old); // sync-ok: the helper's own threads
            let _ = fs::rename(&self.path, &old); // sync-ok: the helper's own threads
            self.reopen();
        }
        if let Some(f) = self.file.as_mut()
            && f.write_all(bytes).is_ok()
        {
            self.size += bytes.len() as u64;
        }
    }
}

/// One helper that runs (or is being started).
struct Run {
    /// Which start this is: an end seen by an older start's waiter is not this one's.
    seq: u64,
    job: Option<Arc<Job>>,
    spec: Spec,
    restarts: Restarts,
}

struct Shared {
    logs: PathBuf,
    pipe: String,
    notify: Box<dyn Fn(Note) + Send + Sync>,
    runs: Mutex<HashMap<String, Run>>,
    /// Jobs stopped and maybe not yet ended, by plugin id (the latest stop of each).
    ending: Mutex<HashMap<String, Ending>>,
    seq: Mutex<u64>,
    max_restarts: usize,
    window: Duration,
    log_max: u64,
}

/// Every plugin's helper. Cheap to clone; all of it is shared.
#[derive(Clone)]
pub struct Host {
    shared: Arc<Shared>,
}

impl Host {
    /// Helpers that log to `logs` (`<profile>\plugins-logs`), are told the channel's `pipe`, and report to `notify`
    /// (from their own threads).
    pub fn new(logs: PathBuf, pipe: String, notify: impl Fn(Note) + Send + Sync + 'static) -> Self {
        Self::with_policy(logs, pipe, notify, MAX_RESTARTS, RESTART_WINDOW, LOG_MAX)
    }

    /// [`Host::new`] with the restart policy and the log size as parameters (the tests).
    pub fn with_policy(
        logs: PathBuf,
        pipe: String,
        notify: impl Fn(Note) + Send + Sync + 'static,
        max_restarts: usize,
        window: Duration,
        log_max: u64,
    ) -> Self {
        Self {
            shared: Arc::new(Shared {
                logs,
                pipe,
                notify: Box::new(notify),
                runs: Mutex::new(HashMap::new()),
                ending: Mutex::new(HashMap::new()),
                seq: Mutex::new(0),
                max_restarts,
                window,
                log_max,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Run>> {
        self.shared.runs.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The plugins whose helper runs or is starting, sorted.
    pub fn running(&self) -> Vec<String> {
        let mut v: Vec<String> = self.lock().keys().cloned().collect();
        v.sort();
        v
    }

    pub fn is_running(&self, id: &str) -> bool {
        self.lock().contains_key(id)
    }

    /// Starts the plugin's helper on a thread of its own, unless it already runs. Its restarts are counted afresh.
    pub fn start(&self, spec: Spec) {
        let seq = {
            let mut g = self.lock();
            if g.contains_key(&spec.id) {
                return;
            }
            let seq = self.next_seq();
            g.insert(
                spec.id.clone(),
                Run {
                    seq,
                    job: None,
                    spec: spec.clone(),
                    restarts: Restarts::new(self.shared.max_restarts, self.shared.window),
                },
            );
            seq
        };
        self.launch_on_thread(spec.id, seq);
    }

    fn next_seq(&self) -> u64 {
        let mut s = self.shared.seq.lock().unwrap_or_else(|e| e.into_inner());
        *s += 1;
        *s
    }

    fn launch_on_thread(&self, id: String, seq: u64) {
        let host = self.clone();
        let spawned = std::thread::Builder::new()
            .name("flare-helper-start".into())
            .spawn(move || host.launch(&id, seq));
        if let Err(e) = spawned {
            log::warn!("a plugin helper could not be started: {e}");
        }
    }

    /// Stops the plugin's helper: its job is terminated now (every process of it ends), and its end is not counted.
    /// Nothing waits here; the helper's own thread sees it end, and [`Host::wait_stopped`] waits for it on a worker.
    pub fn stop(&self, id: &str) {
        let (seq, job) = {
            let mut g = self.lock();
            let Some(run) = g.remove(id) else { return };
            (run.seq, run.job)
        };
        match job {
            Some(job) => self.end_job(id, seq, job),
            // Still starting: its thread ends what it spawns; until then a wait waits for that.
            None => {
                self.ending().insert(
                    id.to_string(),
                    Ending {
                        seq,
                        job: None,
                        procs: Vec::new(),
                    },
                );
            }
        }
        log::info!("plugin {id}: its helper was stopped");
    }

    fn ending(&self) -> std::sync::MutexGuard<'_, HashMap<String, Ending>> {
        self.shared.ending.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Terminates `job` of start `seq`, keeping what [`Host::wait_stopped`] waits on.
    fn end_job(&self, id: &str, seq: u64, job: Arc<Job>) {
        let procs = job.processes();
        job.terminate();
        self.ending().insert(
            id.to_string(),
            Ending {
                seq,
                job: Some(job),
                procs,
            },
        );
    }

    /// A start stopped before it spawned anything: there is nothing to wait for.
    fn nothing_spawned(&self, id: &str, seq: u64) {
        let mut e = self.ending();
        if e.get(id).is_some_and(|x| x.seq == seq && x.job.is_none()) {
            e.remove(id);
        }
    }

    /// Waits, at most `timeout`, until every process of the plugin's last stopped helper has ended and closed its
    /// handles (a terminated job ends its processes a moment later, and the one in the plugin folder holds it until
    /// then). True when they have, or none was stopped. It blocks: a worker's call, never the window's.
    pub fn wait_stopped(&self, id: &str, timeout: Duration) -> bool {
        let until = Instant::now() + timeout;
        let left_ms = || {
            u32::try_from(until.saturating_duration_since(Instant::now()).as_millis())
                .unwrap_or(u32::MAX)
        };
        // A helper stopped while it started: its thread puts its job here, or says it spawned nothing.
        let (job, procs) = loop {
            let mut e = self.ending();
            match e.remove(id) {
                None => return true,
                Some(Ending {
                    job: Some(job),
                    procs,
                    ..
                }) => break (job, procs),
                Some(starting) => {
                    e.insert(id.to_string(), starting);
                }
            }
            drop(e);
            if left_ms() == 0 {
                log::warn!("plugin {id}: its stopped helper did not end in time");
                return false;
            }
            std::thread::sleep(Duration::from_millis(10)); // sync-ok: a worker's wait
        };
        #[cfg(not(windows))]
        if let Some(p) = procs.first() {
            match *p {}
        }
        #[cfg(windows)]
        for p in &procs {
            // SAFETY: `p` is a live process handle with SYNCHRONIZE access.
            if unsafe { WaitForSingleObject(p.as_raw_handle().cast(), left_ms()) } != WAIT_OBJECT_0
            {
                log::warn!("plugin {id}: its stopped helper did not end in time");
                return false;
            }
        }
        while job.active() > 0 {
            if left_ms() == 0 {
                log::warn!("plugin {id}: its stopped helper did not end in time");
                return false;
            }
            std::thread::sleep(Duration::from_millis(10)); // sync-ok: a worker's wait
        }
        true
    }

    /// [`Host::stop`], then [`Host::wait_stopped`]: on a worker only.
    pub fn stop_and_wait(&self, id: &str, timeout: Duration) -> bool {
        self.stop(id);
        self.wait_stopped(id, timeout)
    }

    /// Stops every helper (Flare quits; the jobs would end them when Flare's handles close anyway).
    pub fn stop_all(&self) {
        for id in self.running() {
            self.stop(&id);
        }
    }

    /// The helper's start, on its own thread: checks, spawns suspended, joins its job, runs, and waits for its end.
    fn launch(&self, id: &str, seq: u64) {
        let Some(spec) = self
            .lock()
            .get(id)
            .filter(|r| r.seq == seq)
            .map(|r| r.spec.clone())
        else {
            self.nothing_spawned(id, seq);
            return;
        };
        match self.spawn(&spec) {
            Ok((child, job)) => {
                let pid = child.id();
                let job = Arc::new(job);
                let mut g = self.lock();
                match g.get_mut(id).filter(|r| r.seq == seq) {
                    Some(run) => run.job = Some(Arc::clone(&job)),
                    None => {
                        // Stopped while it started.
                        drop(g);
                        self.end_job(id, seq, Arc::clone(&job));
                        // macOS: the killed helper is reaped here, after its end is marked (rule 286), or it would
                        // stay a zombie and every wait for the stop (an update's, a removal's) would run to its timeout.
                        #[cfg(target_os = "macos")]
                        {
                            let mut child = child;
                            let unreaped = flare_proc::guard::wait_ended_unreaped(child.id());
                            let _ = job.finish(unreaped, || child.wait()); // sync-ok: the helper's own thread
                        }
                        return;
                    }
                }
                drop(g);
                log::info!("plugin {id}: its helper started");
                (self.shared.notify)(Note::Started {
                    id: id.to_string(),
                    pid,
                });
                self.wait_for_end(id, seq, child, job);
            }
            Err(why) => {
                log::warn!("plugin {id}: its helper did not start: {why}");
                self.nothing_spawned(id, seq);
                let ours = self.lock().get(id).is_some_and(|r| r.seq == seq);
                if ours {
                    self.lock().remove(id);
                    (self.shared.notify)(Note::Stopped {
                        id: id.to_string(),
                        why: format!("Its program did not start: {why}."),
                    });
                }
            }
        }
    }

    /// macOS: an executable file inside the folder, started by its canonical path in a session and group of its own
    /// with its guard ([`flare_proc::guard`]), in the plugin folder, stdin closed, its output to the log.
    #[cfg(target_os = "macos")]
    fn spawn(&self, spec: &Spec) -> Result<(Child, Job), String> {
        let program = crate::paths::unix_program_inside(&spec.folder, &spec.command)
            .map_err(|r| format!("helpers.macos.command {r}"))?;
        let mut cmd = Command::new(&program);
        cmd.args(&spec.args)
            .current_dir(&spec.folder)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for var in ENV_REMOVED {
            cmd.env_remove(var);
        }
        cmd.env(ENV_TOKEN, &spec.token)
            .env(flare_ctl::ENV_PIPE, &self.shared.pipe);
        let pending = flare_proc::guard::guard_command(&mut cmd)
            .map_err(|e| format!("its guard could not be made: {e}"))?;
        let mut child = cmd.spawn().map_err(|e| {
            if e.raw_os_error() == Some(flare_proc::guard::NO_GUARD) {
                "its guard could not start".to_string()
            } else {
                e.to_string()
            }
        })?; // sync-ok: the helper's own thread
        let job = Job::new(child.id(), Some(pending.started()));
        let log = Arc::new(Mutex::new(Log::open(
            log_file(&self.shared.logs, &spec.id),
            self.shared.log_max,
        )));
        if let Some(out) = child.stdout.take() {
            pump(out, Arc::clone(&log));
        }
        if let Some(err) = child.stderr.take() {
            pump(err, log);
        }
        Ok((child, job))
    }

    /// Neither Windows nor macOS. The folder check still runs; then the start is refused.
    #[cfg(not(any(windows, target_os = "macos")))]
    fn spawn(&self, spec: &Spec) -> Result<(Child, Job), String> {
        crate::paths::program_inside(&spec.folder, &spec.command)
            .map_err(|r| format!("helper.command {r}"))?;
        Err("plugin helpers are not built on this platform yet".into())
    }

    #[cfg(windows)]
    fn spawn(&self, spec: &Spec) -> Result<(Child, Job), String> {
        let program = crate::paths::program_inside(&spec.folder, &spec.command)
            .map_err(|r| format!("helper.command {r}"))?;
        let mut cmd = Command::new(&program);
        cmd.args(&spec.args)
            .current_dir(&spec.folder)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
        for var in ENV_REMOVED {
            cmd.env_remove(var);
        }
        cmd.env(ENV_TOKEN, &spec.token)
            .env(flare_ctl::ENV_PIPE, &self.shared.pipe);
        let mut child = cmd.spawn().map_err(|e| e.to_string())?; // sync-ok: the helper's own thread
        let job = match Job::new().and_then(|j| j.assign(&child).map(|()| j)) {
            Ok(j) => j,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait(); // sync-ok: the helper's own thread
                return Err(format!("its job object could not be made: {e}"));
            }
        };
        if let Err(e) = resume_process(child.id()) {
            job.terminate();
            let _ = child.wait(); // sync-ok: the helper's own thread
            return Err(e.to_string());
        }
        let log = Arc::new(Mutex::new(Log::open(
            log_file(&self.shared.logs, &spec.id),
            self.shared.log_max,
        )));
        if let Some(out) = child.stdout.take() {
            pump(out, Arc::clone(&log));
        }
        if let Some(err) = child.stderr.take() {
            pump(err, log);
        }
        Ok((child, job))
    }

    /// Waits for the helper to end (its own thread), then starts it again or stops it, by the policy.
    fn wait_for_end(&self, id: &str, seq: u64, mut child: Child, job: Arc<Job>) {
        // macOS: its end is seen before it is reaped, so a stop that races it never signals a recycled pid; the guard,
        // then Flare's own pass, end what is left of the tree (a crash ends it all, as a job's last handle does).
        #[cfg(target_os = "macos")]
        let status = {
            let unreaped = flare_proc::guard::wait_ended_unreaped(child.id());
            job.finish(unreaped, || child.wait()) // sync-ok: the helper's own thread
        };
        #[cfg(not(target_os = "macos"))]
        let status = child.wait(); // sync-ok: the helper's own thread
        drop(job);
        let code = status.ok().and_then(|s| s.code());
        let mut g = self.lock();
        let Some(run) = g.get_mut(id).filter(|r| r.seq == seq) else {
            return; // stopped: not a crash
        };
        log::warn!("plugin {id}: its helper ended by itself (code {code:?})");
        if run.restarts.ended(Instant::now()) {
            let next = self.next_seq();
            run.seq = next;
            run.job = None;
            drop(g);
            (self.shared.notify)(Note::Restarted { id: id.to_string() });
            self.launch(id, next);
        } else {
            g.remove(id);
            drop(g);
            log::warn!("plugin {id}: its helper ended too often and stays stopped");
            (self.shared.notify)(Note::Stopped {
                id: id.to_string(),
                why: STOPPED_HELPER.to_string(),
            });
        }
    }
}

/// Copies one of the helper's outputs into its log, on a thread of its own, until it closes.
fn pump(mut from: impl Read + Send + 'static, log: Arc<Mutex<Log>>) {
    let spawned = std::thread::Builder::new()
        .name("flare-helper-log".into())
        .spawn(move || {
            let mut buf = vec![0u8; 8 * 1024];
            loop {
                match from.read(&mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => log
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .write(&buf[..n]),
                }
            }
        });
    if let Err(e) = spawned {
        log::debug!("a helper's log reader did not start: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(any(windows, target_os = "macos"))]
    use std::sync::mpsc;

    /// A plugin folder whose `helper.exe` is a copy of the system's cmd.exe (a real console program).
    #[cfg(windows)]
    fn folder_with_cmd() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let sys = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        fs::copy(
            format!(r"{sys}\System32\cmd.exe"),
            dir.path().join("helper.exe"),
        )
        .unwrap();
        dir
    }

    fn spec(dir: &Path, args: &[&str]) -> Spec {
        Spec {
            id: "demo".into(),
            folder: fs::canonicalize(dir).unwrap(),
            command: "helper.exe".into(),
            args: args.iter().map(|a| a.to_string()).collect(),
            token: "tok-123".into(),
        }
    }

    fn wait_until(what: &str, mut ok: impl FnMut() -> bool) {
        let t = Instant::now();
        while !ok() {
            assert!(t.elapsed() < Duration::from_secs(15), "timed out: {what}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn the_restart_policy_allows_three_ends_in_the_window_then_stops() {
        let mut r = Restarts::new(3, Duration::from_secs(300));
        let t0 = Instant::now();
        assert!(r.ended(t0));
        assert!(r.ended(t0 + Duration::from_secs(1)));
        assert!(r.ended(t0 + Duration::from_secs(2)));
        assert!(
            !r.ended(t0 + Duration::from_secs(3)),
            "the fourth end stops it"
        );
        // Out of the window, the old ends are forgotten.
        assert!(r.ended(t0 + Duration::from_secs(400)));
    }

    #[cfg(windows)]
    #[test]
    fn the_helper_gets_its_token_and_pipe_but_not_the_tab_secret_and_logs_its_output() {
        let dir = folder_with_cmd();
        let logs = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let host = Host::new(
            logs.path().to_path_buf(),
            r"\\.\pipe\flare-test".into(),
            move |n| {
                let _ = tx.send(n);
            },
        );
        // The test process may carry a tab token of its own (a Flare terminal): the helper must not.
        let s = spec(dir.path(), &["/d", "/c", "set FLARE_"]);
        host.start(s);
        let started = rx.recv_timeout(Duration::from_secs(10)).expect("a note");
        assert!(matches!(started, Note::Started { .. }), "{started:?}");
        let log = log_file(logs.path(), "demo");
        wait_until("the log has the variables", || {
            fs::read_to_string(&log).is_ok_and(|t| t.contains("FLARE_PLUGIN_TOKEN"))
        });
        // cmd ends at once: that end counts as a crash and it is started again; wait for the log to settle.
        host.stop("demo");
        let text = fs::read_to_string(&log).unwrap();
        assert!(text.contains("FLARE_PLUGIN_TOKEN=tok-123"), "{text}");
        assert!(
            text.contains(r"FLARE_CTL_PIPE=\\.\pipe\flare-test"),
            "{text}"
        );
        assert!(!text.contains("FLARE_TAB_TOKEN"), "{text}");
        assert!(!text.contains("FLARE_RESUME_ID"), "{text}");
    }

    #[cfg(windows)]
    #[test]
    fn a_stopped_helper_dies_with_its_job_and_is_not_restarted() {
        let dir = folder_with_cmd();
        let logs = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let host = Host::new(logs.path().to_path_buf(), "p".into(), move |n| {
            let _ = tx.send(n);
        });
        // A grandchild that would run for a minute: the job ends it too.
        host.start(spec(dir.path(), &["/d", "/c", "ping -n 60 127.0.0.1 >nul"]));
        let Note::Started { pid, .. } = rx.recv_timeout(Duration::from_secs(10)).unwrap() else {
            panic!("not started");
        };
        assert!(host.is_running("demo"));
        host.stop("demo");
        assert!(!host.is_running("demo"));
        wait_until("the helper process is gone", || !process_alive(pid));
        // No restart follows a stop.
        assert!(rx.recv_timeout(Duration::from_millis(800)).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn after_a_stop_and_wait_the_plugin_folder_can_be_moved_at_once() {
        // The helper and its grandchild run in the plugin folder; a terminated job ends them a moment later, and an
        // update renames that folder right after the wait.
        let parent = tempfile::tempdir().unwrap();
        let src = folder_with_cmd();
        let folder = parent.path().join("demo");
        let aside = parent.path().join("demo.old");
        fs::create_dir(&folder).unwrap();
        fs::copy(src.path().join("helper.exe"), folder.join("helper.exe")).unwrap();
        let logs = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let host = Host::new(logs.path().to_path_buf(), "p".into(), move |n| {
            let _ = tx.send(n);
        });
        for round in 0..5 {
            host.start(spec(&folder, &["/d", "/c", "ping -n 60 127.0.0.1 >nul"]));
            let started = rx.recv_timeout(Duration::from_secs(10)).expect("a note");
            assert!(matches!(started, Note::Started { .. }), "{started:?}");
            assert!(host.stop_and_wait("demo", STOP_WAIT), "round {round}");
            fs::rename(&folder, &aside).unwrap_or_else(|e| panic!("round {round}: {e}"));
            fs::rename(&aside, &folder).unwrap();
        }
        // Stopped while it still starts: the wait covers what its thread spawns after the stop.
        for round in 0..5 {
            host.start(spec(&folder, &["/d", "/c", "ping -n 60 127.0.0.1 >nul"]));
            assert!(host.stop_and_wait("demo", STOP_WAIT), "round {round}");
            fs::rename(&folder, &aside).unwrap_or_else(|e| panic!("round {round}: {e}"));
            fs::rename(&aside, &folder).unwrap();
        }
        // Nothing stopped: nothing to wait for.
        assert!(host.wait_stopped("other", Duration::ZERO));
    }

    #[cfg(windows)]
    #[test]
    fn a_helper_that_keeps_ending_is_started_three_times_more_then_stopped() {
        let dir = folder_with_cmd();
        let logs = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let host = Host::new(logs.path().to_path_buf(), "p".into(), move |n| {
            let _ = tx.send(n);
        });
        host.start(spec(dir.path(), &["/d", "/c", "exit 3"]));
        let mut notes = Vec::new();
        loop {
            let n = rx.recv_timeout(Duration::from_secs(15)).expect("a note");
            let end = matches!(n, Note::Stopped { .. });
            notes.push(n);
            if end {
                break;
            }
        }
        let starts = notes
            .iter()
            .filter(|n| matches!(n, Note::Started { .. }))
            .count();
        let restarts = notes
            .iter()
            .filter(|n| matches!(n, Note::Restarted { .. }))
            .count();
        assert_eq!((starts, restarts), (4, 3), "{notes:?}");
        assert_eq!(
            notes.last(),
            Some(&Note::Stopped {
                id: "demo".into(),
                why: STOPPED_HELPER.into()
            })
        );
        assert!(!host.is_running("demo"));
    }

    #[cfg(windows)]
    #[test]
    fn only_an_exe_inside_the_folder_starts() {
        let dir = folder_with_cmd();
        let logs = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let host = Host::new(logs.path().to_path_buf(), "p".into(), move |n| {
            let _ = tx.send(n);
        });
        for bad in [
            "../helper.exe",
            r"C:\Windows\System32\cmd.exe",
            "missing.exe",
            "helper.bat",
        ] {
            let mut s = spec(dir.path(), &[]);
            s.command = bad.into();
            host.start(s);
            match rx.recv_timeout(Duration::from_secs(10)).expect("a note") {
                Note::Stopped { why, .. } => {
                    assert!(why.contains("helper.command"), "{bad}: {why}")
                }
                other => panic!("{bad} started: {other:?}"),
            }
        }
    }

    #[test]
    fn the_log_is_moved_aside_past_its_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.log");
        let mut log = Log::open(path.clone(), 10);
        log.write(b"0123456789");
        log.write(b"abc");
        assert_eq!(fs::read(&path).unwrap(), b"abc");
        assert_eq!(fs::read(dir.path().join("x.log.1")).unwrap(), b"0123456789");
    }

    #[test]
    fn the_token_is_not_in_the_debug_form() {
        let s = Spec {
            id: "demo".into(),
            folder: PathBuf::from("x"),
            command: "h.exe".into(),
            args: vec![],
            token: "secret-token".into(),
        };
        assert!(!format!("{s:?}").contains("secret-token"));
    }

    #[cfg(windows)]
    fn process_alive(pid: u32) -> bool {
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        const STILL_ACTIVE: u32 = 259;
        // SAFETY: a handle or null; a handle is queried once and closed.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return false;
            }
            let mut code = 0u32;
            let ok = GetExitCodeProcess(h, &mut code);
            CloseHandle(h);
            ok != 0 && code == STILL_ACTIVE
        }
    }

    // ---- macOS (port phase 6): the group, the guard, and no survivor --------------------------------------------

    /// A plugin folder whose `helper` is a shell script that runs `/bin/sh` with the helper's arguments.
    #[cfg(target_os = "macos")]
    fn folder_with_sh() -> tempfile::TempDir {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let h = dir.path().join("helper");
        fs::write(&h, "#!/bin/sh\nexec /bin/sh \"$@\"\n").unwrap();
        fs::set_permissions(&h, fs::Permissions::from_mode(0o755)).unwrap();
        dir
    }

    /// Marks `path` as downloaded, as a browser does (the attribute only; nothing is asked of Gatekeeper).
    #[cfg(target_os = "macos")]
    fn quarantine(path: &Path) {
        use std::os::unix::ffi::OsStrExt;
        let p = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        let value = b"0083;6730f000;Safari;";
        // SAFETY: NUL-terminated strings and a buffer of the length passed, all alive for the call.
        let r = unsafe {
            libc::setxattr(
                p.as_ptr(),
                c"com.apple.quarantine".as_ptr(),
                value.as_ptr().cast(),
                value.len(),
                0,
                0,
            )
        };
        assert_eq!(r, 0, "setxattr: {}", std::io::Error::last_os_error());
    }

    #[cfg(target_os = "macos")]
    fn mac_spec(dir: &Path, args: &[&str]) -> Spec {
        Spec {
            command: "helper".into(),
            ..spec(dir, args)
        }
    }

    /// The helper's script for a tree that tries every way out: a child, a job in a group of its own (`set -m`), an
    /// orphan grandchild (its subshell ends), and a process that leaves the session (`setsid`, made by this test binary,
    /// [`escapee`]). It writes their pids to `pids` in the plugin folder, then `ready`, and waits.
    #[cfg(target_os = "macos")]
    fn tree_script() -> String {
        let me = std::env::current_exe().unwrap();
        format!(
            "echo $$ >> pids; sleep 300 & echo $! >> pids; \
             set -m; sleep 300 & echo $! >> pids; set +m; \
             (sleep 300 & echo $! > orphan.tmp; mv orphan.tmp orphan) & \
             while [ ! -f orphan ]; do sleep 0.05; done; cat orphan >> pids; \
             FLARE_HELPER_ESCAPE=\"$PWD\" '{}' helper::tests::escapee --exact >/dev/null 2>&1 & \
             while [ ! -f escaped ]; do sleep 0.05; done; cat escaped >> pids; touch ready; wait",
            me.display()
        )
    }

    /// The pids `tree_script` wrote, once it is ready: the helper, its child, the job, the orphan, the escapee.
    #[cfg(target_os = "macos")]
    fn tree_pids(folder: &Path) -> Vec<u32> {
        wait_until("the helper's tree is up", || folder.join("ready").exists());
        let pids: Vec<u32> = fs::read_to_string(folder.join("pids"))
            .unwrap()
            .lines()
            .map(|l| l.trim().parse().unwrap())
            .collect();
        assert_eq!(pids.len(), 5, "{pids:?}");
        let lead = pids[0];
        assert_eq!(
            flare_proc::session_of(lead),
            Some(lead),
            "the helper leads a session"
        );
        assert_ne!(
            flare_proc::stat(pids[2]).unwrap().pgid,
            lead,
            "the job left the group"
        );
        assert_eq!(
            flare_proc::session_of(pids[4]),
            Some(pids[4]),
            "the escapee left the session"
        );
        // Let the guard see the escapee (it notes descendants every tick; the escapee stays the shell's child, so any
        // tick from here on sees it). Generous, for a loaded run: a stop notes descendants again before each pass, but
        // a helper killed from outside orphans the escapee at once.
        std::thread::sleep(flare_proc::guard::TICK * 8);
        pids
    }

    /// Kills the listed pids when a test ends, so a failed one leaves no process behind.
    #[cfg(target_os = "macos")]
    struct Reap(Vec<u32>);

    #[cfg(target_os = "macos")]
    impl Drop for Reap {
        fn drop(&mut self) {
            for p in &self.0 {
                flare_proc::signal(*p, flare_proc::Signal::Kill);
            }
        }
    }

    /// Not a test on its own: a guard started by this binary's tests runs as this test.
    #[cfg(target_os = "macos")]
    #[test]
    fn guard_entry() {
        flare_proc::guard::run_if_guard();
    }

    #[cfg(target_os = "macos")]
    fn all_gone(pids: &[u32]) {
        wait_until("every process of the tree has ended", || {
            pids.iter().all(|p| !flare_proc::alive(*p))
        });
        assert!(
            flare_proc::guard::wait_group_gone(pids[0], STOP_WAIT),
            "the group is empty"
        );
    }

    /// Not a test on its own: run by the tree script with `FLARE_HELPER_ESCAPE` set, it leaves its session, writes its
    /// pid to `escaped`, and sleeps.
    #[test]
    fn escapee() {
        let Some(dir) = std::env::var_os("FLARE_HELPER_ESCAPE") else {
            return;
        };
        #[cfg(target_os = "macos")]
        {
            let dir = PathBuf::from(dir);
            let pid = std::process::id();
            assert!(become_session_leader(), "setsid");
            fs::write(dir.join("escaped.tmp"), format!("{pid}\n")).unwrap();
            fs::rename(dir.join("escaped.tmp"), dir.join("escaped")).unwrap();
            std::thread::sleep(Duration::from_secs(300));
        }
        #[cfg(not(target_os = "macos"))]
        drop(dir);
    }

    #[cfg(target_os = "macos")]
    fn become_session_leader() -> bool {
        unsafe extern "C" {
            fn setsid() -> i32;
        }
        // SAFETY: no arguments; -1 means failure.
        unsafe { setsid() != -1 }
    }

    #[cfg(target_os = "macos")]
    fn mac_host(max_restarts: usize) -> (Host, tempfile::TempDir, mpsc::Receiver<Note>) {
        flare_proc::guard::use_test_entry("helper::tests::guard_entry").unwrap();
        let logs = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let host = Host::with_policy(
            logs.path().to_path_buf(),
            "/tmp/flare-test.sock".into(),
            move |n| {
                let _ = tx.send(n);
            },
            max_restarts,
            RESTART_WINDOW,
            LOG_MAX,
        );
        (host, logs, rx)
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_the_helper_gets_its_token_and_channel_but_not_the_tab_secret_and_logs_its_output() {
        let dir = folder_with_sh();
        let (host, logs, rx) = mac_host(0);
        host.start(mac_spec(dir.path(), &["-c", "env | grep FLARE_"]));
        let started = rx.recv_timeout(Duration::from_secs(10)).expect("a note");
        assert!(matches!(started, Note::Started { .. }), "{started:?}");
        // It ends at once: with no restart allowed it stays stopped.
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(10)),
            Ok(Note::Stopped { .. })
        ));
        let log = log_file(logs.path(), "demo");
        wait_until("the log has the variables", || {
            fs::read_to_string(&log).is_ok_and(|t| t.contains("FLARE_PLUGIN_TOKEN"))
        });
        let text = fs::read_to_string(&log).unwrap();
        assert!(text.contains("FLARE_PLUGIN_TOKEN=tok-123"), "{text}");
        assert!(
            text.contains("FLARE_CTL_PIPE=/tmp/flare-test.sock"),
            "{text}"
        );
        assert!(!text.contains("FLARE_TAB_TOKEN"), "{text}");
        assert!(!text.contains("FLARE_RESUME_ID"), "{text}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_a_stopped_helper_leaves_no_process_and_is_not_restarted() {
        let dir = folder_with_sh();
        let (host, _logs, rx) = mac_host(MAX_RESTARTS);
        host.start(mac_spec(dir.path(), &["-c", &tree_script()]));
        let Note::Started { pid, .. } = rx.recv_timeout(Duration::from_secs(10)).unwrap() else {
            panic!("not started");
        };
        let pids = tree_pids(dir.path());
        let _reap = Reap(pids.clone());
        assert_eq!(pids[0], pid);
        assert!(pids.iter().all(|p| flare_proc::alive(*p)));
        // The plugin is turned off (or unloaded, or updated): stop, then the wait an update makes.
        assert!(host.stop_and_wait("demo", STOP_WAIT));
        all_gone(&pids);
        assert!(!host.is_running("demo"));
        assert!(
            rx.recv_timeout(Duration::from_millis(800)).is_err(),
            "no restart after a stop"
        );
    }

    /// A stop that lands while the helper is being started (a plugin turned off, updated or removed straight after it
    /// was turned on) reaps what was spawned: no zombie is left in its group, so the wait an update or a removal makes
    /// returns at once instead of running to its timeout (found by the app's update test on a Mac, rule 286).
    #[cfg(target_os = "macos")]
    #[test]
    fn mac_a_helper_stopped_while_it_starts_is_reaped_and_the_wait_returns() {
        let dir = folder_with_sh();
        let (host, _logs, _rx) = mac_host(0);
        for i in 0..30 {
            host.start(mac_spec(dir.path(), &["-c", "sleep 60"]));
            if i % 3 == 1 {
                std::thread::sleep(Duration::from_millis(i));
            }
            host.stop("demo");
            // Without the reap the wait runs to its timeout and says false.
            assert!(
                host.wait_stopped("demo", STOP_WAIT),
                "start {i}: the stopped helper was never reaped"
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_a_killed_helper_takes_its_whole_tree_with_it() {
        let dir = folder_with_sh();
        let (host, _logs, rx) = mac_host(0);
        host.start(mac_spec(dir.path(), &["-c", &tree_script()]));
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(10)),
            Ok(Note::Started { .. })
        ));
        let pids = tree_pids(dir.path());
        let _reap = Reap(pids.clone());
        // Killed from outside: a crash of the helper, which a job's end would follow on Windows.
        assert!(flare_proc::signal(pids[0], flare_proc::Signal::Kill));
        all_gone(&pids);
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(10)),
            Ok(Note::Stopped { .. })
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_stop_all_ends_every_helper_tree() {
        let a = folder_with_sh();
        let b = folder_with_sh();
        let (host, _logs, rx) = mac_host(MAX_RESTARTS);
        host.start(mac_spec(a.path(), &["-c", &tree_script()]));
        host.start(Spec {
            id: "other".into(),
            ..mac_spec(b.path(), &["-c", &tree_script()])
        });
        for _ in 0..2 {
            assert!(matches!(
                rx.recv_timeout(Duration::from_secs(10)),
                Ok(Note::Started { .. })
            ));
        }
        let mut pids = tree_pids(a.path());
        pids.extend(tree_pids(b.path()));
        let _reap = Reap(pids.clone());
        host.stop_all();
        assert!(host.running().is_empty());
        wait_until("every process of both trees has ended", || {
            pids.iter().all(|p| !flare_proc::alive(*p))
        });
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_a_helper_that_keeps_ending_is_started_three_times_more_then_stopped() {
        let dir = folder_with_sh();
        let (host, _logs, rx) = mac_host(MAX_RESTARTS);
        host.start(mac_spec(
            dir.path(),
            &["-c", "sleep 300 & echo $! >> pids; exit 3"],
        ));
        let mut notes = Vec::new();
        loop {
            let n = rx.recv_timeout(Duration::from_secs(30)).expect("a note");
            let end = matches!(n, Note::Stopped { .. });
            notes.push(n);
            if end {
                break;
            }
        }
        let starts = notes
            .iter()
            .filter(|n| matches!(n, Note::Started { .. }))
            .count();
        let restarts = notes
            .iter()
            .filter(|n| matches!(n, Note::Restarted { .. }))
            .count();
        assert_eq!((starts, restarts), (4, 3), "{notes:?}");
        // Each start left a sleeping child; each end took it along.
        let pids: Vec<u32> = fs::read_to_string(dir.path().join("pids"))
            .unwrap()
            .lines()
            .map(|l| l.trim().parse().unwrap())
            .collect();
        assert_eq!(pids.len(), 4, "{pids:?}");
        wait_until("no child of any start is left", || {
            pids.iter().all(|p| !flare_proc::alive(*p))
        });
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_only_an_executable_inside_the_folder_starts() {
        use std::os::unix::fs::PermissionsExt;
        let dir = folder_with_sh();
        fs::write(dir.path().join("plain"), "#!/bin/sh\n").unwrap();
        // A Windows program, executable or not, and a downloaded one (rule 289): refused before anything runs.
        for name in ["run.exe", "fetched"] {
            fs::copy(dir.path().join("helper"), dir.path().join(name)).unwrap();
            fs::set_permissions(dir.path().join(name), fs::Permissions::from_mode(0o755)).unwrap();
        }
        quarantine(&dir.path().join("fetched"));
        assert!(crate::paths::quarantined(&dir.path().join("fetched")));
        assert!(!crate::paths::quarantined(&dir.path().join("helper")));
        let (host, _logs, rx) = mac_host(0);
        for (bad, why) in [
            ("../helper", "'..'"),
            ("/bin/sh", "absolute"),
            ("missing", "does not exist"),
            ("plain", "not executable"),
            ("run.exe", "Windows program"),
            ("bin\\helper", "'/'"),
            ("fetched", "quarantined"),
        ] {
            let mut s = mac_spec(dir.path(), &[]);
            s.command = bad.into();
            host.start(s);
            match rx.recv_timeout(Duration::from_secs(10)).expect("a note") {
                Note::Stopped { why: w, .. } => {
                    assert!(
                        w.contains("helpers.macos.command") && w.contains(why),
                        "{bad}: {w}"
                    )
                }
                other => panic!("{bad} started: {other:?}"),
            }
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_no_helper_process_outlives_a_crash_of_flare() {
        // A stand-in for Flare (this test binary, [`fake_flare`]) starts the helper's tree and is then killed with
        // SIGKILL: no exit code of its own runs, only the kernel's closing of its descriptors.
        let dir = folder_with_sh();
        let me = std::env::current_exe().unwrap();
        // Killed on a failed assertion too, so a failing run leaves no stand-in (and so no tree) behind.
        struct Standin(std::process::Child);
        impl Drop for Standin {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut flare = Standin(
            std::process::Command::new(me)
                .args(["helper::tests::fake_flare", "--exact", "--nocapture"])
                .env("FLARE_HELPER_FAKE", dir.path())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let pids = tree_pids(dir.path());
        let _reap = Reap(pids.clone());
        assert!(pids.iter().all(|p| flare_proc::alive(*p)));
        assert_eq!(
            flare_proc::stat(pids[0]).unwrap().ppid,
            flare.0.id(),
            "the helper is the stand-in's child"
        );
        flare.0.kill().unwrap();
        flare.0.wait().unwrap();
        all_gone(&pids);
    }

    /// Rule 284: a helper that finds its guard and kills it by pid still has its session and its group ended when it
    /// stops, by Flare's own pass (`end_session`): a `killpg` of the group alone missed the `set -m` job.
    #[cfg(target_os = "macos")]
    #[test]
    fn mac_a_helper_whose_guard_was_killed_still_has_its_session_and_group_ended() {
        let dir = folder_with_sh();
        let (host, _logs, rx) = mac_host(MAX_RESTARTS);
        host.start(mac_spec(dir.path(), &["-c", &tree_script()]));
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(10)),
            Ok(Note::Started { .. })
        ));
        let pids = tree_pids(dir.path());
        let _reap = Reap(pids.clone());
        let lead = pids[0];
        // The guard: an orphan in the helper's session with a group of its own.
        let guards: Vec<u32> = flare_proc::table()
            .into_iter()
            .filter(|r| {
                r.ppid == 1
                    && r.pgid == r.pid
                    && r.pid != lead
                    && flare_proc::session_of(r.pid) == Some(lead)
            })
            .map(|r| r.pid)
            .collect();
        assert_eq!(guards.len(), 1, "{guards:?}");
        assert!(flare_proc::signal(guards[0], flare_proc::Signal::Kill));
        wait_until("the guard is gone", || !flare_proc::alive(guards[0]));
        assert!(pids[..4].iter().all(|p| flare_proc::alive(*p)));
        assert!(host.stop_and_wait("demo", STOP_WAIT));
        // The helper, its child, the `set -m` job and the orphan were all in its session. (The escapee left the
        // session and only the guard knew it: a guard killed by pid cannot end it, the documented gap.)
        wait_until("the session has ended", || {
            pids[..4].iter().all(|p| !flare_proc::alive(*p))
        });
    }

    /// Rule 286: the helper is marked ended before anything reaps it, also when its end could not be seen unreaped
    /// (`waitid` failed, say `ECHILD` with SIGCHLD ignored), so a stop that races the reap never kills a recycled pid.
    /// When it was seen unreaped, Flare's pass over its session and group runs before the reap (the ids are still held).
    #[cfg(target_os = "macos")]
    #[test]
    fn mac_the_end_is_marked_before_the_reap_and_the_pass_runs_while_the_ids_are_held() {
        // No real helper: leader 0 is never signalled and no session is swept.
        for unreaped in [false, true] {
            let job = Job::new(0, None);
            assert_eq!(job.active(), 1);
            let at_reap = job.finish(unreaped, || {
                let s = job.state();
                (s.ended, s.swept, s.done)
            });
            assert_eq!(
                at_reap,
                (true, unreaped, false),
                "seen unreaped: {unreaped}"
            );
            let s = job.state();
            assert!(s.ended && s.swept && s.done);
            drop(s);
            assert_eq!(job.active(), 0);
        }
    }

    /// Rule 289: a helper is refused while any file of its plugin folder is quarantined, not only its program: a
    /// script that runs a downloaded binary beside it would hand that one to Gatekeeper.
    #[cfg(target_os = "macos")]
    #[test]
    fn mac_a_helper_is_refused_while_any_file_of_its_folder_is_quarantined() {
        let dir = folder_with_sh();
        fs::create_dir(dir.path().join("bin")).unwrap();
        fs::write(dir.path().join("bin/tool"), "#!/bin/sh\n").unwrap();
        quarantine(&dir.path().join("bin/tool"));
        assert!(!crate::paths::quarantined(&dir.path().join("helper")));
        let (host, _logs, rx) = mac_host(0);
        host.start(mac_spec(dir.path(), &["-c", "exit 0"]));
        match rx.recv_timeout(Duration::from_secs(10)).expect("a note") {
            Note::Stopped { why, .. } => {
                assert!(
                    why.contains("quarantined") && why.contains("bin/tool"),
                    "{why}"
                );
            }
            other => panic!("it started: {other:?}"),
        }
    }

    /// Not a test on its own: run by `mac_no_helper_process_outlives_a_crash_of_flare` with `FLARE_HELPER_FAKE` set
    /// to a plugin folder, it is Flare: it starts the helper's tree there and runs until it is killed.
    #[test]
    fn fake_flare() {
        let Some(dir) = std::env::var_os("FLARE_HELPER_FAKE") else {
            return;
        };
        #[cfg(target_os = "macos")]
        {
            let dir = PathBuf::from(dir);
            let (host, _logs, rx) = mac_host(0);
            host.start(mac_spec(&dir, &["-c", &tree_script()]));
            let _ = rx.recv_timeout(Duration::from_secs(10));
            std::thread::sleep(Duration::from_secs(300));
        }
        #[cfg(not(target_os = "macos"))]
        drop(dir);
    }
}
