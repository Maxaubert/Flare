//! The macOS (POSIX) path grammar (see `path_grammar`): one root `/`, `~` for home, `/` the only
//! separator (a `\` is a name character), names compared with their case (APFS is usually
//! case-insensitive, but a case-sensitive volume is allowed, so nothing here assumes either).
//!
//! The network rule of the Windows grammar has a Mac twin: `/net`, `/Network` and `/home` are
//! autofs mount points (so is any `autofs` mount, and each under `/System/Volumes/Data`), and a
//! stat under them mounts a server named by the text (`/net/evil-host/x`), so a path there is
//! never resolved. A mounted volume (`/Volumes/x`) is local to the stat meter: it
//! gets its own slow-volume key, so one hanging share does not hold the rest of the disk.

use super::Special;
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

fn trim_slash(p: &str) -> &str {
    let t = p.trim_end_matches('/');
    if t.is_empty() && p.starts_with('/') {
        "/"
    } else {
        t
    }
}

/// Whether `dir` is `cwd` itself (a trailing `/` aside). Case counts: on a case-sensitive volume
/// `/a` and `/A` are two folders, and a folder wrongly kept as a link is harmless.
pub fn is_cwd(dir: &str, cwd: &str) -> bool {
    trim_slash(dir) == trim_slash(cwd)
}

/// The folder above `dir`, or None at (or right under) the root: like a drive's root on Windows,
/// `/` itself is never a folder a relative path is tried in.
pub fn parent_dir(dir: &str) -> Option<String> {
    let d = dir.trim_end_matches('/');
    let i = d.rfind('/')?;
    let up = &d[..i];
    (!up.is_empty()).then(|| up.to_string())
}

/// The paths in terminal text that the word pass cannot read: a `file://` URL on this machine
/// (percent-decoded, `file:///Users/me/a%20b.txt:12` names `/Users/me/a b.txt`), and a word
/// whose spaces are escaped with `\` (`My\ Files/a.txt`, what zsh, bash and `ls -b` print).
/// The masked span is the whole word, so its pieces are never read on their own.
pub fn special_candidates(chars: &[char]) -> Vec<Special> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let opens = i == 0 || chars[i - 1].is_whitespace() || "([{=,:".contains(chars[i - 1]);
        if !opens || is_break(chars[i]) {
            i += 1;
            continue;
        }
        if let Some((s, next)) = file_url_at(chars, i).or_else(|| escaped_at(chars, i)) {
            out.push(s);
            i = next;
            continue;
        }
        i += 1;
    }
    out
}

fn is_break(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '<' | '>' | '|')
}

/// A `file://` URL starting at `at`, and where its word ends.
fn file_url_at(chars: &[char], at: usize) -> Option<(Special, usize)> {
    let head: String = chars.get(at..at + 7)?.iter().collect();
    if !head.eq_ignore_ascii_case("file://") {
        return None;
    }
    let mut end = at;
    while end < chars.len() && !is_break(chars[end]) {
        end += 1;
    }
    let word_end = end;
    // The sentence's punctuation and a `:line:col`, off the end, as for a plain path.
    let word: String = chars[at..end].iter().collect();
    let mut url = word.as_str();
    for _ in 0..3 {
        let before = url.len();
        url = url.trim_end_matches(['.', ',', ';', '!', ')', ']', '}']);
        url = strip_line_no(url);
        url = url.trim_end_matches(':');
        if url.len() == before {
            break;
        }
    }
    let path = flare_shells::macos::osc7::parse_osc7(url)?;
    if path == "/" {
        return None;
    }
    end = at + url.chars().count();
    Some((
        Special {
            start: at,
            end,
            path,
            mask: (at, word_end),
        },
        word_end,
    ))
}

/// `:12` or `:12:5` off the end.
fn strip_line_no(s: &str) -> &str {
    let mut t = s;
    for _ in 0..2 {
        let Some(i) = t.rfind(':') else { break };
        let digits = &t[i + 1..];
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            break;
        }
        t = &t[..i];
    }
    t
}

/// A word starting at `at` that holds at least one `\ ` (an escaped space), unescaped, and
/// where the word ends. Other escapes in it (`\(`, `\'`) are read too.
fn escaped_at(chars: &[char], at: usize) -> Option<(Special, usize)> {
    let mut raw_of: Vec<usize> = Vec::new();
    let mut text = String::new();
    let mut spaced = false;
    let mut j = at;
    while j < chars.len() {
        let c = chars[j];
        if c == '\\' && j + 1 < chars.len() && chars[j + 1] != '\n' {
            spaced |= chars[j + 1] == ' ';
            raw_of.push(j);
            text.push(chars[j + 1]);
            j += 2;
            continue;
        }
        if is_break(c) {
            break;
        }
        raw_of.push(j);
        text.push(c);
        j += 1;
    }
    if !spaced {
        return None;
    }
    let (from, path) = crate::paths::candidate(&text)?;
    let n = path.chars().count();
    let start = *raw_of.get(from)?;
    let last = *raw_of.get(from + n - 1)?;
    // The raw end: past the last char, and past its escape's second char.
    let end = if chars[last] == '\\' {
        last + 2
    } else {
        last + 1
    };
    Some((
        Special {
            start,
            end,
            path,
            mask: (at, j),
        },
        j,
    ))
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
    fn relative_cwd_and_parent() {
        assert!(is_relative("docs/a.md"));
        assert!(is_relative("C:\\x"), "no drive letters on a Mac");
        assert!(!is_relative("/a"));
        assert!(!is_relative("~/a"));
        assert!(is_cwd("/Users/me/", "/Users/me"));
        assert!(is_cwd("/", "/"));
        assert!(!is_cwd("/Users/Me", "/Users/me"));
        assert_eq!(parent_dir("/Users/me/a").as_deref(), Some("/Users/me"));
        assert_eq!(parent_dir("/Users/me/").as_deref(), Some("/Users"));
        assert_eq!(parent_dir("/Users"), None);
        assert_eq!(parent_dir("/"), None);
    }

    fn specials(text: &str) -> Vec<(String, String)> {
        let chars: Vec<char> = text.chars().collect();
        special_candidates(&chars)
            .into_iter()
            .map(|s| (chars[s.start..s.end].iter().collect(), s.path))
            .collect()
    }

    #[test]
    fn a_file_url_names_its_decoded_path() {
        assert_eq!(
            specials("see file:///Users/me/a%20b.txt:12:3."),
            vec![(
                "file:///Users/me/a%20b.txt".to_string(),
                "/Users/me/a b.txt".to_string()
            )]
        );
        assert_eq!(
            specials("(file://localhost/tmp/x.log)"),
            vec![(
                "file://localhost/tmp/x.log".to_string(),
                "/tmp/x.log".to_string()
            )]
        );
        // Another machine's file, a broken escape and the root name nothing here.
        assert!(specials("file://far-away/tmp/x.log").is_empty());
        assert!(specials("file:///tmp/%ZZ.txt").is_empty());
        assert!(specials("file:///").is_empty());
        assert!(specials("xfile:///tmp/a.txt").is_empty());
    }

    #[test]
    fn an_escaped_space_keeps_the_word_whole() {
        assert_eq!(
            specials("cd My\\ Files/notes\\ 1.md:4 now"),
            vec![(
                "My\\ Files/notes\\ 1.md".to_string(),
                "My Files/notes 1.md".to_string()
            )]
        );
        assert_eq!(
            specials("ls /Users/me/a\\ \\(b\\).txt"),
            vec![(
                "/Users/me/a\\ \\(b\\).txt".to_string(),
                "/Users/me/a (b).txt".to_string()
            )]
        );
        // A backslash with no escaped space is no escape run (the word pass reads it).
        assert!(specials("C:\\Users\\me\\a.txt").is_empty());
        assert!(specials("plain/a.txt").is_empty());
    }
}
