//! The remote folder browser (Max, 2026-11-17: mockup B, rule 1282): a machine's row in the + menu (and in Ctrl+Shift+R's
//! menu) opens a Flare-drawn dialog that lists the host's folders, starting at the host's home. Back and Home, a crumb
//! per folder of the path (the machine's name first), folders before files, files dimmed and never picked. A click
//! picks a folder (the footer names it), a double-click goes into it; Open here opens the remote tab in the picked
//! folder, else in the folder shown, and the folder joins the remote recents (rule 1213). Never the OS dialog.
//!
//! Each listing is one `remote.ls` over a short link to the machine (`Connector::connect`, the same link a remote tab
//! and Settings > Machines' Test start), on a worker (rule 34); the answer comes back as a [`FolderMsg`] and only the
//! newest ask is taken. Home asks with no `path`, which every host answers with its own home (a host from before this
//! change too). The host's answer is not trusted: its path must be a safe path (`remote_status::host_folder`), at most
//! `LS_MAX` names are taken, and a name that holds a separator, a control or invisible character, or is `.` or `..`, is
//! left out. Paths are joined and cut by the host's rules (`PathRules::of_host`), whatever this viewer is.
//!
//! `remote.ls` is the viewer's own: the agent's grant never reaches the `remote` namespace (rule S3, `grant.rs`).

use std::sync::Arc;
use std::time::Duration;

use flare_ctl::Method;
use flare_ctl::remote::LS_MAX;
use flare_tabs::sidebar_rows::PathRules;
use flare_tabs::tabs::Os;
use serde_json::{Value, json};

use crate::remote_link::{Connector, LinkError};

/// How long one listing may take once the link is up: the host's own limit for `remote.ls`.
pub const LS_WAIT: Duration = flare_ctl::method::REMOTE_CALL;

/// How long the dialog shows "Loading..." for one ask before it says the machine cannot be reached and offers Try
/// again (rule 1373). Long enough for the link to look for a machine that moved (`machine_follow`: a few short checks)
/// and for a host that answers at once; a slower answer that still comes is taken when it comes.
pub const LIST_LIMIT: Duration = Duration::from_secs(20);

/// Pure: the dialog's line for a machine that cannot be reached: nothing answered, its name was not found, or no
/// answer came within [`LIST_LIMIT`] (rule 1373).
pub fn unreachable_line(name: &str) -> String {
    format!("Cannot reach {name}. Check that it is on and on this network.")
}

/// The longest name the list takes (a longer one is no file name on any host Flare runs on).
const NAME_MAX: usize = 255;

/// The most crumbs of the path the bar shows: the root, a `…`, and the last ones.
const CRUMBS_SHOWN: usize = 4;

/// The dialog's title for a machine.
pub fn title(machine: &str) -> String {
    format!("Choose a folder on {machine}")
}

/// One name in a host folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub dir: bool,
}

/// A host folder as `remote.ls` named it: its path on the host, its names (folders first, then by name), and whether
/// the host left names out (`LS_MAX`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listing {
    pub path: String,
    pub entries: Vec<Entry>,
    pub truncated: bool,
}

/// Pure: whether a name from the host is one the list shows: no separator, no control or invisible character, not `.`
/// or `..`, not empty, not overlong.
fn plain_name(n: &str) -> bool {
    !n.is_empty()
        && n != "."
        && n != ".."
        && n.chars().count() <= NAME_MAX
        && !n.contains(['/', '\\'])
        && !n
            .chars()
            .any(|c| c.is_control() || flare_ctl::text::invisible(c))
}

/// Pure: the host's `remote.ls` answer read (rule 1282): its path a safe path, at most `LS_MAX` names, each a plain
/// name; a name that is not is left out. The line that says why not, for an answer that is no folder list.
pub fn parse_listing(v: &Value) -> Result<Listing, String> {
    let bad = || "the host's answer is no folder list".to_string();
    let path = v["path"].as_str().ok_or_else(bad)?;
    if !crate::remote_status::host_folder(path) {
        return Err(bad());
    }
    let all = v["entries"].as_array().ok_or_else(bad)?;
    let entries: Vec<Entry> = all
        .iter()
        .take(LS_MAX)
        .filter_map(|e| {
            let name = e["name"].as_str()?;
            plain_name(name).then(|| Entry {
                name: name.to_string(),
                dir: e["dir"].as_bool() == Some(true),
            })
        })
        .collect();
    Ok(Listing {
        path: path.to_string(),
        entries,
        truncated: v["truncated"].as_bool() == Some(true) || all.len() > LS_MAX,
    })
}

/// One crumb of a path: its words and the folder it goes to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Crumb {
    pub label: String,
    pub path: String,
}

/// Pure: a host path's crumbs by the host's rules, root first. A PC's root is its drive (`C:`, going to `C:\`); a Mac's
/// or Linux's is `/`. `C:\Users\Admin` is `C:`, `Users`, `Admin`; `/Users/max` is `/`, `Users`, `max`.
pub fn crumbs(path: &str, rules: PathRules) -> Vec<Crumb> {
    let mut out = Vec::new();
    match rules {
        PathRules::Windows => {
            let p = path.replace('/', "\\");
            let b = p.as_bytes();
            let drive = b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':';
            let (root_label, root_path, rest) = if drive {
                (p[..2].to_string(), format!("{}\\", &p[..2]), &p[2..])
            } else {
                ("\\".to_string(), "\\".to_string(), p.as_str())
            };
            out.push(Crumb {
                label: root_label,
                path: root_path.clone(),
            });
            let mut at = root_path;
            for seg in rest.split('\\').filter(|s| !s.is_empty()) {
                if !at.ends_with('\\') {
                    at.push('\\');
                }
                at.push_str(seg);
                out.push(Crumb {
                    label: seg.to_string(),
                    path: at.clone(),
                });
            }
        }
        PathRules::Posix => {
            out.push(Crumb {
                label: "/".into(),
                path: "/".into(),
            });
            let mut at = String::new();
            for seg in path.split('/').filter(|s| !s.is_empty()) {
                at.push('/');
                at.push_str(seg);
                out.push(Crumb {
                    label: seg.to_string(),
                    path: at.clone(),
                });
            }
        }
    }
    out
}

/// Pure: the folder `name` inside `path`, by the host's rules.
pub fn child(path: &str, name: &str, rules: PathRules) -> String {
    let sep = match rules {
        PathRules::Windows => '\\',
        PathRules::Posix => '/',
    };
    // A root (`C:\`, `/`) loses its separator here and gets it back below.
    let base = path.trim_end_matches(['/', '\\']);
    format!("{base}{sep}{name}")
}

/// Pure: the folder `path` sits in, by the host's rules; none at a root (the crumbs' own reading, held by the tests).
#[cfg(test)]
fn parent(path: &str, rules: PathRules) -> Option<String> {
    let c = crumbs(path, rules);
    (c.len() >= 2).then(|| c[c.len() - 2].path.clone())
}

/// What a crumb of the bar does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CrumbGo {
    /// The machine's name: the host's home.
    Home,
    /// A folder of the path.
    Folder(String),
    /// The `…` that stands for the folders between: nothing.
    Nothing,
}

/// One crumb as the bar draws it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrumbView {
    pub label: String,
    pub go: CrumbGo,
    pub last: bool,
}

/// Pure: the bar's crumbs: the machine's name, then the path's crumbs, the middle ones given way to one `…` when there
/// are more than `CRUMBS_SHOWN`. Before the first answer, the machine's name alone.
pub fn crumb_views(machine: &str, path: Option<&str>, rules: PathRules) -> Vec<CrumbView> {
    let mut out = vec![CrumbView {
        label: machine.to_string(),
        go: CrumbGo::Home,
        last: false,
    }];
    if let Some(p) = path {
        let all = crumbs(p, rules);
        let n = all.len();
        let shown: Vec<CrumbView> = if n > CRUMBS_SHOWN {
            let mut v = vec![CrumbView {
                label: all[0].label.clone(),
                go: CrumbGo::Folder(all[0].path.clone()),
                last: false,
            }];
            v.push(CrumbView {
                label: "\u{2026}".into(),
                go: CrumbGo::Nothing,
                last: false,
            });
            v.extend(all[n - (CRUMBS_SHOWN - 1)..].iter().map(|c| CrumbView {
                label: c.label.clone(),
                go: CrumbGo::Folder(c.path.clone()),
                last: false,
            }));
            v
        } else {
            all.iter()
                .map(|c| CrumbView {
                    label: c.label.clone(),
                    go: CrumbGo::Folder(c.path.clone()),
                    last: false,
                })
                .collect()
        };
        out.extend(shown);
    }
    if let Some(l) = out.last_mut() {
        l.last = true;
    }
    out
}

/// One ask for a listing: its number and the folder (none: the host's home).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    pub seq: u64,
    pub path: Option<String>,
}

/// A worker's answer: the listing for ask `seq`, or the one line that says why not.
#[derive(Debug)]
pub struct FolderMsg {
    pub seq: u64,
    pub answer: Result<Listing, String>,
}

/// The dialog's state (UI thread).
#[derive(Clone, Debug)]
pub struct Browser {
    /// The machine's id in `machines.json` and its name.
    pub machine: String,
    pub name: String,
    kind: Option<Os>,
    /// The folder shown, as the host named it; none before the first answer.
    path: Option<String>,
    listing: Option<Listing>,
    /// The picked row (a folder).
    selected: Option<usize>,
    loading: bool,
    error: Option<String>,
    /// The folders shown before, for Back.
    back: Vec<String>,
    seq: u64,
    /// The folder the newest ask named (none: the home), for Try again.
    asked: Option<String>,
}

impl Browser {
    /// A new dialog for a machine, and its first ask: the host's home.
    pub fn new(machine: &str, name: &str, kind: Option<Os>, seq: u64) -> (Self, Ask) {
        let b = Self {
            machine: machine.to_string(),
            name: name.to_string(),
            kind,
            path: None,
            listing: None,
            selected: None,
            loading: true,
            error: None,
            back: Vec::new(),
            seq,
            asked: None,
        };
        (b, Ask { seq, path: None })
    }

    /// The host's path rules: the machine's system, else the path's shape.
    pub fn rules(&self) -> PathRules {
        PathRules::of_host(self.kind, self.path.as_deref().unwrap_or(""))
    }

    fn ask(&mut self, path: Option<String>) -> Ask {
        self.seq += 1;
        self.loading = true;
        self.error = None;
        self.selected = None;
        self.asked.clone_from(&path);
        Ask {
            seq: self.seq,
            path,
        }
    }

    /// Goes to `path` (none: the home); the folder shown joins Back's list.
    fn go(&mut self, path: Option<String>) -> Option<Ask> {
        if let Some(cur) = self.path.clone()
            && path.as_deref() != Some(cur.as_str())
        {
            self.back.push(cur);
        }
        Some(self.ask(path))
    }

    /// Home: the host's home folder.
    pub fn home(&mut self) -> Option<Ask> {
        self.go(None)
    }

    /// Back: the folder shown before this one.
    pub fn go_back(&mut self) -> Option<Ask> {
        let to = self.back.pop()?;
        Some(self.ask(Some(to)))
    }

    /// Try again (rule 1373): the failed ask once more, as a new ask. None while an ask runs or nothing failed.
    pub fn retry(&mut self) -> Option<Ask> {
        if self.loading || self.error.is_none() {
            return None;
        }
        let path = self.asked.clone();
        self.go(path)
    }

    /// Whether Back has somewhere to go.
    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }

    /// A crumb of the bar, by its place in [`Self::crumbs`].
    pub fn crumb(&mut self, i: usize) -> Option<Ask> {
        match self.crumbs().get(i)?.go.clone() {
            CrumbGo::Home => self.home(),
            CrumbGo::Folder(p) => self.go(Some(p)),
            CrumbGo::Nothing => None,
        }
    }

    /// A click on a row: a folder is picked, a file is not.
    pub fn select(&mut self, i: usize) {
        if self.entry(i).is_some_and(|e| e.dir) {
            self.selected = Some(i);
        }
    }

    /// Up or Down: the next folder that way is picked.
    pub fn step(&mut self, down: bool) {
        let Some(l) = &self.listing else { return };
        let dirs: Vec<usize> = (0..l.entries.len()).filter(|&i| l.entries[i].dir).collect();
        let at = self
            .selected
            .and_then(|s| dirs.iter().position(|&d| d == s));
        let next = match (at, down) {
            (None, true) => dirs.first(),
            (None, false) => dirs.last(),
            (Some(a), true) => dirs.get(a + 1).or(dirs.get(a)),
            (Some(a), false) => dirs.get(a.saturating_sub(1)),
        };
        if let Some(&n) = next {
            self.selected = Some(n);
        }
    }

    /// A double-click on a row: into that folder.
    pub fn enter(&mut self, i: usize) -> Option<Ask> {
        let e = self.entry(i).filter(|e| e.dir)?.clone();
        let base = self.path.clone()?;
        let rules = self.rules();
        self.go(Some(child(&base, &e.name, rules)))
    }

    fn entry(&self, i: usize) -> Option<&Entry> {
        self.listing.as_ref()?.entries.get(i)
    }

    /// A worker's answer: taken only for the newest ask. True when it was taken. An ask already answered (its time ran
    /// out, rule 1373) still takes a listing that comes late, never a second failure.
    pub fn answered(&mut self, m: FolderMsg) -> bool {
        if m.seq != self.seq || (!self.loading && m.answer.is_err()) {
            return false;
        }
        let late = !self.loading;
        self.loading = false;
        match m.answer {
            Ok(l) => {
                // A late listing for a step that was taken back when its time ran out: the step is taken again.
                if late
                    && let Some(cur) = self.path.clone()
                    && cur != l.path
                    && self.back.last() != Some(&cur)
                {
                    self.back.push(cur);
                }
                self.path = Some(l.path.clone());
                self.listing = Some(l);
                self.error = None;
                self.selected = None;
            }
            Err(line) => {
                // The folder shown stays; the step that was taken toward the failed one is taken back.
                if self.back.last().is_some() && self.back.last() == self.path.as_ref() {
                    self.back.pop();
                }
                self.error = Some(line);
            }
        }
        true
    }

    /// The folder Open here opens: the picked folder, else the folder shown, else the host's home (as before the
    /// first answer: what a machine's row always did).
    pub fn target(&self) -> String {
        match (&self.path, self.selected.and_then(|i| self.entry(i))) {
            (Some(p), Some(e)) => child(p, &e.name, self.rules()),
            (Some(p), None) => p.clone(),
            _ => crate::machines::HOST_HOME.to_string(),
        }
    }

    /// The footer's path: what Open here opens, once the host has named a folder.
    pub fn footer(&self) -> String {
        if self.path.is_some() {
            self.target()
        } else {
            String::new()
        }
    }

    pub fn crumbs(&self) -> Vec<CrumbView> {
        crumb_views(&self.name, self.path.as_deref(), self.rules())
    }

    pub fn entries(&self) -> &[Entry] {
        self.listing.as_ref().map_or(&[], |l| &l.entries)
    }

    pub fn truncated(&self) -> bool {
        self.listing.as_ref().is_some_and(|l| l.truncated)
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub fn loading(&self) -> bool {
        self.loading
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    pub fn seq(&self) -> u64 {
        self.seq
    }
}

/// Lists a host folder. BLOCKING: a worker's call.
pub trait FolderLister: Send + Sync {
    /// One folder of `machine` (its id), or its home with no `path`.
    fn list(&self, machine: &str, path: Option<&str>) -> Result<Value, LinkError>;
}

/// The real lister: a short link to the machine (ssh, or under `--e2e` the loopback), `remote.ls`, and the link let go.
pub struct LinkLister {
    connector: Arc<dyn Connector>,
}

impl LinkLister {
    pub fn new(connector: Arc<dyn Connector>) -> Self {
        Self { connector }
    }
}

impl FolderLister for LinkLister {
    fn list(&self, machine: &str, path: Option<&str>) -> Result<Value, LinkError> {
        let mut link = self.connector.connect(machine)?;
        let params = match path {
            Some(p) => json!({ "path": p }),
            None => json!({}),
        };
        link.call(Method::RemoteLs, params, LS_WAIT)
    }
}

/// No lister (the scenes, and a test that sets none): every ask is one line.
pub struct NoLister;

impl FolderLister for NoLister {
    fn list(&self, _machine: &str, _path: Option<&str>) -> Result<Value, LinkError> {
        Err(LinkError::Down("no link here".into()))
    }
}

/// BLOCKING: one ask answered: the listing, or the one line the dialog shows. A machine that cannot be reached (nothing
/// answered, its name not found) is [`unreachable_line`] (rule 1373); anything else is `machines::failure_line`'s
/// words, as on Settings > Machines.
pub fn answer(lister: &dyn FolderLister, machine: &str, name: &str, ask: &Ask) -> FolderMsg {
    use crate::ssh_problem::{SshProblem, sort_down};
    let answer = match lister.list(machine, ask.path.as_deref()) {
        Ok(v) => parse_listing(&v),
        Err(LinkError::Down(m))
            if matches!(
                sort_down(&m),
                Some(SshProblem::NotAnswering | SshProblem::NotFound)
            ) =>
        {
            Err(unreachable_line(name))
        }
        Err(e) => Err(crate::machines::failure_line(
            name,
            &e,
            flare_ctl::REMOTE_PROTOCOL,
        )),
    };
    FolderMsg {
        seq: ask.seq,
        answer,
    }
}

/// BLOCKING (a worker's, rule 34): one ask answered within `limit` (rule 1373). The listing runs on a thread of its own;
/// when no answer came within `limit`, `post` gets [`unreachable_line`] at once, and the answer still gets posted when
/// it comes (the dialog takes a late listing, never a late failure). So the dialog never waits on a link with no end.
pub fn answer_in_time(
    lister: Arc<dyn FolderLister>,
    machine: &str,
    name: &str,
    ask: &Ask,
    limit: Duration,
    post: &dyn Fn(FolderMsg),
) {
    let (tx, rx) = std::sync::mpsc::channel();
    let (m, n, a) = (machine.to_string(), name.to_string(), ask.clone());
    let started = std::thread::Builder::new()
        .name("flare-remote-ls-link".into())
        .spawn(move || {
            let _ = tx.send(answer(&*lister, &m, &n, &a));
        });
    if let Err(e) = started {
        log::warn!("could not start the folder list's link: {e}");
        return post(FolderMsg {
            seq: ask.seq,
            answer: Err("Flare could not ask the machine.".into()),
        });
    }
    match rx.recv_timeout(limit) {
        // sync-ok: the folder list's worker waits for its link, at most `limit`
        Ok(m) => post(m),
        Err(_) => {
            log::info!(
                "remote folder: no answer from {machine} in {} s",
                limit.as_secs()
            );
            post(FolderMsg {
                seq: ask.seq,
                answer: Err(unreachable_line(name)),
            });
            // A late answer (a host that started its Flare slowly) is still shown.
            if let Ok(m) = rx.recv() {
                // sync-ok: the folder list's worker waits for its link's late answer, never the UI thread
                post(m);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_ctl::{CtlError, code};

    fn listing(path: &str, names: &[(&str, bool)]) -> Listing {
        Listing {
            path: path.into(),
            entries: names
                .iter()
                .map(|(n, d)| Entry {
                    name: (*n).into(),
                    dir: *d,
                })
                .collect(),
            truncated: false,
        }
    }

    /// Rule 1282: the host's answer is read, never trusted: its path a safe path, a name with a separator, a control or
    /// invisible character, `.` or `..` left out, at most `LS_MAX` names, and anything else no folder list.
    #[test]
    fn a_hosts_listing_is_read_with_only_plain_names_and_a_safe_path() {
        let v = json!({
            "path": r"C:\Users\Admin",
            "entries": [
                {"name": "Documents", "dir": true},
                {"name": "..", "dir": true},
                {"name": ".", "dir": true},
                {"name": r"a\b", "dir": true},
                {"name": "a/b", "dir": false},
                {"name": "evil\u{1b}]0;x\u{7}", "dir": true},
                {"name": "zero\u{200b}width", "dir": true},
                {"name": "", "dir": true},
                {"name": 7, "dir": true},
                {"name": ".gitconfig", "dir": false},
                {"name": "notes.txt"},
            ],
            "truncated": false,
        });
        let l = parse_listing(&v).unwrap();
        assert_eq!(l.path, r"C:\Users\Admin");
        assert_eq!(
            l.entries,
            listing(
                "",
                &[
                    ("Documents", true),
                    (".gitconfig", false),
                    ("notes.txt", false)
                ]
            )
            .entries
        );
        assert!(!l.truncated);
        for bad in [
            json!({"entries": []}),
            json!({"path": r"\\evil\share", "entries": []}),
            json!({"path": "C:\\x\u{7}", "entries": []}),
            json!({"path": "/Users/max"}),
            json!({"path": "/Users/max", "entries": "x"}),
            json!(null),
        ] {
            assert!(parse_listing(&bad).is_err(), "{bad}");
        }
        // More names than LS_MAX: the first LS_MAX, and the list says it was cut.
        let many: Vec<Value> = (0..LS_MAX + 7)
            .map(|i| json!({"name": format!("f{i}"), "dir": false}))
            .collect();
        let l = parse_listing(&json!({"path": "/x", "entries": many})).unwrap();
        assert_eq!(l.entries.len(), LS_MAX);
        assert!(l.truncated);
        let l = parse_listing(&json!({"path": "/x", "entries": [], "truncated": true})).unwrap();
        assert!(l.truncated);
    }

    /// Rule 1282: crumbs, children and parents by the host's rules on any viewer: a PC's drive is its root (`C:`
    /// going to `C:\`), a Mac's `/`.
    #[test]
    fn crumbs_follow_the_hosts_path_rules() {
        let w = |p: &str| -> Vec<(String, String)> {
            crumbs(p, PathRules::Windows)
                .into_iter()
                .map(|c| (c.label, c.path))
                .collect()
        };
        let pairs = |v: &[(&str, &str)]| -> Vec<(String, String)> {
            v.iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect()
        };
        assert_eq!(
            w(r"C:\Users\Admin"),
            pairs(&[
                ("C:", r"C:\"),
                ("Users", r"C:\Users"),
                ("Admin", r"C:\Users\Admin")
            ])
        );
        assert_eq!(w(r"D:\"), pairs(&[("D:", r"D:\")]));
        assert_eq!(
            w("D:/src/flare/"),
            pairs(&[
                ("D:", r"D:\"),
                ("src", r"D:\src"),
                ("flare", r"D:\src\flare")
            ])
        );
        let p = |p: &str| -> Vec<(String, String)> {
            crumbs(p, PathRules::Posix)
                .into_iter()
                .map(|c| (c.label, c.path))
                .collect()
        };
        assert_eq!(
            p("/Users/max"),
            pairs(&[("/", "/"), ("Users", "/Users"), ("max", "/Users/max")])
        );
        assert_eq!(p("/"), pairs(&[("/", "/")]));
        assert_eq!(
            child(r"C:\Users\Admin", "Documents", PathRules::Windows),
            r"C:\Users\Admin\Documents"
        );
        assert_eq!(child(r"C:\", "Users", PathRules::Windows), r"C:\Users");
        assert_eq!(child("/", "Users", PathRules::Posix), "/Users");
        assert_eq!(child("/Users/max/", "p", PathRules::Posix), "/Users/max/p");
        assert_eq!(
            parent(r"C:\Users\Admin", PathRules::Windows).as_deref(),
            Some(r"C:\Users")
        );
        assert_eq!(
            parent(r"C:\Users", PathRules::Windows).as_deref(),
            Some(r"C:\")
        );
        assert_eq!(parent(r"C:\", PathRules::Windows), None);
        assert_eq!(parent("/Users", PathRules::Posix).as_deref(), Some("/"));
        assert_eq!(parent("/", PathRules::Posix), None);
    }

    /// The bar: the machine's name (Home), then the path's crumbs; a deep path keeps its root and its last three with
    /// one `…` (which goes nowhere) between.
    #[test]
    fn the_bar_names_the_machine_then_the_path_and_shortens_a_deep_one() {
        let labels = |v: &[CrumbView]| v.iter().map(|c| c.label.clone()).collect::<Vec<_>>();
        let v = crumb_views("home-pc", Some(r"C:\Users\Admin"), PathRules::Windows);
        assert_eq!(labels(&v), ["home-pc", "C:", "Users", "Admin"]);
        assert_eq!(v[0].go, CrumbGo::Home);
        assert_eq!(v[2].go, CrumbGo::Folder(r"C:\Users".into()));
        assert!(v[3].last && !v[2].last);
        let v = crumb_views("studio", Some("/Users/max/a/b/c"), PathRules::Posix);
        assert_eq!(labels(&v), ["studio", "/", "\u{2026}", "a", "b", "c"]);
        assert_eq!(v[2].go, CrumbGo::Nothing);
        assert_eq!(v[3].go, CrumbGo::Folder("/Users/max/a".into()));
        let v = crumb_views("home-pc", None, PathRules::Windows);
        assert_eq!(labels(&v), ["home-pc"]);
        assert!(v[0].last);
    }

    fn browser() -> Browser {
        let (mut b, ask) = Browser::new("pc", "home-pc", Some(Os::Pc), 1);
        assert_eq!(ask, Ask { seq: 1, path: None }, "the first ask is the home");
        assert!(b.loading());
        assert_eq!(
            b.target(),
            crate::machines::HOST_HOME,
            "Open here before any answer: the home"
        );
        assert_eq!(b.footer(), "");
        let mut l = listing(
            r"C:\Users\Admin",
            &[("Desktop", true), ("Documents", true), ("notes.txt", false)],
        );
        l.truncated = true;
        assert!(b.answered(FolderMsg {
            seq: 1,
            answer: Ok(l)
        }));
        b
    }

    /// Rule 1282: a click picks a folder (never a file) and the footer names it; a double-click goes in, Back comes
    /// back, Home asks with no path; Open here opens the pick, else the folder shown.
    #[test]
    fn the_browser_picks_enters_and_goes_back() {
        let mut b = browser();
        assert_eq!(b.path(), Some(r"C:\Users\Admin"));
        assert!(b.truncated());
        assert_eq!(b.footer(), r"C:\Users\Admin");
        b.select(2);
        assert_eq!(b.selected(), None, "a file is never picked");
        b.select(1);
        assert_eq!(b.target(), r"C:\Users\Admin\Documents");
        assert_eq!(b.footer(), r"C:\Users\Admin\Documents");
        assert!(b.enter(2).is_none(), "no file is entered");
        let ask = b.enter(1).expect("into Documents");
        assert_eq!(ask.path.as_deref(), Some(r"C:\Users\Admin\Documents"));
        assert!(b.can_back());
        assert!(b.answered(FolderMsg {
            seq: ask.seq,
            answer: Ok(listing(r"C:\Users\Admin\Documents", &[("p", true)]))
        }));
        assert_eq!(b.selected(), None);
        let ask = b.go_back().expect("back");
        assert_eq!(ask.path.as_deref(), Some(r"C:\Users\Admin"));
        assert!(!b.can_back());
        b.answered(FolderMsg {
            seq: ask.seq,
            answer: Ok(listing(r"C:\Users\Admin", &[("Desktop", true)])),
        });
        // A crumb: C: goes to the drive's root.
        let ask = b.crumb(1).expect("C:");
        assert_eq!(ask.path.as_deref(), Some(r"C:\"));
        let ask = b.home().expect("home");
        assert_eq!(ask.path, None);
        assert_eq!(
            b.crumb(0).map(|a| a.path),
            Some(None),
            "the machine's name is Home"
        );
    }

    /// Only the newest ask's answer is taken; a failed ask keeps the folder shown, says one line, and takes back the
    /// step toward it.
    #[test]
    fn a_late_answer_is_dropped_and_a_failure_is_one_line() {
        let mut b = browser();
        let first = b.enter(0).unwrap();
        let second = b.home().unwrap();
        assert!(!b.answered(FolderMsg {
            seq: first.seq,
            answer: Ok(listing(r"C:\Users\Admin\Desktop", &[]))
        }));
        assert!(b.loading());
        assert!(b.answered(FolderMsg {
            seq: second.seq,
            answer: Err("Not answering".into())
        }));
        assert_eq!(b.error(), Some("Not answering"));
        assert!(!b.loading());
        assert_eq!(b.path(), Some(r"C:\Users\Admin"), "the folder shown stays");
        // The two steps taken: Desktop's was pushed then Home's; the failed one is taken back.
        let ask = b.go_back().unwrap();
        assert!(b.error().is_none(), "a new ask clears the line");
        assert_eq!(ask.path.as_deref(), Some(r"C:\Users\Admin"));
    }

    #[test]
    fn up_and_down_step_over_the_folders_only() {
        let mut b = browser();
        b.step(true);
        assert_eq!(b.selected(), Some(0));
        b.step(true);
        assert_eq!(b.selected(), Some(1));
        b.step(true);
        assert_eq!(
            b.selected(),
            Some(1),
            "the file is skipped, the last folder stays"
        );
        b.step(false);
        assert_eq!(b.selected(), Some(0));
        b.step(false);
        assert_eq!(b.selected(), Some(0));
    }

    struct Fixed(Result<Value, (bool, String)>);
    impl FolderLister for Fixed {
        fn list(&self, _m: &str, _p: Option<&str>) -> Result<Value, LinkError> {
            match &self.0 {
                Ok(v) => Ok(v.clone()),
                Err((true, m)) => Err(LinkError::Refused(CtlError::new(
                    code::NOT_FOUND,
                    m.clone(),
                ))),
                Err((false, m)) => Err(LinkError::Down(m.clone())),
            }
        }
    }

    /// A machine that does not answer is the one line Settings > Machines says; a refusal names the host.
    #[test]
    fn an_answer_is_a_listing_or_one_line() {
        let ask = Ask { seq: 4, path: None };
        let m = answer(
            &Fixed(Ok(json!({"path": "/Users/max", "entries": []}))),
            "mac",
            "Mac Mini",
            &ask,
        );
        assert_eq!(m.seq, 4);
        assert_eq!(m.answer.unwrap().path, "/Users/max");
        let m = answer(
            &Fixed(Err((
                false,
                "the link ended; ssh said: ssh: connect to host mini port 22: Connection refused"
                    .into(),
            ))),
            "mac",
            "Mac Mini",
            &ask,
        );
        let line = m.answer.unwrap_err();
        assert!(!line.contains('\n'), "{line}");
        // Rule 1373: a machine that cannot be reached says so in full, with what to check.
        assert_eq!(
            line,
            "Cannot reach Mac Mini. Check that it is on and on this network."
        );
        let m = answer(
            &Fixed(Err((
                false,
                "the link ended; ssh said: ssh: Could not resolve hostname mini: No such host is known."
                    .into(),
            ))),
            "mac",
            "Mac Mini",
            &ask,
        );
        assert_eq!(m.answer.unwrap_err(), unreachable_line("Mac Mini"));
        // Another key there is not "cannot reach": it is the identity line.
        let m = answer(
            &Fixed(Err((
                false,
                "the link ended; ssh said: Host key verification failed.".into(),
            ))),
            "mac",
            "Mac Mini",
            &ask,
        );
        assert_eq!(m.answer.unwrap_err(), "Its identity changed. Pair again");
        let m = answer(
            &Fixed(Err((true, "that folder cannot be read on the host".into()))),
            "mac",
            "Mac Mini",
            &ask,
        );
        assert_eq!(
            m.answer.unwrap_err(),
            "Mac Mini said: that folder cannot be read on the host"
        );
    }

    /// A lister that waits until the test lets it answer (a link that hangs).
    struct Gate {
        open: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
    }
    impl FolderLister for Gate {
        fn list(&self, _m: &str, _p: Option<&str>) -> Result<Value, LinkError> {
            let _ = self.open.lock().unwrap().recv();
            Ok(json!({"path": "/Users/max", "entries": [{"name": "proj", "dir": true}]}))
        }
    }

    /// Rule 1373: an ask with no answer within the limit gives the "Cannot reach" line at once (the dialog never shows
    /// Loading forever); the listing that comes after is still posted, and the dialog takes it.
    #[test]
    fn an_ask_with_no_answer_in_time_is_the_cannot_reach_line_and_a_late_listing_is_taken() {
        let (go, gate) = std::sync::mpsc::channel();
        let lister: Arc<dyn FolderLister> = Arc::new(Gate {
            open: std::sync::Mutex::new(gate),
        });
        let (mut b, ask) = Browser::new("mac", "Maxs-Mac-Mini", Some(Os::Mac), 1);
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            answer_in_time(
                lister,
                "mac",
                "Maxs-Mac-Mini",
                &ask,
                Duration::from_millis(50),
                &|m| tx.send(m).unwrap(),
            );
        });
        let first = rx.recv_timeout(Duration::from_secs(10)).expect("in time");
        assert!(b.answered(first));
        assert!(!b.loading(), "the error state, not Loading");
        assert_eq!(
            b.error(),
            Some("Cannot reach Maxs-Mac-Mini. Check that it is on and on this network.")
        );
        // A late failure is never taken over the line; a late listing is.
        assert!(!b.answered(FolderMsg {
            seq: 1,
            answer: Err("Not answering".into())
        }));
        go.send(()).unwrap();
        let late = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the late answer");
        assert!(b.answered(late));
        assert_eq!(b.error(), None);
        assert_eq!(b.path(), Some("/Users/max"));
        worker.join().unwrap();
        // An answer in time is posted once.
        let (tx, rx) = std::sync::mpsc::channel();
        answer_in_time(
            Arc::new(Fixed(Ok(json!({"path": "/x", "entries": []})))),
            "mac",
            "Mac Mini",
            &Ask { seq: 3, path: None },
            Duration::from_secs(10),
            &|m| tx.send(m).unwrap(),
        );
        assert_eq!(rx.try_iter().count(), 1);
    }

    /// Rule 1373: Try again asks the failed folder once more as a new ask; it does nothing while an ask runs or when
    /// nothing failed. A late listing for a step taken back keeps Back's way home.
    #[test]
    fn try_again_asks_the_failed_folder_again() {
        let (mut b, first) = Browser::new("mac", "Mini", Some(Os::Mac), 1);
        assert_eq!(b.retry(), None, "Loading: nothing to try again");
        b.answered(FolderMsg {
            seq: first.seq,
            answer: Err(unreachable_line("Mini")),
        });
        let again = b.retry().expect("try again");
        assert_eq!(again, Ask { seq: 2, path: None }, "the home again");
        assert!(b.loading() && b.error().is_none());
        b.answered(FolderMsg {
            seq: 2,
            answer: Ok(listing("/Users/max", &[("proj", true)])),
        });
        assert_eq!(b.retry(), None, "nothing failed");
        // A folder that fails: the same folder is asked again, and Back still leads home.
        let into = b.enter(0).unwrap();
        b.answered(FolderMsg {
            seq: into.seq,
            answer: Err(unreachable_line("Mini")),
        });
        assert!(!b.can_back(), "the step was taken back");
        let again = b.retry().unwrap();
        assert_eq!(again.path.as_deref(), Some("/Users/max/proj"));
        assert!(b.can_back());
        // Its time runs out, then the listing comes late: taken, and Back leads home.
        b.answered(FolderMsg {
            seq: again.seq,
            answer: Err(unreachable_line("Mini")),
        });
        assert!(!b.can_back());
        assert!(b.answered(FolderMsg {
            seq: again.seq,
            answer: Ok(listing("/Users/max/proj", &[])),
        }));
        assert_eq!(b.path(), Some("/Users/max/proj"));
        assert_eq!(b.go_back().unwrap().path.as_deref(), Some("/Users/max"));
    }
}
