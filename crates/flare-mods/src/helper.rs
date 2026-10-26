//! A plugin's helper program (plugins phase 7, design 4, 10 and 13): started while the plugin is on, stopped when it
//! turns off, and never outliving Flare.
//!
//! - **Which program.** Only the manifest's `helper.command`, an `.exe` inside the plugin folder, checked again when it
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

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
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
    /// `helper.command`, relative to the folder.
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

/// A kill-on-close job object: closing it, or terminating it, ends every process in it.
struct Job(HANDLE);

// SAFETY: a job handle is a kernel object; the job calls are thread-safe on one handle.
unsafe impl Send for Job {}
// SAFETY: as above; the struct holds nothing else, so a shared reference only makes calls.
unsafe impl Sync for Job {}

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
}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateJobObjectW and is closed once, here.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

/// Lets the threads of a process started suspended run. Error when none could be resumed.
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
    /// Nothing waits here; the helper's own thread sees it end.
    pub fn stop(&self, id: &str) {
        let job = {
            let mut g = self.lock();
            let Some(run) = g.remove(id) else { return };
            run.job
        };
        if let Some(job) = job {
            job.terminate();
        }
        log::info!("plugin {id}: its helper was stopped");
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
                        job.terminate();
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
    use std::sync::mpsc;

    /// A plugin folder whose `helper.exe` is a copy of the system's cmd.exe (a real console program).
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
}
