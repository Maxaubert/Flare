//! The Windows path grammar (see `path_grammar`): drive letters, `\` and `/` both separators, a
//! network path never resolved, folders compared without case. Moved here unchanged from
//! `path_open` and `path_links` when the macOS grammar was added.

use super::Special;
use crate::path_open::{TEXT_MAX, has_drive_root, is_sep};

/// '/' -> '\', collapse repeated separators, resolve "." and ".." (never above the drive root),
/// keep a trailing separator, keep the drive letter's case. `p` starts with `X:`.
fn normalise_win(p: &str) -> String {
    let drive: String = p.chars().take(2).collect();
    let rest: String = p.chars().skip(2).collect();
    let mut parts: Vec<&str> = Vec::new();
    for seg in rest.split(is_sep) {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    let mut out = format!("{drive}\\{}", parts.join("\\"));
    if !parts.is_empty() && rest.ends_with(is_sep) {
        out.push('\\');
    }
    out
}

/// What `text` names from inside `cwd`, or None when it names nothing this checks: a network path,
/// a NUL, a relative path with no folder to be relative to, an over-long text.
pub fn resolve_term_path(cwd: &str, text: &str, home: &str) -> Option<String> {
    if text.is_empty() || text.chars().count() > TEXT_MAX || text.contains('\0') {
        return None;
    }
    let mut p = text.to_string();
    if p == "~" || p.starts_with("~/") || p.starts_with("~\\") {
        p = format!("{home}{}", &p[1..]);
    }
    let mut it = p.chars();
    if it.next().is_some_and(is_sep) && it.next().is_some_and(is_sep) {
        return None;
    }
    if has_drive_root(&p) {
        return Some(normalise_win(&p));
    }
    if p.starts_with(is_sep) {
        if !has_drive_root(cwd) {
            return None;
        }
        let drive: String = cwd.chars().take(2).collect();
        return Some(normalise_win(&format!("{drive}{p}")));
    }
    if !has_drive_root(cwd) || cwd.contains('\0') {
        return None;
    }
    Some(normalise_win(&format!("{cwd}\\{p}")))
}

/// The key the stat meter holds a slow volume under: the drive (`c:`), lower case.
pub fn volume_key(abs: &str) -> String {
    abs.chars().take(2).collect::<String>().to_lowercase()
}

/// True for a path read from inside the shell's folder (not rooted, no drive, no `~`).
pub fn is_relative(p: &str) -> bool {
    let mut it = p.chars();
    match it.next() {
        None => true,
        Some('\\' | '/' | '~') => false,
        Some(c) if c.is_ascii_alphabetic() => it.next() != Some(':'),
        Some(_) => true,
    }
}

/// Whether `dir` is `cwd` itself. Such a folder found from a shown folder is the one the shell is
/// already in said another way (a status line prints it on every redraw). A folder above `cwd` is
/// a real place the text names (`Documents/Claude` from inside `...\Claude\Github\Flare`), so it links.
pub fn is_cwd(dir: &str, cwd: &str) -> bool {
    let norm = |p: &str| p.replace('/', "\\").trim_end_matches('\\').to_lowercase();
    norm(dir) == norm(cwd)
}

/// The folder above `dir`, or None at a drive's root.
pub fn parent_dir(dir: &str) -> Option<String> {
    let d = dir.trim_end_matches(['\\', '/']);
    let i = d.rfind(['\\', '/'])?;
    let up = &d[..i];
    let b = up.as_bytes();
    if b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        None
    } else {
        Some(up.to_string())
    }
}

/// Windows reads no path out of terminal text before the word pass.
pub fn special_candidates(_chars: &[char]) -> Vec<Special> {
    Vec::new()
}
