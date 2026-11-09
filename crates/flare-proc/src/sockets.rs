//! macOS: the TCP listeners of the user's processes, read through libproc (`PROC_PIDLISTFDS`, then
//! `PROC_PIDFDSOCKETINFO` for each socket), never by parsing `lsof` and never by connecting.
//!
//! The kernel answers these calls only for the processes of this user, so a listener held by another
//! account's process (a system daemon, another login) is NOT seen: a port only such a process holds
//! reads as not listening, and one it shares through `SO_REUSEPORT` lists only this user's holders.
//! Windows' table names every process; this is the known gap of the Mac table (regression rule 244).

use crate::{TcpListen, parse_tcp_listen, socket_fdinfo};

/// `PROC_PIDFDSOCKETINFO` (`<sys/proc_info.h>`): the flavor of `proc_pidfdinfo` that fills a
/// `struct socket_fdinfo`. The libc crate has the call but not this flavor.
const PROC_PIDFDSOCKETINFO: libc::c_int = 3;

/// The open file descriptors of `pid` as (fd, type); None when the list cannot be read (another
/// user's process, or gone). A full buffer may have been cut short, so it is grown and the list asked
/// for again (a bounded number of times): a listener past the cut would otherwise be missed.
fn fds(pid: libc::pid_t) -> Option<Vec<(i32, u32)>> {
    let entry = std::mem::size_of::<libc::proc_fdinfo>();
    // SAFETY: a null buffer asks only for the size the list needs, in bytes.
    let need =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
    let need = usize::try_from(need).ok().filter(|n| *n > 0)?;
    // Room for descriptors opened between the two calls.
    let mut count = need / entry + 32;
    for _ in 0..FDS_TRIES {
        let (list, full) = fds_into(pid, count)?;
        if !full {
            return Some(list);
        }
        count *= 2;
    }
    // Still full after the last growth: the list may be cut, so it is not trusted.
    None
}

/// How many times [`fds`] grows its buffer before it gives up on a list that keeps filling it.
const FDS_TRIES: usize = 5;

/// One `PROC_PIDLISTFDS` call into room for `count` entries: the entries, and whether they filled the
/// buffer (so the list may have been cut).
fn fds_into(pid: libc::pid_t, count: usize) -> Option<(Vec<(i32, u32)>, bool)> {
    let entry = std::mem::size_of::<libc::proc_fdinfo>();
    // SAFETY: plain data; all-zero is a valid value before the kernel fills it.
    let mut buf: Vec<libc::proc_fdinfo> = vec![unsafe { std::mem::zeroed() }; count];
    let bytes = libc::c_int::try_from(count * entry).ok()?;
    // SAFETY: `buf` holds `bytes` bytes of `proc_fdinfo`; the call writes at most that many and
    // returns the bytes written.
    let got = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDLISTFDS,
            0,
            buf.as_mut_ptr().cast(),
            bytes,
        )
    };
    let got = usize::try_from(got).ok().filter(|g| *g > 0)?;
    let full = list_was_cut(got, count * entry);
    buf.truncate((got / entry).min(count));
    Some((
        buf.into_iter()
            .map(|f| (f.proc_fd, f.proc_fdtype))
            .collect(),
        full,
    ))
}

/// A list call that wrote as many bytes as it was given room for may have had more to say.
fn list_was_cut(got: usize, room: usize) -> bool {
    got >= room
}

/// The listener behind descriptor `fd` of `pid`, when it is a listening TCP socket.
fn listener(pid: libc::pid_t, fd: i32) -> Option<TcpListen> {
    // 8-byte aligned, as `struct socket_fdinfo` is; a little more than it needs.
    let mut buf = vec![0u64; socket_fdinfo::SIZE / 8 + 16];
    let bytes = libc::c_int::try_from(buf.len() * 8).ok()?;
    // SAFETY: `buf` holds `bytes` writable bytes, more than `struct socket_fdinfo`; the kernel copies
    // that struct and returns how many bytes it wrote.
    let got = unsafe {
        libc::proc_pidfdinfo(
            pid,
            fd,
            PROC_PIDFDSOCKETINFO,
            buf.as_mut_ptr().cast(),
            bytes,
        )
    };
    let got = usize::try_from(got).ok()?;
    // SAFETY: the u64 buffer read as its bytes, no further than it is long.
    let raw = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), buf.len() * 8) };
    parse_tcp_listen(u32::try_from(pid).ok()?, &raw[..got.min(raw.len())])
}

/// Every listening TCP socket of the processes whose descriptors can be read (this user's), as
/// (pid, port, family). None when the process list itself cannot be read.
pub fn tcp_listeners() -> Option<Vec<TcpListen>> {
    let pids = crate::all_pids();
    if pids.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for pid in pids {
        let Ok(p) = libc::pid_t::try_from(pid) else {
            continue;
        };
        let Some(list) = fds(p) else {
            continue;
        };
        for (fd, kind) in list {
            if kind == libc::PROX_FDTYPE_SOCKET as u32
                && let Some(l) = listener(p, fd)
            {
                out.push(l);
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr, TcpListener, TcpStream};

    fn ours(port: u16, v6: bool) -> Vec<u32> {
        tcp_listeners()
            .expect("the table")
            .into_iter()
            .filter(|l| l.port == port && l.v6 == v6)
            .map(|l| l.pid)
            .collect()
    }

    #[test]
    fn a_listener_of_this_process_is_found_with_its_port_and_family() {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let port = l.local_addr().expect("addr").port();
        assert_eq!(ours(port, false), vec![std::process::id()]);
        assert!(ours(port, true).is_empty());
        drop(l);
        assert!(ours(port, false).is_empty());
        let l6 = TcpListener::bind((Ipv6Addr::LOCALHOST, 0)).expect("bind v6");
        let port6 = l6.local_addr().expect("addr").port();
        assert_eq!(ours(port6, true), vec![std::process::id()]);
    }

    #[test]
    fn a_connected_socket_is_no_listener() {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let port = l.local_addr().expect("addr").port();
        let c = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).expect("connect");
        let (s, _) = l.accept().expect("accept");
        let local = c.local_addr().expect("local").port();
        // The client's own port and the accepted socket are connections, not listeners.
        assert!(ours(local, false).is_empty());
        assert_eq!(ours(port, false), vec![std::process::id()]);
        drop((c, s));
    }

    #[test]
    fn another_process_of_this_user_is_seen_with_its_pid() {
        use std::process::{Command, Stdio};
        // A port that was free a moment ago, then a child (`nc -l`) listening on it.
        let probe = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let port = probe.local_addr().expect("addr").port();
        drop(probe);
        let mut child = Command::new("/usr/bin/nc")
            .args(["-l", "127.0.0.1", &port.to_string()])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("nc");
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut seen = Vec::new();
        while std::time::Instant::now() < end {
            seen = ours(port, false);
            if !seen.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(seen, vec![child.id()]);
    }

    #[test]
    fn a_full_descriptor_list_is_taken_as_cut_and_asked_for_again() {
        assert!(list_was_cut(64, 64));
        assert!(list_was_cut(80, 64));
        assert!(!list_was_cut(56, 64));
        // Room for one entry only: the first answer fills it, so the list is grown until it fits, and
        // every descriptor of this process (stdin among them) comes back.
        let me = libc::pid_t::try_from(std::process::id()).unwrap();
        let (one, full) = fds_into(me, 1).expect("one entry");
        assert_eq!(one.len(), 1);
        assert!(full, "a process has more than one descriptor");
        let all = fds(me).expect("the whole list");
        assert!(all.len() > 1);
        assert!(all.iter().any(|(fd, _)| *fd == 0));
    }

    #[test]
    fn launchds_descriptors_cannot_be_read_and_that_is_no_failure() {
        // pid 1 is root's: its list is refused, and the table is still read.
        assert!(fds(1).is_none());
        assert!(tcp_listeners().is_some());
    }
}
