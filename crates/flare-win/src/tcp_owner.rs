//! Who owns a loopback port (audit L21, L27). The dictation engine picks a free port, starts the server on it and
//! waits for "GET /" to answer; any other program that grabbed the port in between would answer it
//! and then be handed the user's audio. The TCP table names the process behind every listener, so the
//! engine trusts the port only when every listener on it is its own server, and the Sonara post sends
//! its bearer token only to the process runtime.json names.

use std::collections::HashSet;

/// True when `owners` (the pids listening on the port) is not empty and every one is `pid`.
pub fn only_owner(owners: &[u32], pid: u32) -> bool {
    !owners.is_empty() && owners.iter().all(|o| *o == pid)
}

/// The listener table of one address family as the system gives it (`TCP_TABLE_OWNER_PID_LISTENER`): the bytes of the
/// table, or None when it cannot be read.
#[cfg(windows)]
fn listener_table(family: u16) -> Option<Vec<u8>> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, TCP_TABLE_OWNER_PID_LISTENER,
    };

    let mut size: u32 = 0;
    // SAFETY: a null table with size 0 only asks for the size needed.
    unsafe {
        GetExtendedTcpTable(
            std::ptr::null_mut(),
            &mut size,
            0,
            u32::from(family),
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        );
    }
    // The table can grow between the two calls; a few tries.
    for _ in 0..4 {
        if size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size as usize + 512];
        let mut len = buf.len() as u32;
        // SAFETY: `buf` is `len` bytes, writable, and the call writes at most `len` bytes.
        let rc = unsafe {
            GetExtendedTcpTable(
                buf.as_mut_ptr().cast(),
                &mut len,
                0,
                u32::from(family),
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };
        if rc == 0 {
            return Some(buf);
        }
        size = len;
    }
    None
}

/// The pids listening on `port` in one family's table; None when the table cannot be read (which is not the same as
/// "nobody listens").
#[cfg(windows)]
fn read_listeners(family: u16, port: u16) -> Option<Vec<u32>> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID,
    };
    use windows_sys::Win32::Networking::WinSock::AF_INET6;

    let buf = listener_table(family)?;
    if buf.len() < 4 {
        return None;
    }
    let count = u32::from_le_bytes(buf[0..4].try_into().unwrap_or([0; 4])) as usize;
    let v6 = family == AF_INET6;
    let row_size = if v6 {
        std::mem::size_of::<MIB_TCP6ROW_OWNER_PID>()
    } else {
        std::mem::size_of::<MIB_TCPROW_OWNER_PID>()
    };
    // The rows start after the count.
    let mut out = Vec::new();
    for i in 0..count {
        let at = 4 + i * row_size;
        if at + row_size > buf.len() {
            break;
        }
        // SAFETY: the row lies inside `buf`; the reads are unaligned-safe.
        let (local, pid) = unsafe {
            if v6 {
                let row: MIB_TCP6ROW_OWNER_PID =
                    std::ptr::read_unaligned(buf.as_ptr().add(at).cast());
                (row.dwLocalPort, row.dwOwningPid)
            } else {
                let row: MIB_TCPROW_OWNER_PID =
                    std::ptr::read_unaligned(buf.as_ptr().add(at).cast());
                (row.dwLocalPort, row.dwOwningPid)
            }
        };
        // The port is in network byte order in the low 16 bits.
        if u16::from_be((local & 0xffff) as u16) == port {
            out.push(pid);
        }
    }
    Some(out)
}

/// The pids of the processes listening on IPv4 `port`, from the TCP table. Empty when nobody is, or
/// when the table cannot be read (which then reads as "not trusted").
#[cfg(windows)]
pub fn listener_pids(port: u16) -> Vec<u32> {
    use windows_sys::Win32::Networking::WinSock::AF_INET;
    read_listeners(AF_INET, port).unwrap_or_default()
}

/// The pids of the processes listening on IPv6 `port` (a server on `[::1]` or `[::]`), from the TCP table.
#[cfg(windows)]
pub fn listener_pids_v6(port: u16) -> Vec<u32> {
    use windows_sys::Win32::Networking::WinSock::AF_INET6;
    read_listeners(AF_INET6, port).unwrap_or_default()
}

/// True when some process listens on `port` on either address family. Read from the system's table; nothing is ever
/// connected to, so the server being looked for sees no client. An unreadable table reads as "no".
pub fn is_listening(port: u16) -> bool {
    !listener_pids(port).is_empty() || !listener_pids_v6(port).is_empty()
}

/// The most process rows one snapshot is read for (a machine has thousands, never millions).
#[cfg(windows)]
const SNAPSHOT_MAX: usize = 200_000;

/// One process of a snapshot: its pid, its parent's pid and when it was created (FILETIME ticks; 0 when that could
/// not be read).
pub type ProcRow = (u32, u32, u64);

/// `root` and every process below it. Each pid is visited once, so a cycle ends the walk, and the set never outgrows
/// the rows. A parent link counts only when both processes' creation times are known and the child is not older than
/// its parent: a child older than its parent means the parent id was REUSED by a newer process, and the link is
/// false (the standard check against pid reuse).
pub fn descendants(rows: &[ProcRow], root: u32) -> HashSet<u32> {
    let mut set = HashSet::new();
    if root == 0 {
        return set;
    }
    set.insert(root);
    let created: std::collections::HashMap<u32, u64> =
        rows.iter().map(|(p, _, t)| (*p, *t)).collect();
    let mut grew = true;
    while grew {
        grew = false;
        for (pid, parent, born) in rows {
            if *pid == 0 || !set.contains(parent) || set.contains(pid) {
                continue;
            }
            let parent_born = created.get(parent).copied().unwrap_or(0);
            if *born != 0 && parent_born != 0 && *born >= parent_born {
                set.insert(*pid);
                grew = true;
            }
        }
    }
    set
}

/// The creation time of a process, or 0 when it cannot be opened or read.
#[cfg(windows)]
fn creation_time(pid: u32) -> u64 {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: OpenProcess takes plain values; a null handle means failure and is checked.
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h.is_null() {
        return 0;
    }
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut c, mut e, mut k, mut u) = (zero, zero, zero, zero);
    // SAFETY: `h` is a live handle and the four out-pointers are valid FILETIMEs.
    let ok = unsafe { GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u) } != 0;
    // SAFETY: `h` is closed once.
    unsafe { CloseHandle(h) };
    if ok {
        (u64::from(c.dwHighDateTime) << 32) | u64::from(c.dwLowDateTime)
    } else {
        0
    }
}

/// Every process from one Toolhelp snapshot (bounded), with its creation time. None when the snapshot cannot be taken.
#[cfg(windows)]
fn process_rows() -> Option<Vec<ProcRow>> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    // SAFETY: plain values in; an invalid handle is checked.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE || snap.is_null() {
        return None;
    }
    let mut rows = Vec::new();
    // SAFETY: PROCESSENTRY32W is plain data; zero is valid before dwSize is set.
    let mut e: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: `snap` is a live snapshot handle and `e` has its size set.
    let mut ok = unsafe { Process32FirstW(snap, &mut e) } != 0;
    while ok && rows.len() < SNAPSHOT_MAX {
        rows.push((e.th32ProcessID, e.th32ParentProcessID, 0));
        // SAFETY: as above.
        ok = unsafe { Process32NextW(snap, &mut e) } != 0;
    }
    // SAFETY: `snap` is closed once.
    unsafe { CloseHandle(snap) };
    for r in &mut rows {
        r.2 = creation_time(r.0);
    }
    Some(rows)
}

/// What the TCP table says of one port, held against a tab's process tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeLook {
    /// Nobody listens on the port.
    Nobody,
    /// Somebody listens and every listener is the tab's shell or one of its descendants.
    InTree,
    /// At least one listener is a process outside the tab's tree.
    Foreign,
    /// A table (or the process list) could not be read and nothing foreign was seen: unknown, never trusted.
    Unreadable,
}

/// The pure part of [`tree_look`]: the listeners each family's table named (None when that table could not be read)
/// against the tree. A foreign listener seen in either table decides at once; with a table missing and nothing
/// foreign seen, the answer is Unreadable, because the missing family may hold a foreign listener.
pub fn classify(v4: Option<&[u32]>, v6: Option<&[u32]>, tree: Option<&HashSet<u32>>) -> TreeLook {
    let owners: Vec<u32> = v4.into_iter().chain(v6).flatten().copied().collect();
    if !owners.is_empty() {
        let Some(tree) = tree else {
            return TreeLook::Unreadable;
        };
        if owners.iter().any(|o| !tree.contains(o)) {
            return TreeLook::Foreign;
        }
    }
    if v4.is_none() || v6.is_none() {
        return TreeLook::Unreadable;
    }
    if owners.is_empty() {
        TreeLook::Nobody
    } else {
        TreeLook::InTree
    }
}

/// Looks at `port` against the tree of `root` (the tab's shell). Run it on a worker.
#[cfg(windows)]
pub fn tree_look(port: u16, root: u32) -> TreeLook {
    use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};
    let v4 = read_listeners(AF_INET, port);
    let v6 = read_listeners(AF_INET6, port);
    let any =
        v4.as_ref().is_some_and(|v| !v.is_empty()) || v6.as_ref().is_some_and(|v| !v.is_empty());
    let tree = if !any {
        None
    } else if root == 0 {
        // No shell: nothing is in a tree.
        Some(HashSet::new())
    } else {
        process_rows().map(|rows| descendants(&rows, root))
    };
    classify(v4.as_deref(), v6.as_deref(), tree.as_ref())
}

/// True when something listens on `port` and EVERY listener on it (either address family) is `root` or one of its
/// descendants. A port held by any other process, or by a server outside the tab's process tree (WSL's
/// processes are not in it), reads as "no". Unreadable tables read as "no". Run it on a worker.
pub fn listening_in_tree(port: u16, root: u32) -> bool {
    root != 0 && tree_look(port, root) == TreeLook::InTree
}

/// macOS: phase 4 of the port plan (the socket table through libproc). Until then no table can be
/// read: nobody is ever trusted as a port's owner.
#[cfg(not(windows))]
pub fn listener_pids(_port: u16) -> Vec<u32> {
    Vec::new()
}

/// macOS: phase 4 of the port plan; empty until then.
#[cfg(not(windows))]
pub fn listener_pids_v6(_port: u16) -> Vec<u32> {
    Vec::new()
}

/// macOS: phase 4 of the port plan; both tables read as unreadable until then.
#[cfg(not(windows))]
pub fn tree_look(_port: u16, _root: u32) -> TreeLook {
    classify(None, None, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    use std::net::Ipv6Addr;
    use std::net::{Ipv4Addr, TcpListener};

    #[test]
    fn only_the_servers_own_pid_is_trusted() {
        assert!(only_owner(&[7], 7));
        assert!(only_owner(&[7, 7], 7));
        assert!(!only_owner(&[], 7));
        assert!(!only_owner(&[8], 7));
        assert!(!only_owner(&[7, 8], 7));
    }

    #[cfg(windows)]
    #[test]
    fn a_listener_in_this_process_names_this_process() {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let port = l.local_addr().expect("addr").port();
        assert_eq!(listener_pids(port), vec![std::process::id()]);
        drop(l);
        assert!(listener_pids(port).is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn a_port_is_listening_while_a_listener_holds_it_on_either_family() {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let port = l.local_addr().expect("addr").port();
        assert!(is_listening(port));
        drop(l);
        assert!(!is_listening(port));
        // A server on [::1] only (Node's `localhost` on a machine that resolves it to IPv6 first).
        if let Ok(l6) = TcpListener::bind((Ipv6Addr::LOCALHOST, 0)) {
            let port6 = l6.local_addr().expect("addr").port();
            assert_eq!(listener_pids_v6(port6), vec![std::process::id()]);
            assert!(is_listening(port6));
            drop(l6);
            assert!(!is_listening(port6));
        }
    }

    #[test]
    fn descendants_include_the_root_and_every_depth_and_nothing_else() {
        let rows = [
            (10, 1, 100),
            (11, 10, 110),
            (12, 11, 120),
            (13, 10, 130),
            (20, 2, 50),
            (21, 20, 60),
        ];
        let set = descendants(&rows, 10);
        assert_eq!(set, [10, 11, 12, 13].into_iter().collect());
        assert!(descendants(&rows, 0).is_empty());
        // A cycle (a reused parent id) ends.
        let cyc = [(5, 6, 10), (6, 5, 20)];
        assert_eq!(descendants(&cyc, 5), [5, 6].into_iter().collect());
    }

    #[test]
    fn a_reused_parent_id_does_not_pull_in_an_older_process() {
        // 30 is older than the process now holding pid 10 (its real parent exited and 10 was reused): not a child.
        let rows = [(10, 1, 100), (30, 10, 40), (31, 30, 45), (11, 10, 110)];
        assert_eq!(descendants(&rows, 10), [10, 11].into_iter().collect());
        // A creation time that could not be read is never trusted.
        let unknown = [(10, 1, 100), (12, 10, 0)];
        assert_eq!(descendants(&unknown, 10), [10].into_iter().collect());
    }

    #[test]
    fn an_unreadable_table_never_reads_as_in_tree() {
        let tree: HashSet<u32> = [7].into_iter().collect();
        assert_eq!(
            classify(Some(&[7]), Some(&[]), Some(&tree)),
            TreeLook::InTree
        );
        assert_eq!(
            classify(Some(&[]), Some(&[]), Some(&tree)),
            TreeLook::Nobody
        );
        // The v4 table is unreadable while v6 shows an in-tree owner: not trusted.
        assert_eq!(
            classify(None, Some(&[7]), Some(&tree)),
            TreeLook::Unreadable
        );
        assert_eq!(
            classify(Some(&[7]), None, Some(&tree)),
            TreeLook::Unreadable
        );
        assert_eq!(classify(None, Some(&[]), Some(&tree)), TreeLook::Unreadable);
        // A foreign owner seen in the readable table decides at once.
        assert_eq!(classify(None, Some(&[8]), Some(&tree)), TreeLook::Foreign);
        assert_eq!(
            classify(Some(&[7, 8]), Some(&[]), Some(&tree)),
            TreeLook::Foreign
        );
        // Owners but no process list: unknown.
        assert_eq!(classify(Some(&[7]), Some(&[]), None), TreeLook::Unreadable);
    }

    #[cfg(windows)]
    #[test]
    fn a_port_held_by_a_stranger_is_foreign_not_nobody() {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let port = l.local_addr().expect("addr").port();
        assert_eq!(tree_look(port, std::process::id()), TreeLook::InTree);
        assert_eq!(tree_look(port, 4), TreeLook::Foreign);
        drop(l);
        assert_eq!(tree_look(port, std::process::id()), TreeLook::Nobody);
    }

    #[cfg(windows)]
    #[test]
    fn a_port_owned_by_another_process_is_not_in_the_tree() {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let port = l.local_addr().expect("addr").port();
        // This process owns it: it is in its own tree, not in an unrelated one.
        assert!(listening_in_tree(port, std::process::id()));
        assert!(!listening_in_tree(port, 4));
        assert!(!listening_in_tree(port, 0));
        drop(l);
        assert!(!listening_in_tree(port, std::process::id()));
    }

    #[test]
    fn a_port_nobody_holds_is_not_listening() {
        // Bind and drop to learn a port that was free a moment ago, then ask about the one above it twice over.
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let port = l.local_addr().expect("addr").port();
        drop(l);
        assert!(!is_listening(port));
        assert!(listener_pids(port).is_empty());
        assert!(listener_pids_v6(port).is_empty());
    }
}
