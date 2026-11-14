//! The folders a file link is read from (rule 1248). A relative link is read from the agent's folder first, as ever
//! (rule 275). When no spelling of it names anything there, the worker tries, in this order and each folder once: the
//! folder the agent's session started in, the tab's own shell folder, the git repo root above the first folder (a
//! worktree's `.git` file counts as a root), and the main repo root that a worktree's `.git` file points to. Agents
//! write paths relative to their session's folder, but the folder their hooks last reported can differ (a `cd` into a
//! subfolder, a worktree, a stale report).
//!
//! Order: folders outer, spellings inner. The first folder is tried with every spelling (as written, then the alts),
//! then the next folder with every spelling, and so on. The first pair that names something wins, so the agent's
//! folder always wins over a fallback that also has the file.
//!
//! An absolute path, a rooted path with no drive and `~` are read from one folder only, as before. Nothing here runs
//! anything: the worker only stats paths and reads a worktree's small `.git` and `commondir` files. Every caller still
//! re-resolves the winning spelling under the winning folder and applies the allowlist (rules 13 and 14).

use std::path::{Component, Path, PathBuf};

use flare_term::path_open::first_existing;

/// Where a link was found. Its `label` is the privacy-safe word the `path-click` crumb carries: never the path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BaseKind {
    /// The folder the agent's hooks last named (or the tab's folder when there is no agent folder).
    Agent,
    /// The folder the agent's session first named.
    Start,
    /// The tab's own shell folder.
    Shell,
    /// A git repo root above the first folder, or the main repo of a worktree.
    Repo,
}

impl BaseKind {
    pub fn label(self) -> &'static str {
        match self {
            BaseKind::Agent => "agent",
            BaseKind::Start => "start",
            BaseKind::Shell => "shell",
            BaseKind::Repo => "repo",
        }
    }
}

/// The crumb's word when no folder has the link.
pub const BASE_NONE: &str = "none";

/// The folders a link is tried under, built on the UI thread with nothing statted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LinkBases {
    /// The folders, in order; the first is the one the link was always read from.
    pub tried: Vec<(BaseKind, String)>,
    /// Also try the git repo roots above the first folder (found on the worker). Only for a relative link.
    pub repo: bool,
}

impl From<String> for LinkBases {
    /// One folder and no fallback.
    fn from(base: String) -> Self {
        LinkBases {
            tried: vec![(BaseKind::Agent, base)],
            repo: false,
        }
    }
}

impl LinkBases {
    /// The folder the link was always read from ("" when none is known).
    pub fn first(&self) -> &str {
        self.tried.first().map_or("", |(_, b)| b.as_str())
    }

    /// Every folder to try, in order, each once. Worker only: the repo roots are found on the disk.
    fn candidates(&self) -> Vec<(BaseKind, String)> {
        let mut out: Vec<(BaseKind, String)> = Vec::new();
        let mut push = |kind: BaseKind, base: String| {
            if base.is_empty() {
                return;
            }
            let key = dedupe_key(&base);
            if !out.iter().any(|(_, b)| dedupe_key(b) == key) {
                out.push((kind, base));
            }
        };
        for (kind, base) in &self.tried {
            push(*kind, base.clone());
        }
        if self.repo && !self.first().is_empty() {
            for root in repo_roots(Path::new(self.first())) {
                push(BaseKind::Repo, root);
            }
        }
        out
    }
}

/// The folder and spelling that name something.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub kind: BaseKind,
    pub base: String,
    pub text: String,
}

/// The first folder, in order, under which a spelling names something, and that spelling (rule 1248). Worker only.
pub fn find_link(bases: &LinkBases, texts: &[String], home: &str) -> Option<Found> {
    bases.candidates().into_iter().find_map(|(kind, base)| {
        first_existing(&base, texts, home).map(|text| Found { kind, base, text })
    })
}

/// Two spellings of one folder compare equal: no trailing separator, and on Windows any case and either slash.
fn dedupe_key(p: &str) -> String {
    let t = p.trim_end_matches(['/', '\\']);
    if cfg!(windows) {
        t.replace('/', "\\").to_lowercase()
    } else {
        t.to_string()
    }
}

/// The most folders walked up looking for `.git`.
const WALK_MAX: usize = 64;
/// The most bytes read of a `.git` or `commondir` file.
const GIT_FILE_MAX: u64 = 4096;

/// The git repo roots above `start` (itself included): the nearest folder holding `.git` (a folder, or a worktree's
/// file), then, for a worktree, the main repo root its `.git` file points to. Empty when there is none.
pub fn repo_roots(start: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if !local_folder(start) {
        return out;
    }
    for dir in start.ancestors().take(WALK_MAX) {
        let dot = dir.join(".git");
        let Ok(meta) = std::fs::symlink_metadata(&dot) else {
            // sync-ok: link workers only, never the UI thread
            continue;
        };
        if meta.is_dir() {
            out.push(path_text(dir));
        } else if meta.is_file() {
            out.push(path_text(dir));
            if let Some(main) = worktree_main_root(dir, &dot) {
                out.push(path_text(&main));
            }
        }
        break;
    }
    out
}

/// The main repo root of a worktree whose `.git` file is `dot` in `dir`: its `gitdir:` names
/// `<main>/.git/worktrees/<name>`, whose `commondir` names `<main>/.git`.
fn worktree_main_root(dir: &Path, dot: &Path) -> Option<PathBuf> {
    let text = read_small(dot)?;
    let gitdir = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("gitdir:"))?
        .trim();
    if gitdir.is_empty() {
        return None;
    }
    let gitdir = lexical(&dir.join(gitdir));
    // The `.git` file is data in a folder the link came from: a `gitdir:` naming a network share (or a `\\?\` path)
    // is never read, so no click or hover makes Flare reach out to another machine.
    if !local_folder(&gitdir) {
        return None;
    }
    let common = match read_small(&gitdir.join("commondir")) {
        Some(c) if !c.trim().is_empty() => lexical(&gitdir.join(c.trim())),
        _ => {
            let up = gitdir.parent()?;
            if up.file_name()? != "worktrees" {
                return None;
            }
            up.parent()?.to_path_buf()
        }
    };
    if common.file_name()? != ".git" {
        return None;
    }
    common
        .parent()
        .filter(|p| local_folder(p))
        .map(Path::to_path_buf)
}

/// A folder a relative path resolves from: on Windows a drive path, never a network or `\\?\` one (the filter the
/// agent's folder gets in `appctl::link_folder`).
fn local_folder(p: &Path) -> bool {
    flare_term::path_open::resolve_term_path(&path_text(p), "x", "").is_some()
}

fn read_small(p: &Path) -> Option<String> {
    use std::io::Read;
    let f = std::fs::File::open(p).ok()?; // sync-ok: link workers only, a file of at most 4 KB
    let mut s = String::new();
    f.take(GIT_FILE_MAX).read_to_string(&mut s).ok()?; // sync-ok: link workers only, capped read
    Some(s)
}

/// `p` with `.` and `..` taken out, without asking the disk.
fn lexical(p: &Path) -> PathBuf {
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

/// A folder as the path grammar reads it: the platform's own separator.
fn path_text(p: &Path) -> String {
    let s = p.to_string_lossy().into_owned();
    if cfg!(windows) {
        s.replace('/', "\\")
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    fn texts(t: &[&str]) -> Vec<String> {
        t.iter().map(|x| (*x).to_string()).collect()
    }

    /// Rule 1248: the agent's folder is a subfolder; the file lives at the session's start folder.
    #[test]
    fn a_relative_link_missing_under_the_agents_folder_is_found_under_the_start_folder() {
        let root = tempfile::tempdir().unwrap();
        let sub = root.path().join("sub");
        std::fs::create_dir_all(sub.join("deep")).unwrap();
        std::fs::create_dir_all(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("src").join("App.tsx"), "x").unwrap();
        let bases = LinkBases {
            tried: vec![
                (BaseKind::Agent, s(&sub.join("deep"))),
                (BaseKind::Start, s(root.path())),
            ],
            repo: true,
        };
        let got = find_link(&bases, &texts(&["src/App.tsx"]), "").expect("found");
        assert_eq!(got.kind, BaseKind::Start);
        assert_eq!(got.base, s(root.path()));
        assert_eq!(got.text, "src/App.tsx");
    }

    #[test]
    fn the_agents_folder_wins_when_both_have_the_file() {
        let root = tempfile::tempdir().unwrap();
        let sub = root.path().join("sub");
        for d in [root.path(), sub.as_path()] {
            std::fs::create_dir_all(d.join("src")).unwrap();
            std::fs::write(d.join("src").join("a.rs"), "x").unwrap();
        }
        let bases = LinkBases {
            tried: vec![
                (BaseKind::Agent, s(&sub)),
                (BaseKind::Start, s(root.path())),
                (BaseKind::Shell, s(root.path())),
            ],
            repo: true,
        };
        let got = find_link(&bases, &texts(&["src/a.rs"]), "").unwrap();
        assert_eq!((got.kind, got.base), (BaseKind::Agent, s(&sub)));
    }

    #[test]
    fn folders_go_before_spellings() {
        // The second spelling names a file under the agent's folder; the first names one under the shell's.
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        std::fs::write(a.path().join("two.txt"), "x").unwrap();
        std::fs::write(b.path().join("one.txt"), "x").unwrap();
        let bases = LinkBases {
            tried: vec![
                (BaseKind::Agent, s(a.path())),
                (BaseKind::Shell, s(b.path())),
            ],
            repo: false,
        };
        let got = find_link(&bases, &texts(&["one.txt", "two.txt"]), "").unwrap();
        assert_eq!((got.kind, got.text.as_str()), (BaseKind::Agent, "two.txt"));
    }

    #[test]
    fn nothing_anywhere_is_none() {
        let a = tempfile::tempdir().unwrap();
        let bases = LinkBases {
            tried: vec![
                (BaseKind::Agent, s(a.path())),
                (BaseKind::Shell, s(a.path())),
            ],
            repo: true,
        };
        assert_eq!(find_link(&bases, &texts(&["gone.tsx"]), ""), None);
    }

    #[test]
    fn the_repo_root_above_the_agents_folder_is_tried() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".git")).unwrap();
        let deep = root.path().join("crates").join("x");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(root.path().join("README.md"), "x").unwrap();
        let bases = LinkBases {
            tried: vec![(BaseKind::Agent, s(&deep))],
            repo: true,
        };
        let got = find_link(&bases, &texts(&["README.md"]), "").unwrap();
        assert_eq!(got.kind, BaseKind::Repo);
        assert_eq!(dedupe_key(&got.base), dedupe_key(&s(root.path())));
        // Not a relative link's bases: no repo walk.
        let one: LinkBases = s(&deep).into();
        assert_eq!(find_link(&one, &texts(&["README.md"]), ""), None);
    }

    #[test]
    fn a_worktree_counts_as_a_root_and_its_main_repo_root_is_tried_after_it() {
        let main = tempfile::tempdir().unwrap();
        let common = main.path().join(".git");
        let admin = common.join("worktrees").join("wt");
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::write(admin.join("commondir"), "../..\n").unwrap();
        let wt = main.path().join(".claude").join("worktrees").join("wt");
        std::fs::create_dir_all(wt.join("crates")).unwrap();
        std::fs::write(
            wt.join(".git"),
            format!("gitdir: {}\n", s(&admin).replace('\\', "/")),
        )
        .unwrap();
        assert_eq!(
            repo_roots(&wt.join("crates"))
                .iter()
                .map(|r| dedupe_key(r))
                .collect::<Vec<_>>(),
            vec![dedupe_key(&s(&wt)), dedupe_key(&s(main.path()))]
        );
        // Only the main checkout has the file.
        std::fs::write(main.path().join("only-main.md"), "x").unwrap();
        let bases = LinkBases {
            tried: vec![(BaseKind::Agent, s(&wt.join("crates")))],
            repo: true,
        };
        let got = find_link(&bases, &texts(&["only-main.md"]), "").unwrap();
        assert_eq!(got.kind, BaseKind::Repo);
        assert_eq!(dedupe_key(&got.base), dedupe_key(&s(main.path())));
    }

    #[test]
    fn a_worktree_without_commondir_still_finds_its_main_root() {
        let main = tempfile::tempdir().unwrap();
        let admin = main.path().join(".git").join("worktrees").join("wt");
        std::fs::create_dir_all(&admin).unwrap();
        let wt = main.path().join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join(".git"), "gitdir: ../.git/worktrees/wt\n").unwrap();
        let roots = repo_roots(&wt);
        assert_eq!(roots.len(), 2, "{roots:?}");
        assert_eq!(dedupe_key(&roots[1]), dedupe_key(&s(main.path())));
    }

    /// Rule 1248: a worktree's `.git` file is data. A `gitdir:` that names no local folder (a network share, here a
    /// `\\?\` path so the test never touches the network) is never read and gives no main root.
    #[cfg(windows)]
    #[test]
    fn a_gitdir_that_is_no_local_folder_is_never_followed() {
        let main = tempfile::tempdir().unwrap();
        let admin = main.path().join(".git").join("worktrees").join("wt");
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::write(admin.join("commondir"), "../..\n").unwrap();
        let wt = main.path().join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: \\\\?\\{}\n", s(&admin))).unwrap();
        let roots = repo_roots(&wt);
        assert_eq!(roots.len(), 1, "{roots:?}");
        assert_eq!(dedupe_key(&roots[0]), dedupe_key(&s(&wt)));
        // A first folder that is no local folder is not walked at all.
        assert!(repo_roots(Path::new("\\\\server\\share\\x")).is_empty());
    }

    #[test]
    fn the_same_folder_is_tried_once_and_labels_are_fixed_words() {
        let bases = LinkBases {
            tried: vec![
                (BaseKind::Agent, "C:\\a\\".into()),
                (BaseKind::Start, "c:/A".into()),
                (BaseKind::Shell, String::new()),
            ],
            repo: false,
        };
        let c = bases.candidates();
        if cfg!(windows) {
            assert_eq!(c.len(), 1, "{c:?}");
        }
        assert_eq!(c[0].0, BaseKind::Agent);
        let words: Vec<_> = [
            BaseKind::Agent,
            BaseKind::Start,
            BaseKind::Shell,
            BaseKind::Repo,
        ]
        .iter()
        .map(|k| k.label())
        .collect();
        assert_eq!(words, ["agent", "start", "shell", "repo"]);
        assert_eq!(BASE_NONE, "none");
    }
}
