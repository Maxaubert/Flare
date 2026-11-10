//! The process table and process-group signals Flare needs off Windows: every process with its
//! parent, group, session, start time and controlling terminal; the working folder, executable and
//! argv of one process; signals to a process or a whole group; and a wait for a process to end.
//!
//! macOS only today (libproc, sysctl `KERN_PROCARGS2`, kqueue `NOTE_EXIT`, the TCP listeners through
//! `PROC_PIDFDSOCKETINFO`). On Windows the crate is
//! empty: the Win32 process code lives in `flare-term` and `flare-win` behind `cfg(windows)`
//! (regression rule 227). The policy (which processes a tab owns, which a closing tab ends) stays
//! with the callers; this crate only reads and signals.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;
#[cfg(target_os = "macos")]
mod sockets;
#[cfg(target_os = "macos")]
pub use sockets::*;

/// One TCP socket in the listening state: who holds it, the local port, and whether it is an IPv6
/// socket (a server on `[::1]` or `[::]`) or an IPv4 one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TcpListen {
    pub pid: u32,
    pub port: u16,
    pub v6: bool,
}

/// The parts of `struct socket_fdinfo` (`<sys/proc_info.h>`, 792 bytes on arm64 macOS 15 to 27)
/// read here: the byte offsets of `psi.soi_family`, `psi.soi_kind`, the TCP state
/// (`psi.soi_proto.pri_tcp.tcpsi_state`) and the local port
/// (`psi.soi_proto.pri_tcp.tcpsi_ini.insi_lport`). Measured with `offsetof` against the macOS 27.2
/// SDK (2026-11-08); a real listener is found by them in `sockets::tests`.
pub mod socket_fdinfo {
    pub const SIZE: usize = 792;
    pub const FAMILY: usize = 184;
    pub const KIND: usize = 256;
    pub const LOCAL_PORT: usize = 268;
    pub const TCP_STATE: usize = 344;
    /// `soi_kind` of a TCP socket.
    pub const SOCKINFO_TCP: i32 = 2;
    /// `tcpsi_state` of a listening socket.
    pub const TSI_S_LISTEN: i32 = 1;
    pub const AF_INET: i32 = 2;
    pub const AF_INET6: i32 = 30;
}

/// Pure: the listener a `socket_fdinfo` buffer describes, when it is a TCP socket in the listening
/// state on IPv4 or IPv6; None for anything else or a buffer too short. The port is stored in
/// network byte order in the low 16 bits of an `int` (`ntohs((u_short)insi_lport)`, as lsof reads it).
pub fn parse_tcp_listen(pid: u32, buf: &[u8]) -> Option<TcpListen> {
    use socket_fdinfo as f;
    if buf.len() < f::SIZE {
        return None;
    }
    let int_at = |at: usize| -> Option<i32> {
        Some(i32::from_ne_bytes(buf.get(at..at + 4)?.try_into().ok()?))
    };
    if int_at(f::KIND)? != f::SOCKINFO_TCP || int_at(f::TCP_STATE)? != f::TSI_S_LISTEN {
        return None;
    }
    let v6 = match int_at(f::FAMILY)? {
        f::AF_INET => false,
        f::AF_INET6 => true,
        _ => return None,
    };
    let raw = int_at(f::LOCAL_PORT)? as u32;
    let port = u16::from_be((raw & 0xffff) as u16);
    Some(TcpListen { pid, port, v6 })
}

/// Pure: the executable path and the argv in a `KERN_PROCARGS2` buffer. The layout is `argc` (a
/// native-endian `i32`), the executable path, NUL padding, then `argc` NUL-terminated strings (the
/// environment follows and is not read). None when the buffer is too short or malformed.
pub fn parse_procargs2(buf: &[u8]) -> Option<(String, Vec<String>)> {
    let argc = i32::from_ne_bytes(buf.get(..4)?.try_into().ok()?);
    let argc = usize::try_from(argc).ok()?;
    let rest = &buf[4..];
    let end = rest.iter().position(|&b| b == 0)?;
    let exe = String::from_utf8_lossy(&rest[..end]).into_owned();
    let mut at = end;
    while rest.get(at) == Some(&0) {
        at += 1;
    }
    let mut argv = Vec::with_capacity(argc.min(256));
    for _ in 0..argc {
        if at >= rest.len() {
            break;
        }
        let len = rest[at..]
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(rest.len() - at);
        argv.push(String::from_utf8_lossy(&rest[at..at + len]).into_owned());
        at += len + 1;
    }
    Some((exe, argv))
}

/// Pure: the bytes of a C string field up to its first NUL, as text (lossy).
pub fn c_field(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn procargs(argc: i32, exe: &str, pad: usize, args: &[&str], env: &[&str]) -> Vec<u8> {
        let mut b = argc.to_ne_bytes().to_vec();
        b.extend_from_slice(exe.as_bytes());
        b.extend(std::iter::repeat_n(0u8, pad));
        for a in args.iter().chain(env) {
            b.extend_from_slice(a.as_bytes());
            b.push(0);
        }
        b
    }

    #[test]
    fn procargs2_gives_the_executable_and_argv_but_never_the_environment() {
        let b = procargs(
            3,
            "/opt/homebrew/bin/node",
            5,
            &["node", "/usr/lib/cli.js", "--resume"],
            &["HOME=/Users/x", "PATH=/bin"],
        );
        let (exe, argv) = parse_procargs2(&b).unwrap();
        assert_eq!(exe, "/opt/homebrew/bin/node");
        assert_eq!(argv, ["node", "/usr/lib/cli.js", "--resume"]);
    }

    #[test]
    fn procargs2_keeps_an_empty_argument_and_survives_a_cut_buffer() {
        let b = procargs(3, "/bin/sh", 1, &["sh", "", "x"], &[]);
        assert_eq!(parse_procargs2(&b).unwrap().1, ["sh", "", "x"]);
        // argc says 4 but only two strings arrived.
        let b = procargs(4, "/bin/sh", 1, &["sh", "-c"], &[]);
        assert_eq!(parse_procargs2(&b).unwrap().1, ["sh", "-c"]);
        assert!(parse_procargs2(&[1, 0]).is_none());
        assert!(parse_procargs2(&(-1i32).to_ne_bytes()).is_none());
        assert!(parse_procargs2(&procargs(1, "/bin/sh", 0, &[], &[])[..8]).is_none());
    }

    /// A `socket_fdinfo` with the fields `parse_tcp_listen` reads set.
    fn fdinfo(kind: i32, state: i32, family: i32, port: u16) -> Vec<u8> {
        use socket_fdinfo as f;
        let mut b = vec![0u8; f::SIZE];
        let mut put = |at: usize, v: i32| b[at..at + 4].copy_from_slice(&v.to_ne_bytes());
        put(f::KIND, kind);
        put(f::TCP_STATE, state);
        put(f::FAMILY, family);
        put(f::LOCAL_PORT, i32::from(port.to_be()));
        b
    }

    #[test]
    fn a_listening_tcp_socket_is_read_with_its_port_and_family() {
        use socket_fdinfo as f;
        let l = parse_tcp_listen(
            7,
            &fdinfo(f::SOCKINFO_TCP, f::TSI_S_LISTEN, f::AF_INET, 8080),
        );
        assert_eq!(
            l,
            Some(TcpListen {
                pid: 7,
                port: 8080,
                v6: false
            })
        );
        let l6 = parse_tcp_listen(
            7,
            &fdinfo(f::SOCKINFO_TCP, f::TSI_S_LISTEN, f::AF_INET6, 443),
        );
        assert_eq!(l6.map(|l| (l.port, l.v6)), Some((443, true)));
        // Not a listener: an established socket, a UDP or Unix socket, another family, a short buffer.
        assert_eq!(
            parse_tcp_listen(7, &fdinfo(f::SOCKINFO_TCP, 4, f::AF_INET, 80)),
            None
        );
        assert_eq!(
            parse_tcp_listen(7, &fdinfo(1, f::TSI_S_LISTEN, f::AF_INET, 80)),
            None
        );
        assert_eq!(
            parse_tcp_listen(7, &fdinfo(f::SOCKINFO_TCP, f::TSI_S_LISTEN, 1, 80)),
            None
        );
        assert_eq!(
            parse_tcp_listen(
                7,
                &fdinfo(f::SOCKINFO_TCP, f::TSI_S_LISTEN, f::AF_INET, 80)[..100]
            ),
            None
        );
    }

    #[test]
    fn a_c_field_stops_at_the_first_nul() {
        assert_eq!(c_field(b"zsh\0\0garbage"), "zsh");
        assert_eq!(c_field(b"full"), "full");
        assert_eq!(c_field(b"\0"), "");
    }
}
