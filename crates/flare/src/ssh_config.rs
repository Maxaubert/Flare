//! The hosts `~/.ssh/config` names, for Settings > Machines (remote sessions PR 7): the SSH hosts of the Add a machine dialog's drop-down.
//!
//! Only `Host` lines are read, and only their plain names: a pattern (`*`, `?`, `!`) is no host one can pick, and the
//! lines under a `Match` block belong to no host. `Include` is followed (relative names under `~/.ssh`, `~/` for the
//! home folder, `*` and `?` in the last part), at most [`INCLUDE_DEPTH`] deep, as ssh itself does. Keywords are matched
//! without case, a value may be in double quotes, `Host=name` is the same as `Host name`, a CRLF ending is a line
//! ending. A name that would not be one plain argv item for ssh (`machines::argv_word`) is left out, so nothing read
//! here can become an option of ssh (rule 305).
//!
//! The reads are a worker's (rule 34): `hosts_of_home` is BLOCKING.

use std::path::{Path, PathBuf};

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
            "host" => {
                for a in args {
                    out.push(Item::Host(a));
                }
            }
            "include" => {
                for a in args {
                    out.push(Item::Include(a));
                }
            }
            _ => {}
        }
    }
    out
}

enum Item {
    Host(String),
    Include(String),
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

/// The hosts the config at `path` names, following its `Include`s, with the reads given (a test gives its own).
pub fn hosts_with(
    path: &Path,
    home: &Path,
    read: &dyn Fn(&Path) -> Option<String>,
    read_dir: &dyn Fn(&Path) -> Vec<String>,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    walk(path, home, read, read_dir, 0, &mut out);
    out
}

fn walk(
    path: &Path,
    home: &Path,
    read: &dyn Fn(&Path) -> Option<String>,
    read_dir: &dyn Fn(&Path) -> Vec<String>,
    depth: usize,
    out: &mut Vec<String>,
) {
    if depth > INCLUDE_DEPTH {
        return;
    }
    let Some(text) = read(path) else {
        return;
    };
    for item in read_text(&text) {
        if out.len() >= HOSTS_MAX {
            return;
        }
        match item {
            Item::Host(h) => {
                if pickable(&h) && !out.contains(&h) {
                    out.push(h);
                }
            }
            Item::Include(arg) => {
                for f in include_files(&include_path(&arg, home), read_dir) {
                    walk(&f, home, read, read_dir, depth + 1, out);
                }
            }
        }
    }
}

/// BLOCKING (file reads): the hosts of `<home>/.ssh/config`. A worker's call.
pub fn hosts_of_home(home: &Path) -> Vec<String> {
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
    hosts_with(&home.join(".ssh").join("config"), home, &read, &read_dir)
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
        hosts_with(&home.join(".ssh/config"), &home, &read, &read_dir)
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

    #[test]
    fn globs_fit_as_ssh_reads_them() {
        assert!(glob_fits("*.conf", "a.conf"));
        assert!(!glob_fits("*.conf", "a.txt"));
        assert!(glob_fits("h?st", "host"));
        assert!(!glob_fits("h?st", "hst"));
    }
}
