//! Windows: a helper program that ends with Flare (rule 1053), the Windows match for [`crate`]'s macOS `guard`.
//!
//! The helper goes into a kill-on-close job object of its own: when Flare lets the [`Guard`] go, or Flare's process
//! ends in any way (a crash included, since Windows closes a dead process's handles), the job's last handle closes and
//! Windows ends every process in it, the helper and anything it started after it joined. Lifted from
//! `flare-dictation/src/job.rs` (Android plan PR A2: the emu host is guarded this way).
//!
//! A process the helper starts with `CREATE_BREAKAWAY_FROM_JOB` leaves the job (the job allows it), for the rare child
//! that must outlive the helper.

use std::io;
use std::os::windows::io::AsRawHandle;
use std::process::Child;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, STILL_ACTIVE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, JOB_OBJECT_LIMIT_BREAKAWAY_OK,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

/// `CREATE_NO_WINDOW`: a console program started with it gets no console window (rule 1370: nothing Flare starts
/// shows a window).
pub const NO_WINDOW: u32 = 0x0800_0000;
/// `CREATE_BREAKAWAY_FROM_JOB`.
pub const BREAKAWAY: u32 = 0x0100_0000;
/// `CREATE_NEW_PROCESS_GROUP`.
pub const NEW_GROUP: u32 = 0x0000_0200;

/// A kill-on-close job object. Dropping it closes the handle, which ends every process in it.
struct Job {
    handle: HANDLE,
}

// SAFETY: a job handle is a kernel object; the Win32 job calls are thread-safe on one handle.
unsafe impl Send for Job {}
// SAFETY: as above; the struct holds no other state.
unsafe impl Sync for Job {}

impl Job {
    fn new() -> io::Result<Job> {
        // SAFETY: null attributes and a null name make an unnamed job with the default security.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Job { handle };
        // SAFETY: zeroed is a valid value for this plain C struct; two flags are set below.
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
        // SAFETY: `job.handle` is the live job just made; the pointer and size describe `info`, which outlives the call.
        let ok = unsafe {
            SetInformationJobObject(
                job.handle,
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&info).cast(),
                u32::try_from(std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
                    .unwrap_or(0),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    fn contains(&self, pid: u32) -> bool {
        // SAFETY: OpenProcess returns a handle or null; a handle is used for one query and closed once.
        unsafe {
            let p = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if p.is_null() {
                return false;
            }
            let mut inside = 0;
            let asked = IsProcessInJob(p, self.handle, &mut inside);
            CloseHandle(p);
            asked != 0 && inside != 0
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateJobObjectW and is closed once, here.
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

/// Flare's hold on a guarded helper. Dropping it, or [`Guard::release`], ends the helper and its job's processes; so
/// does Flare's exit, however it happens.
pub struct Guard {
    job: Option<Job>,
}

impl std::fmt::Debug for Guard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Guard")
            .field("held", &self.job.is_some())
            .finish()
    }
}

impl Guard {
    /// Lets the helper go: Windows ends every process in its job now.
    pub fn release(&mut self) {
        self.job = None;
    }

    /// Whether process `pid` is in this guard's job.
    pub fn holds(&self, pid: u32) -> bool {
        self.job.as_ref().is_some_and(|j| j.contains(pid))
    }
}

/// Puts a just-started helper into a kill-on-close job of its own. Start it with [`NO_WINDOW`]. A child it started
/// before this call is not in the job (the emu host starts nothing until Flare's first request).
pub fn guard_child(child: &Child) -> io::Result<Guard> {
    let job = Job::new()?;
    // SAFETY: both handles are live: the job is ours, the process handle is owned by `child`.
    let ok = unsafe { AssignProcessToJobObject(job.handle, child.as_raw_handle().cast()) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Guard { job: Some(job) })
}

/// Whether process `pid` still runs.
pub fn pid_alive(pid: u32) -> bool {
    // SAFETY: OpenProcess returns a handle or null; a handle is used for one query and closed once.
    unsafe {
        let p = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if p.is_null() {
            return false;
        }
        let mut code = 0u32;
        let asked = GetExitCodeProcess(p, &mut code);
        CloseHandle(p);
        asked != 0 && code == STILL_ACTIVE as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    fn ping() -> Child {
        Command::new("ping")
            .args(["-n", "60", "127.0.0.1"])
            .creation_flags(NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("ping starts")
    }

    fn gone_soon(c: &mut Child) -> bool {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(10) {
            if c.try_wait().expect("try_wait").is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    /// Rule 1053 on Windows: a guarded helper ends when Flare lets it go, and only then.
    #[test]
    fn a_guarded_helper_ends_with_its_guard() {
        let mut c = ping();
        let mut g = guard_child(&c).expect("a guard");
        assert!(g.holds(c.id()));
        assert!(pid_alive(c.id()));
        std::thread::sleep(Duration::from_millis(200));
        assert!(c.try_wait().unwrap().is_none(), "it runs while held");
        g.release();
        assert!(gone_soon(&mut c), "it outlived its guard");
        assert!(!pid_alive(c.id()));
        // Dropping the guard ends it too.
        let mut c = ping();
        let g = guard_child(&c).expect("a guard");
        drop(g);
        assert!(gone_soon(&mut c));
        assert!(!pid_alive(u32::MAX - 1));
        assert!(format!("{:?}", guard_child(&ping()).unwrap()).contains("held: true"));
    }
}
