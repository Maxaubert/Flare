//! A native process snapshot for the agent poll: every process as (pid, parent pid), and the
//! command line of the descendants of the live shells that pass a cheap word test. No PowerShell,
//! no WMI (decision D4).

use std::collections::{HashMap, HashSet};

use windows_sys::Wdk::System::Threading::{
    NtQueryInformationProcess, ProcessCommandLineInformation,
};
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE, UNICODE_STRING};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
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

const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004u32 as i32;
const STATUS_BUFFER_OVERFLOW: i32 = 0x8000_0005u32 as i32;
const STATUS_BUFFER_TOO_SMALL: i32 = 0xC000_0023u32 as i32;

/// The command line of one process; empty when it cannot be opened or read.
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

/// BLOCKING (the poll's worker). Every process as (pid, ppid); `cmd` is read ONLY for descendants of
/// `roots` and kept only when `wanted(&cmd)` says so. None when the snapshot itself fails or is empty.
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
    let want: HashSet<u32> = descendants(&rows, roots).into_iter().collect();
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn cmd_from_utf16_is_lossy_and_capped() {
        let u: Vec<u16> = "claude --resume x".encode_utf16().collect();
        assert_eq!(cmd_from_utf16(&u), "claude --resume x");
        assert_eq!(cmd_from_utf16(&[0x61, 0xD800, 0x62]), "a\u{FFFD}b");
        let big = vec![b'a' as u16; 40_000];
        assert_eq!(cmd_from_utf16(&big).chars().count(), CMD_CAP);
    }

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
