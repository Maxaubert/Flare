//! Who owns a loopback port (audit L21, L27). The dictation engine picks a free port, starts the server on it and
//! waits for "GET /" to answer; any other program that grabbed the port in between would answer it
//! and then be handed the user's audio. The TCP table names the process behind every listener, so the
//! engine trusts the port only when every listener on it is its own server, and the Sonara post sends
//! its bearer token only to the process runtime.json names.
//!
//! Windows reads the IP Helper table (`GetExtendedTcpTable`); macOS reads libproc's socket table
//! (`flare_proc::tcp_listeners`), which sees this user's processes only (regression rule 244).

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

/// macOS: the listeners on `port` of one family, from libproc's socket table
/// (`flare_proc::tcp_listeners`, never `lsof`, never a connection). None when the table cannot be read.
/// Only this user's processes are seen (regression rule 244).
#[cfg(not(windows))]
fn read_listeners(v6: bool, port: u16) -> Option<Vec<u32>> {
    let table = flare_proc::tcp_listeners()?;
    Some(listeners_in(&table, v6, port))
}

/// Pure: the pids of `table` listening on `port` in one family.
#[cfg(not(windows))]
fn listeners_in(table: &[flare_proc::TcpListen], v6: bool, port: u16) -> Vec<u32> {
    table
        .iter()
        .filter(|l| l.port == port && l.v6 == v6)
        .map(|l| l.pid)
        .collect()
}

/// macOS: the pids listening on IPv4 `port`. Empty when nobody is, or when the table cannot be read.
#[cfg(not(windows))]
pub fn listener_pids(port: u16) -> Vec<u32> {
    read_listeners(false, port).unwrap_or_default()
}

/// macOS: the pids listening on IPv6 `port` (a server on `[::1]` or `[::]`).
#[cfg(not(windows))]
pub fn listener_pids_v6(port: u16) -> Vec<u32> {
    read_listeners(true, port).unwrap_or_default()
}

/// macOS: every process with its parent and start (microseconds; 0 when unknown, as another user's
/// process is), from libproc. None when the table cannot be read.
#[cfg(not(windows))]
fn process_rows() -> Option<Vec<ProcRow>> {
    let table = flare_proc::table();
    if table.is_empty() {
        return None;
    }
    Some(
        table
            .into_iter()
            .map(|r| (r.pid, r.ppid, r.start_us.unwrap_or(0)))
            .collect(),
    )
}

/// macOS: looks at `port` against the tree of `root` (the tab's shell), one socket table read for both
/// families. An in-tree answer stands only when each loopback family the tree holds no listener on is
/// shown free as well ([`confirm_unseen_families`]): the table never names another account's listener,
/// and `localhost` may reach either family. Run it on a worker.
#[cfg(not(windows))]
pub fn tree_look(port: u16, root: u32) -> TreeLook {
    let table = flare_proc::tcp_listeners();
    let v4 = table.as_ref().map(|t| listeners_in(t, false, port));
    let v6 = table.as_ref().map(|t| listeners_in(t, true, port));
    let any =
        v4.as_ref().is_some_and(|v| !v.is_empty()) || v6.as_ref().is_some_and(|v| !v.is_empty());
    let tree = if !any {
        None
    } else if root == 0 {
        Some(HashSet::new())
    } else {
        process_rows().map(|rows| descendants(&rows, root))
    };
    let look = classify(v4.as_deref(), v6.as_deref(), tree.as_ref());
    confirm_unseen_families(
        look,
        v4.as_ref().is_some_and(|v| !v.is_empty()),
        v6.as_ref().is_some_and(|v| !v.is_empty()),
        |v6| loopback_bind_refused(v6, port),
    )
}

/// The pure part of the macOS family check (regression rules 244 and 250). The listener table shows this user's
/// processes only, so a tree that holds 127.0.0.1:P says nothing of [::1]:P, which another account may
/// hold, and `http://localhost:P` may reach either. So an `InTree` answer stands only when every family
/// with no listener in the table is shown free: `refused(v6)` is whether binding that family's loopback
/// address on the port was refused (Some(true): a holder the table cannot see), shown free (Some(false)),
/// or could not be told (None). Anything but "free" makes the answer `Unreadable`, never trusted.
/// Within ONE family the kernel already refuses another account's bind on the same or an overlapping
/// address (unless that account is root), so a family the tree holds needs no probe.
pub fn confirm_unseen_families(
    look: TreeLook,
    v4_seen: bool,
    v6_seen: bool,
    mut refused: impl FnMut(bool) -> Option<bool>,
) -> TreeLook {
    if look != TreeLook::InTree {
        return look;
    }
    for (v6, seen) in [(false, v4_seen), (true, v6_seen)] {
        if !seen && refused(v6) != Some(false) {
            return TreeLook::Unreadable;
        }
    }
    look
}

/// macOS: whether binding the loopback address of one family (127.0.0.1 or ::1) on `port` is refused,
/// with SO_REUSEADDR and never a `listen`, so no client can ever reach the probe. With SO_REUSEADDR a
/// wildcard listener of THIS user does not refuse it, while any listener of another account on that
/// address or the family's wildcard does (the kernel's per-uid check), which is the holder the table
/// cannot show. Some(true): refused; Some(false): free, or the family is missing (nothing can listen on
/// it); None: could not be told.
#[cfg(not(windows))]
pub fn loopback_bind_refused(v6: bool, port: u16) -> Option<bool> {
    let family = if v6 { libc::AF_INET6 } else { libc::AF_INET };
    // SAFETY: plain values in; the result is checked.
    let fd = unsafe { libc::socket(family, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        let e = std::io::Error::last_os_error();
        return (e.raw_os_error() == Some(libc::EAFNOSUPPORT)).then_some(false);
    }
    let one: libc::c_int = 1;
    let int_len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: a live socket; the option value is a c_int that outlives the call.
    unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_REUSEADDR,
            (&raw const one).cast(),
            int_len,
        );
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_NOSIGPIPE,
            (&raw const one).cast(),
            int_len,
        );
    }
    let r = if v6 {
        // SAFETY: plain data; all-zero is a valid sockaddr_in6 before the fields are set.
        let mut a: libc::sockaddr_in6 = unsafe { std::mem::zeroed() };
        a.sin6_len = std::mem::size_of::<libc::sockaddr_in6>() as u8;
        a.sin6_family = libc::AF_INET6 as libc::sa_family_t;
        a.sin6_port = port.to_be();
        a.sin6_addr.s6_addr[15] = 1;
        // SAFETY: a live socket; the option value is a c_int that outlives the call.
        unsafe {
            libc::setsockopt(
                fd,
                libc::IPPROTO_IPV6,
                libc::IPV6_V6ONLY,
                (&raw const one).cast(),
                int_len,
            );
        }
        // SAFETY: a live socket and an address of the stated length.
        unsafe {
            libc::bind(
                fd,
                (&raw const a).cast(),
                std::mem::size_of::<libc::sockaddr_in6>() as libc::socklen_t,
            )
        }
    } else {
        // SAFETY: plain data; all-zero is a valid sockaddr_in before the fields are set.
        let mut a: libc::sockaddr_in = unsafe { std::mem::zeroed() };
        a.sin_len = std::mem::size_of::<libc::sockaddr_in>() as u8;
        a.sin_family = libc::AF_INET as libc::sa_family_t;
        a.sin_port = port.to_be();
        a.sin_addr.s_addr = u32::from(std::net::Ipv4Addr::LOCALHOST).to_be();
        // SAFETY: a live socket and an address of the stated length.
        unsafe {
            libc::bind(
                fd,
                (&raw const a).cast(),
                std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
            )
        }
    };
    let err = (r != 0).then(|| std::io::Error::last_os_error().raw_os_error());
    // SAFETY: `fd` is this function's own socket, closed once.
    unsafe { libc::close(fd) };
    match err {
        None => Some(false),
        Some(Some(libc::EADDRINUSE)) => Some(true),
        Some(Some(libc::EADDRNOTAVAIL)) => Some(false),
        Some(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr, TcpListener};

    /// A process whose tree does not hold this test: System (4) on Windows; launchd (1) on macOS, root's,
    /// whose start cannot be read, so no parent link from it is ever trusted.
    const STRANGER: u32 = if cfg!(windows) { 4 } else { 1 };

    #[test]
    fn only_the_servers_own_pid_is_trusted() {
        assert!(only_owner(&[7], 7));
        assert!(only_owner(&[7, 7], 7));
        assert!(!only_owner(&[], 7));
        assert!(!only_owner(&[8], 7));
        assert!(!only_owner(&[7, 8], 7));
    }

    #[test]
    fn a_listener_in_this_process_names_this_process() {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let port = l.local_addr().expect("addr").port();
        assert_eq!(listener_pids(port), vec![std::process::id()]);
        drop(l);
        assert!(listener_pids(port).is_empty());
    }

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

    #[test]
    fn a_port_held_by_a_stranger_is_foreign_not_nobody() {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let port = l.local_addr().expect("addr").port();
        assert_eq!(tree_look(port, std::process::id()), TreeLook::InTree);
        assert_eq!(tree_look(port, STRANGER), TreeLook::Foreign);
        drop(l);
        assert_eq!(tree_look(port, std::process::id()), TreeLook::Nobody);
    }

    #[test]
    fn a_port_owned_by_another_process_is_not_in_the_tree() {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let port = l.local_addr().expect("addr").port();
        // This process owns it: it is in its own tree, not in an unrelated one.
        assert!(listening_in_tree(port, std::process::id()));
        assert!(!listening_in_tree(port, STRANGER));
        assert!(!listening_in_tree(port, 0));
        drop(l);
        assert!(!listening_in_tree(port, std::process::id()));
    }

    #[cfg(not(windows))]
    #[test]
    fn a_childs_listener_is_in_this_processs_tree_on_macos() {
        use std::process::{Command, Stdio};
        let probe = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let port = probe.local_addr().expect("addr").port();
        drop(probe);
        // sync-ok: test code on the test thread only
        let mut child = Command::new("/usr/bin/nc")
            .args(["-l", "127.0.0.1", &port.to_string()])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("nc");
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while listener_pids(port).is_empty() && std::time::Instant::now() < end {
            std::thread::sleep(std::time::Duration::from_millis(20)); // sync-ok: test code
        }
        let pids = listener_pids(port);
        let mine = tree_look(port, std::process::id());
        let theirs = tree_look(port, child.id());
        let stranger = tree_look(port, STRANGER);
        let _ = child.kill();
        let _ = child.wait(); // sync-ok: test code on the test thread only
        assert_eq!(pids, vec![child.id()]);
        assert_eq!(mine, TreeLook::InTree, "the child is below this process");
        assert_eq!(theirs, TreeLook::InTree, "the root itself holds it");
        assert_eq!(stranger, TreeLook::Foreign);
    }

    #[test]
    fn an_in_tree_answer_stands_only_when_the_unseen_family_is_shown_free() {
        use TreeLook::*;
        let free = |_v6: bool| Some(false);
        let held = |v6: bool| Some(v6);
        // The tree holds v4 only: v6 is probed, and a hidden v6 holder (another account) makes it unknown.
        assert_eq!(confirm_unseen_families(InTree, true, false, free), InTree);
        assert_eq!(
            confirm_unseen_families(InTree, true, false, held),
            Unreadable
        );
        assert_eq!(
            confirm_unseen_families(InTree, true, false, |_| None),
            Unreadable
        );
        // The tree holds v6 only: v4 is probed.
        assert_eq!(
            confirm_unseen_families(InTree, false, true, |v6| Some(!v6)),
            Unreadable
        );
        assert_eq!(confirm_unseen_families(InTree, false, true, free), InTree);
        // Both families held by the tree: nothing to probe.
        let mut asked = 0;
        assert_eq!(
            confirm_unseen_families(InTree, true, true, |_| {
                asked += 1;
                None
            }),
            InTree
        );
        assert_eq!(asked, 0);
        // Every other answer passes through untouched, and nothing is probed for it.
        for look in [Nobody, Foreign, Unreadable] {
            assert_eq!(confirm_unseen_families(look, true, false, |_| None), look);
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn the_loopback_probe_tells_a_held_address_from_a_free_one() {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let port = l.local_addr().expect("addr").port();
        assert_eq!(loopback_bind_refused(false, port), Some(true), "held");
        // The tree holds 127.0.0.1 only and nothing holds [::1]: the in-tree answer stands.
        assert_eq!(loopback_bind_refused(true, port), Some(false));
        assert_eq!(tree_look(port, std::process::id()), TreeLook::InTree);
        // The probe never listens: the port still has exactly one listener, this one.
        assert_eq!(listener_pids(port), vec![std::process::id()]);
        assert!(listener_pids_v6(port).is_empty());
        drop(l);
        assert_eq!(loopback_bind_refused(false, port), Some(false));
        // A wildcard listener of THIS user does not refuse the loopback probe (SO_REUSEADDR); another
        // account's would (the kernel's per-uid check), which no test can stage without a second user.
        let any = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)).expect("bind any");
        let p2 = any.local_addr().expect("addr").port();
        assert_eq!(loopback_bind_refused(false, p2), Some(false));
        drop(any);
        if let Ok(l6) = TcpListener::bind((Ipv6Addr::LOCALHOST, 0)) {
            let p6 = l6.local_addr().expect("addr").port();
            assert_eq!(loopback_bind_refused(true, p6), Some(true));
            assert_eq!(tree_look(p6, std::process::id()), TreeLook::InTree);
        }
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
