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
