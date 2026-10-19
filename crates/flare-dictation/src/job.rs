//! The Job Object that reaps child processes (Task 17): one kill-on-close job for every dictation child.
//! When the last handle to the job closes (the app quits, or a test drops its job), Windows ends every
//! process still assigned to it, so no engine, Parakeet or other child outlives the app.

use std::io;
use std::os::windows::io::AsRawHandle;
use std::process::Child;
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

/// A kill-on-close job object. Dropping it closes the handle, which ends every process in it.
pub struct Job {
    handle: HANDLE,
}

// SAFETY: a job handle is a kernel object; the Win32 job calls are thread-safe on one handle.
unsafe impl Send for Job {}
// SAFETY: as above; the struct holds no other state, so shared references only make calls.
unsafe impl Sync for Job {}

impl Job {
    /// A new job whose processes die when it closes.
    pub fn new() -> io::Result<Self> {
        // SAFETY: null attributes and a null name make an unnamed job with the default security.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Job { handle };
        // SAFETY: `info` is fully initialised (zeroed, then one flag set) and lives for the call; its size is passed.
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: `job.handle` is the live job just created; the info pointer and size describe `info` above.
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

    /// Puts a child process into the job. Its own children created afterwards join it too.
    pub fn assign(&self, child: &Child) -> io::Result<()> {
        // SAFETY: both handles are live: the job is owned by `self`, the process by `child`.
        let ok = unsafe { AssignProcessToJobObject(self.handle, child.as_raw_handle().cast()) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: the handle was returned by CreateJobObjectW and is closed exactly once, here.
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

/// The one job every dictation child joins for the life of the process (never closed).
pub fn shared() -> Option<&'static Job> {
    static SHARED: OnceLock<Option<Job>> = OnceLock::new();
    SHARED.get_or_init(|| Job::new().ok()).as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    #[test]
    fn a_child_in_the_job_dies_with_the_job() {
        // A single long-running Windows binary (ping, 60 s), so no grandchild escapes the job.
        let job = Job::new().expect("a job object");
        let mut child = Command::new("ping")
            .args(["-n", "60", "127.0.0.1"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("ping starts");
        job.assign(&child).expect("the child joins the job");
        assert!(
            child.try_wait().expect("try_wait").is_none(),
            "the child runs before the job closes"
        );
        drop(job);
        // Windows ends a job's processes with exit code 0, so the proof is the time: ping would run 60 s.
        let started = Instant::now();
        loop {
            if child.try_wait().expect("try_wait").is_some() {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "the child outlived its job"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
