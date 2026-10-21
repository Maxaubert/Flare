//! Who owns a loopback port (audit L21, L27). The dictation engine picks a free port, starts the server on it and
//! waits for "GET /" to answer; any other program that grabbed the port in between would answer it
//! and then be handed the user's audio. The TCP table names the process behind every listener, so the
//! engine trusts the port only when every listener on it is its own server, and the Sonara post sends
//! its bearer token only to the process runtime.json names.

/// True when `owners` (the pids listening on the port) is not empty and every one is `pid`.
pub fn only_owner(owners: &[u32], pid: u32) -> bool {
    !owners.is_empty() && owners.iter().all(|o| *o == pid)
}

/// The pids of the processes listening on IPv4 `port`, from the TCP table. Empty when nobody is, or
/// when the table cannot be read (which then reads as "not trusted").
pub fn listener_pids(port: u16) -> Vec<u32> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
    };
    use windows_sys::Win32::Networking::WinSock::AF_INET;

    let mut size: u32 = 0;
    // SAFETY: a null table with size 0 only asks for the size needed.
    unsafe {
        GetExtendedTcpTable(
            std::ptr::null_mut(),
            &mut size,
            0,
            u32::from(AF_INET),
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        );
    }
    // The table can grow between the two calls; a few tries.
    for _ in 0..4 {
        if size == 0 {
            return Vec::new();
        }
        let mut buf = vec![0u8; size as usize + 512];
        let mut len = buf.len() as u32;
        // SAFETY: `buf` is `len` bytes, writable, and the call writes at most `len` bytes.
        let rc = unsafe {
            GetExtendedTcpTable(
                buf.as_mut_ptr().cast(),
                &mut len,
                0,
                u32::from(AF_INET),
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };
        if rc == 0 {
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
            return out;
        }
        size = len;
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, TcpListener};

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
}
