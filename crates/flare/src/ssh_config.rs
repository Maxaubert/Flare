//! The hosts `~/.ssh/config` names, for Settings > Machines (remote sessions PR 7): the SSH hosts of the Add a machine dialog's drop-down.
//!
//! Only `Host` lines are listed, and only their plain names: a pattern (`*`, `?`, `!`) is no host one can pick, and the
//! lines under a `Match` block belong to no host. Each host's `HostName` is read too (the first one met in a block that
//! takes it, as ssh does), so the drop-down can leave out a host that is a computer already listed nearby (rule 1276). `Include` is followed (relative names under `~/.ssh`, `~/` for the
//! home folder, `*` and `?` in the last part), at most [`INCLUDE_DEPTH`] deep, as ssh itself does. Keywords are matched
//! without case, a value may be in double quotes, `Host=name` is the same as `Host name`, a CRLF ending is a line
//! ending. A name that would not be one plain argv item for ssh (`machines::argv_word`) is left out, so nothing read
//! here can become an option of ssh (rule 305).
//!
//! The reads are a worker's (rule 34): `hosts_of_home` is BLOCKING.

use std::path::{Path, PathBuf};

pub use flare_settings::page::machines::SshHost;

/// How deep `Include` is followed (ssh's own limit is 16).
pub const INCLUDE_DEPTH: usize = 16;
/// The most hosts listed.
pub const HOSTS_MAX: usize = 200;

/// A line's keyword (lower case) and its arguments, quotes taken off. None for a blank line or a comment.
fn split_line(line: &str) -> Option<(String, Vec<String>)> {
    let line = line.trim_end_matches('\r').trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    // `Keyword value`, `Keyword=value` and `Keyword = value` are all one form.
    let cut = line.find(|c: char| c.is_whitespace() || c == '=')?;
    let key = line[..cut].to_ascii_lowercase();
    let rest = line[cut..].trim_start();
    let rest = rest.strip_prefix('=').unwrap_or(rest).trim_start();
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in rest.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    args.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        args.push(cur);
    }
    Some((key, args))
}

/// Pure: the hosts one file names, and the `Include` arguments in the order met (for the caller to follow).
fn read_text(text: &str) -> Vec<Item> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some((key, args)) = split_line(line) else {
            continue;
        };
        match key.as_str() {
            "host" => out.push(Item::Host(args)),
            "include" => {
                for a in args {
                    out.push(Item::Include(a));
                }
            }
            "match" => out.push(Item::Match),
            "hostname" => {
                if let Some(a) = args.into_iter().next() {
                    out.push(Item::HostName(a));
                }
            }
            _ => {}
        }
    }
    out
}

enum Item {
    /// A `Host` line's patterns: a new block.
    Host(Vec<String>),
    Include(String),
    /// A `Match` line: the lines under it belong to no `Host` block.
    Match,
    HostName(String),
}

/// A name one can pick: no pattern, no negation, one plain argv word.
fn pickable(name: &str) -> bool {
    !name.contains(['*', '?', '!']) && crate::machines::argv_word(name)
}

/// Where an `Include` argument points: `~/` is the home folder, a relative name is under `~/.ssh`.
fn include_path(arg: &str, home: &Path) -> PathBuf {
    if let Some(rest) = arg.strip_prefix("~/") {
        return home.join(rest);
    }
    let p = PathBuf::from(arg);
    if p.is_absolute() {
        p
    } else {
        home.join(".ssh").join(p)
    }
}

/// Whether `name` fits a glob of `*` and `?`.
fn glob_fits(glob: &str, name: &str) -> bool {
    fn go(g: &[char], n: &[char]) -> bool {
        match (g.first(), n.first()) {
            (None, None) => true,
            (Some('*'), _) => go(&g[1..], n) || (!n.is_empty() && go(g, &n[1..])),
            (Some('?'), Some(_)) => go(&g[1..], &n[1..]),
            (Some(a), Some(b)) if a == b => go(&g[1..], &n[1..]),
            _ => false,
        }
    }
    let g: Vec<char> = glob.chars().collect();
    let n: Vec<char> = name.chars().collect();
    go(&g, &n)
}

/// The files an `Include` names: the path itself, or with `*` or `?` in its last part the files of that folder that
/// fit it, by name.
fn include_files(path: &Path, read_dir: &dyn Fn(&Path) -> Vec<String>) -> Vec<PathBuf> {
    let last = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !last.contains(['*', '?']) {
        return vec![path.to_path_buf()];
    }
    let Some(dir) = path.parent() else {
        return Vec::new();
    };
    let mut names: Vec<String> = read_dir(dir)
        .into_iter()
        .filter(|n| glob_fits(&last, n))
        .collect();
    names.sort();
    names.into_iter().map(|n| dir.join(n)).collect()
}

/// The hosts the config at `path` names, each with the computer it reaches (its `HostName`, else its own name), with
/// the reads given (a test gives its own).
pub fn named_hosts_with(
    path: &Path,
    home: &Path,
    read: &dyn Fn(&Path) -> Option<String>,
    read_dir: &dyn Fn(&Path) -> Vec<String>,
) -> Vec<SshHost> {
    let mut items: Vec<Item> = Vec::new();
    walk(path, home, read, read_dir, 0, &mut items);
    named_hosts_of(&items)
}

/// The most lines kept from one config and its `Include`s (a file that includes itself twice would grow without end).
const ITEMS_MAX: usize = 20_000;

/// The file's lines that matter, its `Include`s followed in place.
fn walk(
    path: &Path,
    home: &Path,
    read: &dyn Fn(&Path) -> Option<String>,
    read_dir: &dyn Fn(&Path) -> Vec<String>,
    depth: usize,
    out: &mut Vec<Item>,
) {
    if depth > INCLUDE_DEPTH {
        return;
    }
    let Some(text) = read(path) else {
        return;
    };
    for item in read_text(&text) {
        if out.len() >= ITEMS_MAX {
            return;
        }
        match item {
            Item::Include(arg) => {
                for f in include_files(&include_path(&arg, home), read_dir) {
                    walk(&f, home, read, read_dir, depth + 1, out);
                }
            }
            other => out.push(other),
        }
    }
}

/// Whether a `Host` line's patterns take `alias`, as ssh reads them: one pattern fits and no `!` pattern does.
fn block_fits(patterns: &[String], alias: &str) -> bool {
    let alias = alias.to_ascii_lowercase();
    let mut any = false;
    for p in patterns {
        let p = p.to_ascii_lowercase();
        if let Some(neg) = p.strip_prefix('!') {
            if glob_fits(neg, &alias) {
                return false;
            }
        } else if glob_fits(&p, &alias) {
            any = true;
        }
    }
    any
}

/// Pure: the plain hosts in order (once each, at most [`HOSTS_MAX`]), each with its `HostName`: the first one met in a
/// block that takes it, as ssh uses the first value it reads (`%h` is the host's own name, `%%` a `%`). The lines before
/// the first `Host` take every host; the lines under a `Match` take none here.
fn named_hosts_of(items: &[Item]) -> Vec<SshHost> {
    let mut aliases: Vec<String> = Vec::new();
    let mut names: Vec<(Vec<String>, String)> = Vec::new();
    let mut block: Option<Vec<String>> = Some(vec!["*".to_string()]);
    for item in items {
        match item {
            Item::Host(patterns) => {
                for h in patterns {
                    if aliases.len() < HOSTS_MAX && pickable(h) && !aliases.contains(h) {
                        aliases.push(h.clone());
                    }
                }
                block = Some(patterns.clone());
            }
            Item::Match => block = None,
            Item::HostName(name) => {
                if let Some(p) = &block {
                    names.push((p.clone(), name.clone()));
                }
            }
            Item::Include(_) => {}
        }
    }
    aliases
        .into_iter()
        .map(|alias| {
            let host_name = names
                .iter()
                .find(|(p, _)| block_fits(p, &alias))
                .map(|(_, n)| {
                    n.replace("%%", "\u{0}")
                        .replace("%h", &alias)
                        .replace('\u{0}', "%")
                })
                .unwrap_or_else(|| alias.clone());
            SshHost { alias, host_name }
        })
        .collect()
}

/// BLOCKING (file reads): the hosts of `<home>/.ssh/config`. A worker's call.
pub fn hosts_of_home(home: &Path) -> Vec<String> {
    named_hosts_of_home(home)
        .into_iter()
        .map(|h| h.alias)
        .collect()
}

/// BLOCKING (file reads): the hosts of `<home>/.ssh/config`, each with the computer it reaches. A worker's call.
pub fn named_hosts_of_home(home: &Path) -> Vec<SshHost> {
    let read = |p: &Path| std::fs::read_to_string(p).ok(); // sync-ok: the Machines page's worker reads the ssh config
    let read_dir = |d: &Path| {
        std::fs::read_dir(d) // sync-ok: the Machines page's worker lists an Include folder
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    };
    named_hosts_with(&home.join(".ssh").join("config"), home, &read, &read_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn hosts(files: &[(&str, &str)]) -> Vec<String> {
        let home = PathBuf::from("/home/max");
        let map: HashMap<PathBuf, String> = files
            .iter()
            .map(|(p, t)| (home.join(p), t.to_string()))
            .collect();
        let read = |p: &Path| map.get(p).cloned();
        let read_dir = |d: &Path| {
            map.keys()
                .filter(|k| k.parent() == Some(d))
                .map(|k| k.file_name().unwrap().to_string_lossy().into_owned())
                .collect()
        };
        named_hosts_with(&home.join(".ssh/config"), &home, &read, &read_dir)
            .into_iter()
            .map(|h| h.alias)
            .collect()
    }

    #[test]
    fn plain_hosts_are_listed_once_in_order_and_patterns_and_negations_are_not() {
        let cfg = "Host mac-mini\n  HostName 10.0.0.2\n  User max\n\nHost studio nas *.lan !bad laptop?\nHost *\n  ServerAliveInterval 30\nHost mac-mini\n";
        assert_eq!(
            hosts(&[(".ssh/config", cfg)]),
            ["mac-mini", "studio", "nas"]
        );
    }

    #[test]
    fn keywords_ignore_case_values_may_be_quoted_and_crlf_ends_a_line() {
        let cfg = "# a comment\r\nHOST=desk\r\n  hostname desk.local\r\nhost \"quoted\"\r\n\tHost\tlast\r\n";
        assert_eq!(hosts(&[(".ssh/config", cfg)]), ["desk", "quoted", "last"]);
    }

    #[test]
    fn a_name_with_a_space_or_a_leading_dash_is_never_listed() {
        // Rule 305: nothing read here may reach ssh as two items or as an option.
        let cfg = "Host \"two words\" -oProxyCommand=x ok\n";
        assert_eq!(hosts(&[(".ssh/config", cfg)]), ["ok"]);
    }

    #[test]
    fn match_blocks_add_no_host() {
        let cfg = "Match host foo exec \"true\"\n  User x\nHost real\n";
        assert_eq!(hosts(&[(".ssh/config", cfg)]), ["real"]);
    }

    #[test]
    fn includes_are_followed_relative_to_ssh_home_or_absolute_with_globs_in_name_order() {
        let cfg = "Include conf.d/*.conf\nHost top\nInclude ~/extra\n";
        let files = [
            (".ssh/config", cfg),
            (".ssh/conf.d/b.conf", "Host bee\n"),
            (".ssh/conf.d/a.conf", "Host ay\nInclude deep\n"),
            (".ssh/conf.d/skip.txt", "Host never\n"),
            (".ssh/deep", "Host deep\n"),
            ("extra", "Host extra\n"),
        ];
        assert_eq!(hosts(&files), ["ay", "deep", "bee", "top", "extra"]);
    }

    #[test]
    fn an_include_loop_ends_and_a_missing_file_is_no_host() {
        let files = [(".ssh/config", "Include config\nHost once\n")];
        assert_eq!(hosts(&files), ["once"]);
        assert!(hosts(&[]).is_empty());
    }

    fn named(files: &[(&str, &str)]) -> Vec<(String, String)> {
        let home = PathBuf::from("/home/max");
        let map: HashMap<PathBuf, String> = files
            .iter()
            .map(|(p, t)| (home.join(p), t.to_string()))
            .collect();
        let read = |p: &Path| map.get(p).cloned();
        let read_dir = |_: &Path| Vec::new();
        named_hosts_with(&home.join(".ssh/config"), &home, &read, &read_dir)
            .into_iter()
            .map(|h| (h.alias, h.host_name))
            .collect()
    }

    /// Rule 1276: each host comes with the computer it reaches, its first `HostName` as ssh reads it, else its name.
    #[test]
    fn each_host_has_its_first_host_name_else_its_own_name() {
        let cfg = "Host mac\n  HostName 192.168.0.120\n  User max\n\
                   Host a\nHost b\n  HostName b.example\n\
                   Host *.lan\n  HostName %h.example\n\
                   Host nas.lan\n  HostName never\n\
                   Host plain !plain2 plain2\n\
                   Match host plain\n  HostName from-match\n\
                   Host *\n  HostName %%%h\n";
        assert_eq!(
            named(&[(".ssh/config", cfg)]),
            [
                ("mac".to_string(), "192.168.0.120".to_string()),
                ("a".into(), "%a".into()),
                ("b".into(), "b.example".into()),
                ("nas.lan".into(), "nas.lan.example".into()),
                ("plain".into(), "%plain".into()),
                ("plain2".into(), "%plain2".into()),
            ]
        );
        assert_eq!(
            named(&[(
                ".ssh/config",
                "HostName first\nHost x\n  HostName second\nHost y\n"
            )]),
            [
                ("x".to_string(), "first".to_string()),
                ("y".into(), "first".into())
            ]
        );
        assert_eq!(
            named(&[(".ssh/config", "Host bare\n")]),
            [("bare".to_string(), "bare".to_string())]
        );
    }

    #[test]
    fn globs_fit_as_ssh_reads_them() {
        assert!(glob_fits("*.conf", "a.conf"));
        assert!(!glob_fits("*.conf", "a.txt"));
        assert!(glob_fits("h?st", "host"));
        assert!(!glob_fits("h?st", "hst"));
    }
}
