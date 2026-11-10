//! A native process snapshot for the agent poll: every process as (pid, parent pid), and the
//! command line of the descendants of the live shells that pass a cheap word test. No PowerShell,
//! no WMI (decision D4).

use std::collections::{HashMap, HashSet};

#[cfg(windows)]
use windows_sys::Wdk::System::Threading::{
    NtQueryInformationProcess, ProcessCommandLineInformation,
};
#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE, UNICODE_STRING};
#[cfg(windows)]
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcEntry {
    pub pid: u32,
    pub ppid: u32,
    pub cmd: String,
}

/// A command line longer than this is cut (at a char boundary).
pub const CMD_CAP: usize = 32 * 1024;

/// Pure: the descendants of `roots` in a (pid, ppid) list, cycle-safe, roots excluded.
pub fn descendants(rows: &[(u32, u32)], roots: &[u32]) -> Vec<u32> {
    descendants_where(rows, roots, &|_, _| true)
}

/// Pure: whether a process may be the child of the parent it names. A process keeps its dead
/// parent's id, and Windows reuses ids: a child created BEFORE its listed parent only carries a
/// recycled number. An unreadable creation time proves nothing, so that edge stands.
pub fn born_after(parent: Option<u64>, child: Option<u64>) -> bool {
    match (parent, child) {
        (Some(p), Some(c)) => c >= p,
        _ => true,
    }
}

/// `descendants` over the edges `edge(parent, child)` accepts.
pub fn descendants_where(
    rows: &[(u32, u32)],
    roots: &[u32],
    edge: &dyn Fn(u32, u32) -> bool,
) -> Vec<u32> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for &(pid, ppid) in rows {
        children.entry(ppid).or_default().push(pid);
    }
    let mut seen: HashSet<u32> = roots.iter().copied().collect();
    let mut out = Vec::new();
    let mut queue: Vec<u32> = roots.to_vec();
    let mut i = 0;
    while i < queue.len() {
        let p = queue[i];
        i += 1;
        if let Some(kids) = children.get(&p) {
            for &k in kids {
                if !edge(p, k) {
                    continue;
                }
                if seen.insert(k) {
                    out.push(k);
                    queue.push(k);
                }
            }
        }
    }
    out
}

/// Pure: UTF-16 units as a String (lossy), cut at CMD_CAP chars.
pub fn cmd_from_utf16(units: &[u16]) -> String {
    let s = String::from_utf16_lossy(units);
    match s.char_indices().nth(CMD_CAP) {
        Some((at, _)) => s[..at].to_string(),
        None => s,
    }
}

#[cfg(windows)]
const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004u32 as i32;
#[cfg(windows)]
const STATUS_BUFFER_OVERFLOW: i32 = 0x8000_0005u32 as i32;
#[cfg(windows)]
const STATUS_BUFFER_TOO_SMALL: i32 = 0xC000_0023u32 as i32;

/// The command line of one process; empty when it cannot be opened or read.
#[cfg(windows)]
fn command_line(pid: u32) -> String {
    // SAFETY: OpenProcess takes plain values; a null handle means failure and is checked.
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h.is_null() {
        return String::new();
    }
    let mut len: u32 = 0;
    // SAFETY: a null buffer with length 0 asks only for the size; `len` is a valid out pointer.
    let st = unsafe {
        NtQueryInformationProcess(
            h,
            ProcessCommandLineInformation,
            std::ptr::null_mut(),
            0,
            &mut len,
        )
    };
    let mut out = String::new();
    if (st == STATUS_INFO_LENGTH_MISMATCH
        || st == STATUS_BUFFER_OVERFLOW
        || st == STATUS_BUFFER_TOO_SMALL)
        && len as usize >= std::mem::size_of::<UNICODE_STRING>()
    {
        let words = (len as usize).div_ceil(8);
        let mut buf: Vec<u64> = vec![0; words];
        let bytes = words * 8;
        let mut ret: u32 = 0;
        // SAFETY: `buf` is `bytes` long and 8-byte aligned; `ret` is a valid out pointer.
        let st2 = unsafe {
            NtQueryInformationProcess(
                h,
                ProcessCommandLineInformation,
                buf.as_mut_ptr().cast(),
                bytes as u32,
                &mut ret,
            )
        };
        if st2 >= 0 {
            let base = buf.as_ptr() as usize;
            // SAFETY: the buffer is aligned and at least a UNICODE_STRING long (checked above).
            let us = unsafe { std::ptr::read(buf.as_ptr().cast::<UNICODE_STRING>()) };
            let start = us.Buffer as usize;
            let n = (us.Length / 2) as usize;
            if n > 0 && start >= base && start.is_multiple_of(2) && start + n * 2 <= base + bytes {
                // SAFETY: the range [start, start + 2n) lies inside `buf`, is 2-aligned, initialised.
                let units = unsafe { std::slice::from_raw_parts(start as *const u16, n) };
                out = cmd_from_utf16(units);
            }
        }
    }
    // SAFETY: `h` came from a successful OpenProcess and is closed once.
    unsafe { CloseHandle(h) };
    out
}

/// Waits until process `pid` has ended, at most `max`. True when it is gone (or was never there to open).
#[cfg(windows)]
pub fn wait_for_exit(pid: u32, max: std::time::Duration) -> bool {
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::Threading::{PROCESS_SYNCHRONIZE, WaitForSingleObject};
    // SAFETY: plain values in; a null handle means the process is gone or not ours, and is checked.
    let h = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if h.is_null() {
        return true;
    }
    let ms = u32::try_from(max.as_millis()).unwrap_or(u32::MAX);
    // SAFETY: `h` is a live handle opened with SYNCHRONIZE.
    let r = unsafe { WaitForSingleObject(h, ms) };
    // SAFETY: `h` came from a successful OpenProcess and is closed once.
    unsafe { CloseHandle(h) };
    r == WAIT_OBJECT_0
}

/// When the process was created (FILETIME ticks); None when it cannot be opened or read.
#[cfg(windows)]
fn creation_time(pid: u32) -> Option<u64> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::GetProcessTimes;
    // SAFETY: plain values in; a null handle means failure and is checked.
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h.is_null() {
        return None;
    }
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut cr, mut ex, mut kr, mut us) = (zero, zero, zero, zero);
    // SAFETY: `h` is a live handle; the four out structs are live.
    let ok = unsafe { GetProcessTimes(h, &mut cr, &mut ex, &mut kr, &mut us) } != 0;
    // SAFETY: `h` came from a successful OpenProcess and is closed once.
    unsafe { CloseHandle(h) };
    ok.then(|| (u64::from(cr.dwHighDateTime) << 32) | u64::from(cr.dwLowDateTime))
}

/// BLOCKING (the poll's worker). Every process as (pid, ppid); `cmd` is read ONLY for descendants of
/// `roots` and kept only when `wanted(&cmd)` says so. None when the snapshot itself fails or is empty.
#[cfg(windows)]
pub fn snapshot_processes(roots: &[u32], wanted: &dyn Fn(&str) -> bool) -> Option<Vec<ProcEntry>> {
    // SAFETY: plain values in; an invalid handle is checked.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE || snap.is_null() {
        return None;
    }
    let mut rows: Vec<(u32, u32)> = Vec::new();
    // SAFETY: PROCESSENTRY32W is plain data; zero is a valid starting state before dwSize is set.
    let mut e: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: `snap` is a live snapshot handle and `e` has its size set.
    let mut ok = unsafe { Process32FirstW(snap, &mut e) } != 0;
    while ok {
        rows.push((e.th32ProcessID, e.th32ParentProcessID));
        // SAFETY: as above.
        ok = unsafe { Process32NextW(snap, &mut e) } != 0;
    }
    // SAFETY: `snap` is closed once.
    unsafe { CloseHandle(snap) };
    if rows.is_empty() {
        return None;
    }
    // Each process's creation time is read once, and only for processes on an edge the walk tries.
    let times = std::cell::RefCell::new(HashMap::<u32, Option<u64>>::new());
    let created = |pid: u32| {
        *times
            .borrow_mut()
            .entry(pid)
            .or_insert_with(|| creation_time(pid))
    };
    let want: HashSet<u32> = descendants_where(&rows, roots, &|parent, child| {
        born_after(created(parent), created(child))
    })
    .into_iter()
    .collect();
    Some(
        rows.into_iter()
            .map(|(pid, ppid)| {
                let cmd = if want.contains(&pid) {
                    let c = command_line(pid);
                    if wanted(&c) { c } else { String::new() }
                } else {
                    String::new()
                };
                ProcEntry { pid, ppid, cmd }
            })
            .collect(),
    )
}

/// Waits until process `pid` has ended, at most `max`. True when it is gone. Off Windows it polls
/// `kill(pid, 0)`, which sends no signal and only asks whether the pid exists. Phase 2 of the port
/// plan replaces the poll with a kqueue `NOTE_EXIT` wait.
#[cfg(not(windows))]
pub fn wait_for_exit(pid: u32, max: std::time::Duration) -> bool {
    use std::time::{Duration, Instant};
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    const ESRCH: i32 = 3;
    let Ok(pid) = i32::try_from(pid) else {
        return true;
    };
    if pid <= 0 {
        // 0 and negative pids name process groups, never one process.
        return true;
    }
    let alive = || {
        // SAFETY: signal 0 sends nothing; kill only checks that the pid exists.
        if unsafe { kill(pid, 0) } == 0 {
            return true;
        }
        // EPERM: it exists but belongs to someone else. Only ESRCH means it is gone.
        std::io::Error::last_os_error().raw_os_error() != Some(ESRCH)
    };
    let started = Instant::now();
    loop {
        if !alive() {
            return true;
        }
        let left = max.saturating_sub(started.elapsed());
        if left.is_zero() {
            return false;
        }
        std::thread::sleep(left.min(Duration::from_millis(50))); // sync-ok: startup only, before any window
    }
}

/// macOS: phase 2 of the port plan (libproc for the child tree). None: the snapshot fails.
#[cfg(not(windows))]
pub fn snapshot_processes(
    _roots: &[u32],
    _wanted: &dyn Fn(&str) -> bool,
) -> Option<Vec<ProcEntry>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn waiting_for_an_ended_process_returns_at_once_and_a_live_one_times_out() {
        let mut child = std::process::Command::new("cmd")
            .args(["/c", "exit"])
            .spawn()
            .unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert!(wait_for_exit(pid, std::time::Duration::from_secs(2)));
        let me = std::process::id();
        let t = std::time::Instant::now();
        assert!(!wait_for_exit(me, std::time::Duration::from_millis(100)));
        assert!(t.elapsed() >= std::time::Duration::from_millis(90));
    }

    #[cfg(not(windows))]
    #[test]
    fn waiting_for_an_ended_process_returns_at_once_and_a_live_one_times_out() {
        let mut child = std::process::Command::new("/usr/bin/true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert!(wait_for_exit(pid, std::time::Duration::from_secs(2)));
        let me = std::process::id();
        let t = std::time::Instant::now();
        assert!(!wait_for_exit(me, std::time::Duration::from_millis(100)));
        assert!(t.elapsed() >= std::time::Duration::from_millis(90));
    }

    #[test]
    fn descendants_walks_the_tree_and_survives_a_cycle() {
        let rows = [(10, 1), (20, 10), (30, 20), (40, 1), (5, 6), (6, 5)];
        let mut d = descendants(&rows, &[10]);
        d.sort();
        assert_eq!(d, vec![20, 30]);
        assert!(descendants(&rows, &[40]).is_empty());
        assert_eq!(descendants(&rows, &[5]), vec![6]);
        assert!(descendants(&rows, &[999]).is_empty());
    }

    #[test]
    fn a_process_older_than_its_listed_parent_is_not_its_child() {
        // 30 (an agent started long ago) lists 20 as its parent, but 20's number was reused by a
        // newer process under shell 10: 30 is an orphan, not a descendant of the tab.
        let rows = [(20, 10), (30, 20), (40, 20)];
        let time = |pid: u32| match pid {
            10 => Some(100),
            20 => Some(500),
            30 => Some(50),
            40 => Some(600),
            _ => None,
        };
        let mut d = descendants_where(&rows, &[10], &|p, c| born_after(time(p), time(c)));
        d.sort();
        assert_eq!(d, vec![20, 40]);
        // An unreadable time keeps the edge.
        assert!(born_after(None, Some(1)) && born_after(Some(1), None));
        assert!(born_after(Some(5), Some(5)));
    }

    #[test]
    fn cmd_from_utf16_is_lossy_and_capped() {
        let u: Vec<u16> = "claude --resume x".encode_utf16().collect();
        assert_eq!(cmd_from_utf16(&u), "claude --resume x");
        assert_eq!(cmd_from_utf16(&[0x61, 0xD800, 0x62]), "a\u{FFFD}b");
        let big = vec![b'a' as u16; 40_000];
        assert_eq!(cmd_from_utf16(&big).chars().count(), CMD_CAP);
    }

    #[cfg(windows)]
    #[test]
    fn a_real_child_command_line_is_read_and_filtered() {
        use std::os::windows::process::CommandExt;
        let mut child = std::process::Command::new("cmd.exe")
            .raw_arg(r#"/d /c "ping -n 8 127.0.0.1 >nul & rem claude-code-test""#)
            .creation_flags(0x0800_0000)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("spawn cmd"); // sync-ok: test code on the test thread only
        std::thread::sleep(std::time::Duration::from_millis(300)); // sync-ok: test code on the test thread only
        let rows = snapshot_processes(&[std::process::id()], &|c| {
            c.to_lowercase().contains("claude")
        });
        let me = child.id();
        // Killing cmd alone leaves its ping running for seconds: end the whole tree this test made.
        let _ = std::process::Command::new("taskkill.exe")
            .args(["/T", "/F", "/PID", &me.to_string()])
            .creation_flags(0x0800_0000)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status(); // sync-ok: test code on the test thread only
        let _ = child.kill();
        let _ = child.wait(); // sync-ok: test code on the test thread only
        let rows = rows.expect("snapshot");
        let c = rows.iter().find(|r| r.pid == me).expect("child present");
        assert!(c.cmd.contains("claude-code-test"), "cmd was {:?}", c.cmd);
        let grand = rows
            .iter()
            .find(|r| r.ppid == me)
            .expect("ping grandchild present");
        assert!(grand.cmd.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn the_snapshot_includes_every_process_even_without_roots() {
        let rows = snapshot_processes(&[], &|_| true).expect("snapshot");
        let me = rows
            .iter()
            .find(|r| r.pid == std::process::id())
            .expect("self");
        assert!(me.ppid != 0);
        assert!(rows.iter().all(|r| r.cmd.is_empty()));
    }

    #[cfg(windows)]
    #[test]
    fn an_unopenable_process_is_not_an_error() {
        assert!(snapshot_processes(&[4, 1], &|_| true).is_some());
    }

    #[test]
    fn no_unmarked_blocking_calls_in_src() {
        let src = include_str!("procs.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();
        let needle = ["Command", "::", "new"].concat();
        assert!(!prod.contains(&needle));
    }
}
