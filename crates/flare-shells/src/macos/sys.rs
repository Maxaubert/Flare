//! The user record and the host name, read through libc (the user record is Directory Services on
//! macOS, the same `UserShell` that `dscl . -read ~ UserShell` prints).

use std::ffi::CStr;

/// The user's login shell and home folder from the password database (`getpwuid_r`), each None
/// when unset or unreadable.
pub fn user_record() -> (Option<String>, Option<String>) {
    // SAFETY: getpwuid_r writes only into `pw` and `buf`, both owned here and sized as given; the
    // strings it points `pw` at live in `buf`, which outlives the reads below.
    unsafe {
        let mut pw: libc::passwd = std::mem::zeroed();
        let mut out: *mut libc::passwd = std::ptr::null_mut();
        let mut buf = vec![0 as libc::c_char; 16 * 1024];
        let rc = libc::getpwuid_r(
            libc::getuid(),
            &mut pw,
            buf.as_mut_ptr(),
            buf.len(),
            &mut out,
        );
        if rc != 0 || out.is_null() {
            return (None, None);
        }
        let text = |p: *const libc::c_char| {
            if p.is_null() {
                None
            } else {
                let s = CStr::from_ptr(p).to_string_lossy().into_owned();
                Some(s).filter(|s| !s.trim().is_empty())
            }
        };
        (text(pw.pw_shell), text(pw.pw_dir))
    }
}

/// This machine's host name (`gethostname`), None when it cannot be read.
pub fn host_name() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: gethostname writes at most `buf.len()` bytes into `buf`.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if rc != 0 {
        return None;
    }
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    std::str::from_utf8(&buf[..end])
        .ok()
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}

/// Where autofs maps are mounted right now (`getfsstat` with `MNT_NOWAIT`, so it never waits on a
/// server): each mount point whose file system type is `autofs`, as the kernel spells it. Empty
/// when the list cannot be read.
pub fn autofs_mounts() -> Vec<String> {
    // SAFETY: a null buffer asks only for the number of mounts.
    let n = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
    let Ok(n) = usize::try_from(n) else {
        return Vec::new();
    };
    // Room for a few mounts that appear between the two calls.
    let cap = n + 8;
    // SAFETY: `statfs` is plain old data; all zeroes is a valid value.
    let mut buf: Vec<libc::statfs> = vec![unsafe { std::mem::zeroed() }; cap];
    let bytes = libc::c_int::try_from(cap * std::mem::size_of::<libc::statfs>()).unwrap_or(0);
    // SAFETY: `buf` holds `cap` statfs records, `bytes` is its size in bytes.
    let got = unsafe { libc::getfsstat(buf.as_mut_ptr(), bytes, libc::MNT_NOWAIT) };
    let Ok(got) = usize::try_from(got) else {
        return Vec::new();
    };
    let field = |f: &[libc::c_char]| {
        // SAFETY: c_char and u8 have the same size and alignment.
        let b = unsafe { std::slice::from_raw_parts(f.as_ptr().cast::<u8>(), f.len()) };
        flare_proc::c_field(b)
    };
    buf[..got.min(cap)]
        .iter()
        .filter(|s| field(&s.f_fstypename) == "autofs")
        .map(|s| field(&s.f_mntonname))
        .filter(|m| m.starts_with('/'))
        .collect()
}

/// The user's home folder: `$HOME`, else the password database.
pub fn home_dir() -> Option<String> {
    std::env::var("HOME")
        .ok()
        .filter(|h| h.starts_with('/'))
        .or_else(|| user_record().1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_user_record_names_an_absolute_login_shell_and_home() {
        let (shell, home) = user_record();
        let shell = shell.expect("the user has a login shell");
        assert!(shell.starts_with('/'), "{shell}");
        assert!(home.expect("the user has a home").starts_with('/'));
    }

    #[test]
    fn the_autofs_mounts_are_what_mount_lists() {
        let out = std::process::Command::new("/sbin/mount")
            .output()
            .expect("mount runs");
        let text = String::from_utf8_lossy(&out.stdout);
        let mut listed: Vec<String> = text
            .lines()
            .filter(|l| l.contains("(autofs"))
            .filter_map(|l| {
                let (_, rest) = l.split_once(" on ")?;
                Some(rest.split(" (").next()?.to_string())
            })
            .collect();
        let mut ours = autofs_mounts();
        listed.sort();
        ours.sort();
        assert_eq!(ours, listed);
    }

    #[test]
    fn the_host_name_is_what_the_shells_see() {
        let name = host_name().expect("a host name");
        let out = std::process::Command::new("/bin/hostname")
            .output()
            .expect("hostname runs");
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), name);
    }
}
