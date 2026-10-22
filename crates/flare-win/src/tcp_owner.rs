//! Who owns a loopback port (audit L21, L27). The dictation engine picks a free port, starts the server on it and
//! waits for "GET /" to answer; any other program that grabbed the port in between would answer it
//! and then be handed the user's audio. The TCP table names the process behind every listener, so the
//! engine trusts the port only when every listener on it is its own server, and the Sonara post sends
//! its bearer token only to the process runtime.json names.

/// True when `owners` (the pids listening on the port) is not empty and every one is `pid`.
pub fn only_owner(owners: &[u32], pid: u32) -> bool {
    !owners.is_empty() && owners.iter().all(|o| *o == pid)
}

/// The listener table of one address family as the system gives it (`TCP_TABLE_OWNER_PID_LISTENER`): the bytes of the
/// table, or None when it cannot be read.
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

/// The pids of the processes listening on IPv4 `port`, from the TCP table. Empty when nobody is, or
/// when the table cannot be read (which then reads as "not trusted").
pub fn listener_pids(port: u16) -> Vec<u32> {
    use windows_sys::Win32::NetworkManagement::IpHelper::MIB_TCPROW_OWNER_PID;
    use windows_sys::Win32::Networking::WinSock::AF_INET;

    let Some(buf) = listener_table(AF_INET) else {
        return Vec::new();
    };
    let count = u32::from_le_bytes(buf[0..4].try_into().unwrap_or([0; 4])) as usize;
    let row_size = std::mem::size_of::<MIB_TCPROW_OWNER_PID>();
    let mut out = Vec::new();
    for i in 0..count {
        let at = 4 + i * row_size;
        if at + row_size > buf.len() {
            break;
        }
        // SAFETY: the row lies inside `buf`; the read is unaligned-safe.
        let row: MIB_TCPROW_OWNER_PID =
            unsafe { std::ptr::read_unaligned(buf.as_ptr().add(at).cast()) };
        // The port is in network byte order in the low 16 bits.
        if u16::from_be((row.dwLocalPort & 0xffff) as u16) == port {
            out.push(row.dwOwningPid);
        }
    }
    out
}

/// The pids of the processes listening on IPv6 `port` (a server on `[::1]` or `[::]`), from the TCP table.
pub fn listener_pids_v6(port: u16) -> Vec<u32> {
    use windows_sys::Win32::NetworkManagement::IpHelper::MIB_TCP6ROW_OWNER_PID;
    use windows_sys::Win32::Networking::WinSock::AF_INET6;

    let Some(buf) = listener_table(AF_INET6) else {
        return Vec::new();
    };
    let count = u32::from_le_bytes(buf[0..4].try_into().unwrap_or([0; 4])) as usize;
    let row_size = std::mem::size_of::<MIB_TCP6ROW_OWNER_PID>();
    // The rows start after the count; the first row is aligned to 4 in this table.
    let mut out = Vec::new();
    for i in 0..count {
        let at = 4 + i * row_size;
        if at + row_size > buf.len() {
            break;
        }
        // SAFETY: the row lies inside `buf`; the read is unaligned-safe.
        let row: MIB_TCP6ROW_OWNER_PID =
            unsafe { std::ptr::read_unaligned(buf.as_ptr().add(at).cast()) };
        if u16::from_be((row.dwLocalPort & 0xffff) as u16) == port {
            out.push(row.dwOwningPid);
        }
    }
    out
}

/// True when some process listens on `port` on either address family. Read from the system's table; nothing is ever
/// connected to, so the server being looked for sees no client. An unreadable table reads as "no".
pub fn is_listening(port: u16) -> bool {
    !listener_pids(port).is_empty() || !listener_pids_v6(port).is_empty()
}

/// The most process rows one snapshot is read for (a machine has thousands, never millions).
const SNAPSHOT_MAX: usize = 200_000;

/// `root` and every process below it, from (pid, parent pid) rows. Each pid is visited once, so a cycle (a reused
/// parent id) ends the walk, and the set never outgrows the rows.
pub fn descendants(rows: &[(u32, u32)], root: u32) -> std::collections::HashSet<u32> {
    let mut set = std::collections::HashSet::new();
    if root == 0 {
        return set;
    }
    set.insert(root);
    let mut grew = true;
    while grew {
        grew = false;
        for (pid, parent) in rows {
            if *pid != 0 && set.contains(parent) && set.insert(*pid) {
                grew = true;
            }
        }
    }
    set
}

/// (pid, parent pid) of every process, from one Toolhelp snapshot (bounded). Empty when it cannot be taken.
fn process_rows() -> Vec<(u32, u32)> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    // SAFETY: plain values in; an invalid handle is checked.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE || snap.is_null() {
        return Vec::new();
    }
    let mut rows = Vec::new();
    // SAFETY: PROCESSENTRY32W is plain data; zero is valid before dwSize is set.
    let mut e: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: `snap` is a live snapshot handle and `e` has its size set.
    let mut ok = unsafe { Process32FirstW(snap, &mut e) } != 0;
    while ok && rows.len() < SNAPSHOT_MAX {
        rows.push((e.th32ProcessID, e.th32ParentProcessID));
        // SAFETY: as above.
        ok = unsafe { Process32NextW(snap, &mut e) } != 0;
    }
    // SAFETY: `snap` is closed once.
    unsafe { CloseHandle(snap) };
    rows
}

/// True when something listens on `port` and EVERY listener on it (either address family) is `root` or one of its
/// descendants. A port held by any other process, or by a server outside the tab's process tree (WSL's
/// processes are not in it), reads as "no". Unreadable tables read as "no". Run it on a worker.
pub fn listening_in_tree(port: u16, root: u32) -> bool {
    if root == 0 {
        return false;
    }
    let mut owners = listener_pids(port);
    owners.extend(listener_pids_v6(port));
    if owners.is_empty() {
        return false;
    }
    let tree = descendants(&process_rows(), root);
    owners.iter().all(|o| tree.contains(o))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr, TcpListener};

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
        let rows = [(10, 1), (11, 10), (12, 11), (13, 10), (20, 2), (21, 20)];
        let set = descendants(&rows, 10);
        assert_eq!(set, [10, 11, 12, 13].into_iter().collect());
        assert!(descendants(&rows, 0).is_empty());
        // A cycle (a reused parent id) ends.
        let cyc = [(5, 6), (6, 5)];
        assert_eq!(descendants(&cyc, 5), [5, 6].into_iter().collect());
    }

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
