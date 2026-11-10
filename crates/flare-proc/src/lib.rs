//! The process table and process-group signals Flare needs off Windows: every process with its
//! parent, group, session, start time and controlling terminal; the working folder, executable and
//! argv of one process; signals to a process or a whole group; and a wait for a process to end.
//!
//! macOS only today (libproc, sysctl `KERN_PROCARGS2`, kqueue `NOTE_EXIT`). On Windows the crate is
//! empty: the Win32 process code lives in `flare-term` and `flare-win` behind `cfg(windows)`
//! (regression rule 227). The policy (which processes a tab owns, which a closing tab ends) stays
//! with the callers; this crate only reads and signals.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

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

    #[test]
    fn a_c_field_stops_at_the_first_nul() {
        assert_eq!(c_field(b"zsh\0\0garbage"), "zsh");
        assert_eq!(c_field(b"full"), "full");
        assert_eq!(c_field(b"\0"), "");
    }
}
