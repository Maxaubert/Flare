//! The macOS folder rules behind `cwd::is_remote` and `cwd::decide_follow` (rule 227: the
//! Windows rules stay in `cwd.rs`, these are their POSIX twins), and the one automount refusal
//! every macOS path check uses (`is_automount`: the tab's folder here, terminal links in
//! `flare-term`, panel images in `flare-surfaces`).

use crate::cwd::Follow;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// True for a folder a stat, a restore or a spawn would reach a server through: a `//` path, a
/// path under an automount point (`is_automount`), where looking mounts the server the path
/// names, and a path with a `.` or `..` segment (a restore file is anyone's to edit, and
/// `/tmp/../net/h` is under `/net`). The Mac twin of Windows' UNC rule; the Windows network forms
/// themselves (`crate::cwd::is_unc`: a `\\host\share` an OSC 9;9 report may still name) count too,
/// so no such text is ever adopted as a tab's folder. Any other text is local: a relative one
/// names nothing a restore can find, and a spawn in it fails like one in a folder that went away.
pub fn is_remote(p: &str) -> bool {
    if p.starts_with("//") || crate::cwd::is_unc(p) {
        return true;
    }
    if p.split('/').any(|s| s == "." || s == "..") {
        return true;
    }
    p.starts_with('/') && is_automount(p)
}

/// The autofs trigger points a Mac may have, by root name (any case: the system volume ignores
/// it): `/net` (the `-hosts` map), `/Network` (`/Network/Servers`) and `/home` (`auto_home`).
/// Refused whether or not their map is enabled now: an admin can enable one at any time.
const AUTOFS_ROOTS: [&str; 3] = ["net", "network", "home"];
/// Current macOS mounts the autofs maps on the Data volume, with a link at the root
/// (`/home -> /System/Volumes/Data/home`).
const DATA_VOLUME: &str = "/system/volumes/data";

/// True for an absolute path (already free of `.` and `..`) under an automount point, where a
/// stat mounts a server the text names: one of `AUTOFS_ROOTS`, at the root or under the Data
/// volume, or under any mount point of type `autofs` mounted now (also spelt without the Data
/// volume). The one refusal behind `is_remote`, the terminal's link grammar and the panels'
/// `is_local_path`.
pub fn is_automount(abs: &str) -> bool {
    is_automount_in(abs, &autofs_now())
}

/// Pure `is_automount`, with the autofs mount points given.
pub fn is_automount_in(abs: &str, mounts: &[String]) -> bool {
    let p = norm(abs).to_lowercase();
    let off_data = |s: &str| -> Option<String> {
        let rest = s.strip_prefix(DATA_VOLUME)?;
        (rest.is_empty() || rest.starts_with('/')).then(|| {
            if rest.is_empty() {
                "/".to_string()
            } else {
                rest.to_string()
            }
        })
    };
    let rootless = off_data(&p).unwrap_or_else(|| p.clone());
    let first = rootless.split('/').nth(1).unwrap_or("");
    if rootless.starts_with('/') && AUTOFS_ROOTS.contains(&first) {
        return true;
    }
    let under = |path: &str, m: &str| path == m || path.starts_with(&format!("{m}/"));
    mounts.iter().any(|m| {
        let m = norm(m).to_lowercase();
        if m == "/" || !m.starts_with('/') {
            return false;
        }
        let m_rootless = off_data(&m).unwrap_or_else(|| m.clone());
        under(&p, &m) || (m_rootless != "/" && under(&rootless, &m_rootless))
    })
}

/// The autofs mount points, read again at most every few seconds (a path check runs per link
/// candidate on every repaint).
fn autofs_now() -> Arc<Vec<String>> {
    const FRESH: Duration = Duration::from_secs(5);
    static CACHE: Mutex<Option<(Instant, Arc<Vec<String>>)>> = Mutex::new(None);
    let mut c = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    match &*c {
        Some((at, m)) if at.elapsed() < FRESH => Arc::clone(m),
        _ => {
            let m = Arc::new(crate::macos::sys::autofs_mounts());
            *c = Some((Instant::now(), Arc::clone(&m)));
            m
        }
    }
}

/// Every run of `/` as one, a trailing `/` off (the root stays `/`). Case is kept: a volume may
/// be case-sensitive.
fn norm(p: &str) -> String {
    let mut out = String::with_capacity(p.len());
    for ch in p.chars() {
        if ch == '/' && out.ends_with('/') {
            continue;
        }
        out.push(ch);
    }
    if out.len() > 1 && out.ends_with('/') {
        out.pop();
    }
    out
}

/// `decide_follow` for POSIX folders: either empty -> Outside; equal -> Same; `cwd` under
/// `root` -> Inside; else Outside.
pub fn decide_follow(root: &str, cwd: &str) -> Follow {
    let r = norm(root);
    let c = norm(cwd);
    if r.is_empty() || c.is_empty() {
        Follow::Outside
    } else if r == c {
        Follow::Same
    } else if r == "/" || c.starts_with(&format!("{r}/")) {
        Follow::Inside
    } else {
        Follow::Outside
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remote_or_odd_folder_is_remote() {
        for p in [
            "//evil/share",
            "/net/evil/share",
            "/NET/evil",
            "/Network/Servers/evil",
            "/tmp/../net/evil",
            "/System/Volumes/Data/net/evil",
            "/home/someone",
            "/Users/./me",
            "../up",
            "\\\\evil\\share",
            "\\/evil/share",
            "\\\\?\\UNC\\evil\\share",
        ] {
            assert!(is_remote(p), "{p:?}");
        }
        for p in [
            "/",
            "/Users/me",
            "/Volumes/USB/x",
            "/netbsd",
            "/Users/me/a b",
            "net/x",
            "",
        ] {
            assert!(!is_remote(p), "{p:?}");
        }
    }

    #[test]
    fn every_automount_point_is_refused_in_every_spelling() {
        let none: Vec<String> = Vec::new();
        for p in [
            "/net",
            "/net/host/x",
            "/NET/host",
            "/Network/Servers/h",
            "/home/me",
            "/HOME",
            "/System/Volumes/Data/net/host",
            "/system/volumes/data/Network/x",
            "/System/Volumes/Data/home/me",
            "//net/host",
        ] {
            assert!(is_automount_in(p, &none), "{p:?}");
        }
        for p in [
            "/",
            "/Users/me",
            "/netbsd",
            "/homes/x",
            "/System/Volumes/Data",
            "/System/Volumes/Data/Users/me",
            "/Volumes/USB",
        ] {
            assert!(!is_automount_in(p, &none), "{p:?}");
        }
        // A map an admin mounted elsewhere, found by its file system type, in both spellings.
        let mounts = vec!["/System/Volumes/Data/corp".to_string(), "/".to_string()];
        for p in ["/corp/share", "/System/Volumes/Data/corp", "/CORP"] {
            assert!(is_automount_in(p, &mounts), "{p:?}");
        }
        assert!(!is_automount_in("/corpus", &mounts));
        assert!(!is_automount_in("/Users/me", &mounts));
    }

    #[test]
    fn the_live_autofs_mounts_are_refused() {
        for m in crate::macos::sys::autofs_mounts() {
            assert!(is_automount(&format!("{m}/x")), "{m}");
            assert!(is_remote(&m), "{m}");
        }
        assert!(!is_automount("/Users"));
    }

    #[test]
    fn decide_follow_tells_same_inside_outside() {
        let root = "/Users/me/work/Atlas";
        assert_eq!(decide_follow(root, "/Users/me/work/Atlas"), Follow::Same);
        assert_eq!(decide_follow(root, "/Users/me/work/Atlas/"), Follow::Same);
        assert_eq!(decide_follow(root, "/Users/me//work/Atlas"), Follow::Same);
        assert_eq!(
            decide_follow(root, "/Users/me/work/Atlas/src/main"),
            Follow::Inside
        );
        assert_eq!(
            decide_follow(root, "/Users/me/work/Atlas2"),
            Follow::Outside
        );
        assert_eq!(decide_follow(root, "/Users/me/work"), Follow::Outside);
        assert_eq!(decide_follow(root, "/users/me/work/atlas"), Follow::Outside);
        assert_eq!(decide_follow("/", "/tmp"), Follow::Inside);
        assert_eq!(decide_follow("/", "/"), Follow::Same);
        assert_eq!(decide_follow("", "/tmp"), Follow::Outside);
    }
}
