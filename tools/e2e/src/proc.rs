//! M9 Task 36: e2e process control (start, reap what the scenario started).
//!
//! Every launched app goes into a job object with KILL_ON_JOB_CLOSE, so a crashed test leaves
//! nothing running. `reap_by_profile` also kills any process whose command line holds the
//! profile path, and never one whose command line lacks it. The job API is declared by hand
//! (windows-sys' JobObjects feature is not enabled in this workspace).

use std::ffi::c_void;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, GetExitCodeProcess, OpenProcess, TerminateProcess,
    PROCESS_QUERY_INFORMATION, PROCESS_TERMINATE, PROCESS_VM_READ,
};

#[repr(C)]
#[derive(Default)]
struct ExtendedLimit {
    per_process_user_time: i64,
    per_job_user_time: i64,
    limit_flags: u32,
    min_ws: usize,
    max_ws: usize,
    active_limit: u32,
    affinity: usize,
    priority_class: u32,
    scheduling_class: u32,
    io: [u64; 6],
    process_memory_limit: usize,
    job_memory_limit: usize,
    peak_process: usize,
    peak_job: usize,
}

const KILL_ON_JOB_CLOSE: u32 = 0x2000;
const EXTENDED_LIMIT_CLASS: i32 = 9;
const STILL_ACTIVE: u32 = 259;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateJobObjectW(attrs: *const c_void, name: *const u16) -> HANDLE;
    fn SetInformationJobObject(job: HANDLE, class: i32, info: *const c_void, len: u32) -> i32;
    fn AssignProcessToJobObject(job: HANDLE, process: HANDLE) -> i32;
}

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryInformationProcess(
        process: HANDLE,
        class: u32,
        info: *mut c_void,
        len: u32,
        ret_len: *mut u32,
    ) -> i32;
}

/// A job object that kills everything in it when the handle closes.
pub struct Job(HANDLE);

// SAFETY: a job handle is usable from any thread.
unsafe impl Send for Job {}
// SAFETY: the kernel serialises job calls.
unsafe impl Sync for Job {}

impl Job {
    pub fn new() -> Job {
        // SAFETY: null attributes and name are allowed; the handle is closed in Drop.
        let h = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        assert!(!h.is_null(), "CreateJobObjectW failed");
        let info = ExtendedLimit {
            limit_flags: KILL_ON_JOB_CLOSE,
            ..Default::default()
        };
        // SAFETY: info is a correctly laid out JOBOBJECT_EXTENDED_LIMIT_INFORMATION (144 bytes).
        let ok = unsafe {
            SetInformationJobObject(
                h,
                EXTENDED_LIMIT_CLASS,
                (&info as *const ExtendedLimit).cast(),
                size_of::<ExtendedLimit>() as u32,
            )
        };
        assert!(ok != 0, "SetInformationJobObject failed");
        Job(h)
    }

    pub fn assign(&self, child: &std::process::Child) -> bool {
        // SAFETY: both handles are valid for the call.
        unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle() as HANDLE) != 0 }
    }
}

impl Default for Job {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateJobObjectW and is closed once; this kills the job.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

/// The argv a launch passes: the caller's, plus `--e2e` and `--user-data-dir=<profile>` unless
/// the caller gave its own.
pub fn launch_args(args: &[&str], profile: &Path) -> Vec<String> {
    let mut v: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
    if !v.iter().any(|a| a == "--e2e") {
        v.push("--e2e".into());
    }
    if !v.iter().any(|a| a.starts_with("--user-data-dir")) {
        v.push(format!("--user-data-dir={}", profile.display()));
    }
    v
}

pub struct Child {
    proc: Mutex<std::process::Child>,
    profile: PathBuf,
    _job: Job,
}

impl Child {
    pub fn launch(exe: &Path, args: &[&str], env: &[(String, String)], profile: &Path) -> Child {
        let mut cmd = Command::new(exe);
        cmd.args(launch_args(args, profile));
        for (k, v) in env {
            cmd.env(k, v);
        }
        Self::spawn(cmd, profile)
    }

    /// The same job and reaping code for any command (the harness tests use a dummy).
    pub fn spawn(mut cmd: Command, profile: &Path) -> Child {
        let proc = cmd.spawn().expect("spawn");
        let job = Job::new();
        job.assign(&proc);
        Child {
            proc: Mutex::new(proc),
            profile: profile.to_path_buf(),
            _job: job,
        }
    }

    pub fn wait_for(&self, what: impl Fn() -> bool, timeout: Duration) -> bool {
        let end = Instant::now() + timeout;
        loop {
            if what() {
                return true;
            }
            if Instant::now() >= end {
                return false;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    pub fn exited(&self) -> Option<i32> {
        let mut p = self.proc.lock().unwrap_or_else(|e| e.into_inner());
        p.try_wait().ok().flatten().map(|s| s.code().unwrap_or(-1))
    }

    pub fn pid(&self) -> u32 {
        self.proc.lock().unwrap_or_else(|e| e.into_inner()).id()
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        {
            let mut p = self.proc.lock().unwrap_or_else(|e| e.into_inner());
            let _ = p.kill();
            let _ = p.wait();
        }
        reap_by_profile(&self.profile);
        // The job handle closes after this body, killing anything still inside it.
    }
}

fn process_ids() -> Vec<u32> {
    let mut out = Vec::new();
    // SAFETY: a plain snapshot of the process list; the handle is closed below.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return out;
    }
    // SAFETY: PROCESSENTRY32W is plain data.
    let mut e: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    e.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: e is sized and snap is valid.
    let mut ok = unsafe { Process32FirstW(snap, &mut e) };
    while ok != 0 {
        out.push(e.th32ProcessID);
        // SAFETY: as above.
        ok = unsafe { Process32NextW(snap, &mut e) };
    }
    // SAFETY: closes the snapshot once.
    unsafe {
        CloseHandle(snap);
    }
    out
}

/// Reads a process's command line out of its PEB (x64 layout). None when it cannot be read.
fn command_line(h: HANDLE) -> Option<String> {
    let mut pbi = [0usize; 6];
    let mut ret = 0u32;
    // SAFETY: pbi is the size of PROCESS_BASIC_INFORMATION on x64 (48 bytes).
    let st = unsafe { NtQueryInformationProcess(h, 0, pbi.as_mut_ptr().cast(), 48, &mut ret) };
    if st != 0 || pbi[1] == 0 {
        return None;
    }
    let read = |addr: usize, buf: *mut c_void, len: usize| -> bool {
        let mut got = 0usize;
        // SAFETY: buf has len bytes; a failed read is reported, not trusted.
        unsafe { ReadProcessMemory(h, addr as *const c_void, buf, len, &mut got) != 0 }
    };
    let mut params = 0usize;
    // PEB.ProcessParameters at +0x20.
    if !read(pbi[1] + 0x20, (&mut params as *mut usize).cast(), 8) || params == 0 {
        return None;
    }
    // RTL_USER_PROCESS_PARAMETERS.CommandLine (UNICODE_STRING) at +0x70.
    let mut us = [0u8; 16];
    if !read(params + 0x70, us.as_mut_ptr().cast(), 16) {
        return None;
    }
    let len = u16::from_le_bytes([us[0], us[1]]) as usize;
    let ptr = usize::from_le_bytes(us[8..16].try_into().ok()?);
    if len == 0 || ptr == 0 {
        return Some(String::new());
    }
    let mut buf = vec![0u16; len / 2];
    if !read(ptr, buf.as_mut_ptr().cast(), len) {
        return None;
    }
    Some(String::from_utf16_lossy(&buf))
}

/// Kills every process (except this one) whose command line contains `profile`, ignoring case
/// and slash direction. Returns how many it killed. A process whose command line cannot be read
/// is left alone.
pub fn reap_by_profile(profile: &Path) -> usize {
    let needle = norm(&profile.to_string_lossy());
    if needle.len() < 4 {
        return 0;
    }
    // SAFETY: no arguments.
    let me = unsafe { GetCurrentProcessId() };
    let mut killed = 0;
    for pid in process_ids() {
        if pid == me || pid == 0 {
            continue;
        }
        // SAFETY: a failed open returns null and is skipped.
        let h = unsafe {
            OpenProcess(
                PROCESS_QUERY_INFORMATION | PROCESS_VM_READ | PROCESS_TERMINATE,
                0,
                pid,
            )
        };
        if h.is_null() {
            continue;
        }
        if command_line(h).is_some_and(|c| norm(&c).contains(&needle)) {
            // SAFETY: h has PROCESS_TERMINATE.
            if unsafe { TerminateProcess(h, 1) } != 0 {
                killed += 1;
            }
        }
        // SAFETY: closes the handle once.
        unsafe {
            CloseHandle(h);
        }
    }
    killed
}

fn norm(s: &str) -> String {
    s.replace('/', "\\").to_lowercase()
}

/// True while a process with this id exists and has not exited.
pub fn pid_alive(pid: u32) -> bool {
    // SAFETY: a failed open returns null.
    let h = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION, 0, pid) };
    if h.is_null() {
        return false;
    }
    let mut code = 0u32;
    // SAFETY: h is valid.
    let ok = unsafe { GetExitCodeProcess(h, &mut code) };
    // SAFETY: closes the handle once.
    unsafe {
        CloseHandle(h);
    }
    ok != 0 && code == STILL_ACTIVE
}
