//! The macOS (POSIX) path grammar (see `path_grammar`): one root `/`, `~` for home, `/` the only
//! separator (a `\` is a name character).
//!
//! The network rule of the Windows grammar has a Mac twin: `/net`, `/Network` and `/home` are
//! autofs mount points (so is any `autofs` mount, and each under `/System/Volumes/Data`), and a
//! stat under them mounts a server named by the text (`/net/evil-host/x`), so a path there is
//! never resolved. A mounted volume (`/Volumes/x`) is local to the stat meter: it
//! gets its own slow-volume key, so one hanging share does not hold the rest of the disk.

use crate::path_open::TEXT_MAX;

/// Collapse repeated `/`, resolve `.` and `..` (never above the root), keep a trailing `/`.
/// `p` starts with `/`.
fn normalise(p: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    let mut out = format!("/{}", parts.join("/"));
    if !parts.is_empty() && p.ends_with('/') {
        out.push('/');
    }
    out
}

/// True for a path under an automount point, where a stat reaches a server. The one rule every
/// macOS path check shares (`flare_shells::macos::paths::is_automount`).
fn is_automount(abs: &str) -> bool {
    flare_shells::macos::paths::is_automount(abs)
}

/// What `text` names from inside `cwd`, or None when it names nothing this checks: a path under
/// an automount point, a `//` path, `~user`, a NUL, a relative path with no folder to be relative
/// to, an over-long text.
pub fn resolve_term_path(cwd: &str, text: &str, home: &str) -> Option<String> {
    if text.is_empty() || text.chars().count() > TEXT_MAX || text.contains('\0') {
        return None;
    }
    let p = if text == "~" || text.starts_with("~/") {
        if !home.starts_with('/') {
            return None;
        }
        format!("{home}{}", &text[1..])
    } else if text.starts_with('~') {
        // `~user`: another user's home is never looked up for a link.
        return None;
    } else {
        text.to_string()
    };
    if p.starts_with("//") {
        return None;
    }
    let abs = if p.starts_with('/') {
        normalise(&p)
    } else {
        if !cwd.starts_with('/') || cwd.contains('\0') {
            return None;
        }
        normalise(&format!("{cwd}/{p}"))
    };
    (!is_automount(&abs)).then_some(abs)
}

/// The key the stat meter holds a slow volume under: `/volumes/<name>` for a mounted volume,
/// else `/` (the start-up disk), lower case.
pub fn volume_key(abs: &str) -> String {
    let mut it = abs.split('/').skip(1);
    match (it.next(), it.next()) {
        (Some(v), Some(name)) if v.eq_ignore_ascii_case("volumes") && !name.is_empty() => {
            format!("/volumes/{}", name.to_lowercase())
        }
        _ => "/".to_string(),
    }
}

/// True for a path read from inside the shell's folder (not rooted, no `~`).
pub fn is_relative(p: &str) -> bool {
    !p.starts_with(['/', '~'])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(cwd: &str, t: &str, home: &str) -> Option<String> {
        resolve_term_path(cwd, t, home)
    }

    #[test]
    fn resolve_against_the_shell_folder() {
        assert_eq!(
            r("/Users/me/work", "docs/a.pdf", "").as_deref(),
            Some("/Users/me/work/docs/a.pdf")
        );
        assert_eq!(
            r("/Users/me/work", "./a.txt", "").as_deref(),
            Some("/Users/me/work/a.txt")
        );
        assert_eq!(
            r("/Users/me/work", "../up.txt", "").as_deref(),
            Some("/Users/me/up.txt")
        );
        assert_eq!(
            r("/Users/me/work", "/etc/hosts", "").as_deref(),
            Some("/etc/hosts")
        );
        assert_eq!(
            r("/w", "~/n.txt", "/Users/me").as_deref(),
            Some("/Users/me/n.txt")
        );
        assert_eq!(r("/w", "~", "/Users/me").as_deref(), Some("/Users/me"));
        // A backslash is a name character, never a separator.
        assert_eq!(r("/w", "a\\b.txt", "").as_deref(), Some("/w/a\\b.txt"));
    }

    #[test]
    fn resolve_refuses_what_it_cannot_check() {
        assert_eq!(r("/w", "//server/share/a.txt", ""), None);
        assert_eq!(r("/w", "a\0.txt", ""), None);
        assert_eq!(r("", "docs/a.pdf", ""), None);
        assert_eq!(r("relative", "docs/a.pdf", ""), None);
        assert_eq!(r("/w", "", ""), None);
        assert_eq!(r("/w", "~/a.txt", ""), None, "no home");
        assert_eq!(r("/w", "~/a.txt", "relative"), None);
        assert_eq!(r("/w", "~bob/a.txt", "/Users/me"), None);
        assert_eq!(r("/w", &"a".repeat(TEXT_MAX + 1), ""), None);
    }

    #[test]
    fn an_automount_path_is_never_resolved() {
        for t in [
            "/net/evil-host/share/a.txt",
            "/NET/evil/a.txt",
            "/Network/Servers/evil/a.txt",
            "/tmp/../net/evil/a.txt",
            "/System/Volumes/Data/net/evil/a.txt",
            "/home/someone/a.txt",
        ] {
            assert_eq!(r("/w", t, ""), None, "{t}");
        }
        assert_eq!(r("/net", "x.txt", ""), None);
        assert_eq!(
            r("/w", "/netbsd/a.txt", "").as_deref(),
            Some("/netbsd/a.txt")
        );
    }

    #[test]
    fn normalise_keeps_a_trailing_slash_and_stops_at_the_root() {
        assert_eq!(r("/work", "a//b/./c/", "").as_deref(), Some("/work/a/b/c/"));
        assert_eq!(r("/w", "../../../x", "").as_deref(), Some("/x"));
        assert_eq!(r("/w", "/", "").as_deref(), Some("/"));
    }

    #[test]
    fn a_mounted_volume_has_its_own_slow_key() {
        assert_eq!(volume_key("/Volumes/Share/a.txt"), "/volumes/share");
        assert_eq!(volume_key("/Volumes/Share"), "/volumes/share");
        assert_eq!(volume_key("/Users/me/a.txt"), "/");
        assert_eq!(volume_key("/Volumes/"), "/");
    }

    #[test]
    fn relative_paths() {
        assert!(is_relative("docs/a.md"));
        assert!(is_relative("C:\\x"), "no drive letters on a Mac");
        assert!(!is_relative("/a"));
        assert!(!is_relative("~/a"));
    }
}
