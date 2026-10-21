//! paths_from_argv, folders_from_argv, unquote_arg (Task 14).
//!
//! Every path a command line (or a hand-off) names is taken, in order: a folder roots a tab and a file
//! opens inside one. The app's own exe is skipped by identity, switches are skipped, and a relative
//! path is made absolute against the folder the command was typed in.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

/// The most paths one command line may bring. argv is not a promise.
pub const ARGV_MAX: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArgPath {
    pub path: PathBuf,
    pub dir: bool,
}

/// A drive root handed over by Explorer as `C:"` (the backslash before the closing quote escapes it)
/// is the root again: a trailing stray quote is that backslash, put back.
pub fn unquote_arg(a: &str) -> String {
    match a.strip_suffix('"') {
        Some(rest) => format!("{rest}\\"),
        None => a.to_string(),
    }
}

/// EVERY argument is made absolute against `base` (the folder the command line was typed in) with a
/// lexical clean (`.` and `..`, no canonicalize). Kept when it exists as a file or a folder, once each
/// (case-insensitive), never one of `ignore`, at most `ARGV_MAX`.
pub fn paths_from_argv(argv: &[String], ignore: &[PathBuf], base: &Path) -> Vec<ArgPath> {
    let mine: HashSet<String> = ignore.iter().map(|p| key(&clean(&base.join(p)))).collect();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for arg in argv.iter().skip(1) {
        if out.len() >= ARGV_MAX {
            break;
        }
        if arg.starts_with("--") {
            continue;
        }
        let a = clean(&base.join(unquote_arg(arg)));
        let Ok(meta) = std::fs::metadata(&a) else {
            // sync-ok: runs on the launch / hand-off worker
            continue;
        };
        let dir = meta.is_dir();
        if !dir && !meta.is_file() {
            continue;
        }
        let k = key(&a);
        if mine.contains(&k) || !seen.insert(k) {
            continue;
        }
        out.push(ArgPath { path: a, dir });
    }
    out
}

/// A terminal opens folders: a file argument means the folder it is in, each folder once.
pub fn folders_from_argv(argv: &[String], ignore: &[PathBuf], base: &Path) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for a in paths_from_argv(argv, ignore, base) {
        let dir = if a.dir {
            Some(a.path)
        } else {
            a.path.parent().map(Path::to_path_buf)
        };
        let Some(dir) = dir else {
            continue;
        };
        let text = dir.to_string_lossy().into_owned();
        if seen.insert(text.to_lowercase()) {
            out.push(text);
        }
    }
    out
}

/// The app's own files, which are never something to open: `[current_exe]`.
pub fn own_files() -> Vec<PathBuf> {
    std::env::current_exe().map(|p| vec![p]).unwrap_or_default()
}

/// Lexical clean: `.` dropped, `..` removes the component before it. No filesystem access.
fn clean(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn key(p: &Path) -> String {
    p.to_string_lossy().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    fn s(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    fn named(v: &[ArgPath]) -> Vec<(PathBuf, bool)> {
        v.iter().map(|a| (a.path.clone(), a.dir)).collect()
    }

    #[test]
    fn a_relative_path_is_resolved_against_the_typing_folder() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        fs::create_dir(base.join("src")).unwrap();
        assert_eq!(
            folders_from_argv(&args(&["exe", "src"]), &[], base),
            vec![s(&base.join("src"))]
        );
        assert_eq!(
            folders_from_argv(&args(&["exe", "."]), &[], base),
            vec![s(base)]
        );
        assert_eq!(
            folders_from_argv(&args(&["exe", "src\\.."]), &[], base),
            vec![s(base)]
        );
    }

    #[test]
    fn a_drive_root_with_the_escaped_quote_is_the_root() {
        assert_eq!(unquote_arg("C:\""), "C:\\");
        assert_eq!(unquote_arg("C:\\Users"), "C:\\Users");
        let dir = tempfile::tempdir().unwrap();
        let root: String = s(dir.path()).chars().take(2).collect();
        assert_eq!(
            folders_from_argv(&args(&["exe", &format!("{root}\"")]), &[], dir.path()),
            vec![format!("{root}\\")]
        );
    }

    #[test]
    fn takes_every_path_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        let a = base.join("a.jpg");
        let b = base.join("b.jpg");
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();
        let got = paths_from_argv(&args(&["prism.exe", &s(&a), &s(&b)]), &[], base);
        assert_eq!(named(&got), vec![(a, false), (b, false)]);
    }

    #[test]
    fn skips_the_own_exe_by_identity_wherever_it_sits() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        let own = base.join("flare.exe");
        let a = base.join("a.jpg");
        fs::write(&own, b"x").unwrap();
        fs::write(&a, b"a").unwrap();
        let got = paths_from_argv(
            &args(&["flare.exe", &s(&own), &s(&a)]),
            std::slice::from_ref(&own),
            base,
        );
        assert_eq!(named(&got), vec![(a.clone(), false)]);
        let got = paths_from_argv(
            &args(&[
                "flare.exe",
                "--user-data-dir=C:\\x",
                "--e2e",
                &s(&own),
                &s(&a),
            ]),
            std::slice::from_ref(&own),
            base,
        );
        assert_eq!(named(&got), vec![(a, false)]);
    }

    #[test]
    fn skips_switches() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        let a = base.join("a.jpg");
        fs::write(&a, b"a").unwrap();
        let got = paths_from_argv(
            &args(&["flare.exe", "--e2e", "--user-data-dir=C:\\x", &s(&a)]),
            &[],
            base,
        );
        assert_eq!(named(&got), vec![(a, false)]);
    }

    #[test]
    fn says_which_are_folders() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        let sub = base.join("sub");
        fs::create_dir(&sub).unwrap();
        let got = paths_from_argv(&args(&["flare.exe", &s(&sub)]), &[], base);
        assert_eq!(named(&got), vec![(sub, true)]);
    }

    #[test]
    fn ignores_what_does_not_exist() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        let a = base.join("a.jpg");
        fs::write(&a, b"a").unwrap();
        let missing = base.join("missing.jpg");
        let got = paths_from_argv(&args(&["flare.exe", &s(&missing), &s(&a)]), &[], base);
        assert_eq!(named(&got), vec![(a, false)]);
    }

    #[test]
    fn a_repeated_path_is_taken_once() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        let a = base.join("a.jpg");
        fs::write(&a, b"a").unwrap();
        let got = paths_from_argv(&args(&["flare.exe", &s(&a), &s(&a)]), &[], base);
        assert_eq!(got.len(), 1);
        let upper = s(&a).to_uppercase();
        let got = paths_from_argv(&args(&["flare.exe", &s(&a), &upper]), &[], base);
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn a_bare_launch_has_no_paths() {
        assert!(paths_from_argv(&args(&["flare.exe"]), &[], Path::new(".")).is_empty());
    }

    #[test]
    fn a_file_means_its_folder_once() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let file = d.join("a.txt");
        fs::write(&file, b"a").unwrap();
        assert_eq!(
            folders_from_argv(&args(&["exe", &s(&file), &s(d), "--e2e"]), &[], d),
            vec![s(d)]
        );
    }

    #[test]
    fn two_folders_keep_their_order() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        let two = base.join("two");
        let one = base.join("one");
        fs::create_dir(&two).unwrap();
        fs::create_dir(&one).unwrap();
        assert_eq!(
            folders_from_argv(&args(&["exe", &s(&two), &s(&one)]), &[], base),
            vec![s(&two), s(&one)]
        );
    }

    #[test]
    fn folders_of_a_bare_launch_are_empty() {
        assert!(folders_from_argv(&args(&["exe", "--e2e"]), &[], Path::new(".")).is_empty());
    }

    #[test]
    fn at_most_32_paths_are_taken() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        let mut argv = vec!["flare.exe".to_string()];
        for i in 0..40 {
            let sub = base.join(format!("f{i:02}"));
            fs::create_dir(&sub).unwrap();
            argv.push(s(&sub));
        }
        let got = paths_from_argv(&argv, &[], base);
        assert_eq!(got.len(), ARGV_MAX);
        for (i, p) in got.iter().enumerate() {
            assert_eq!(p.path, base.join(format!("f{i:02}")));
        }
    }
}
