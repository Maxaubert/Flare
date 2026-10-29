//! Install when idle (2026-10-29): the staged build, the swap, the rollback and the last-good guard.
//!
//! An installed copy lives in `<root>\current\` (`%LOCALAPPDATA%\StableFlare\current\FlareStable.exe`, or
//! `%LOCALAPPDATA%\Flare\current\Flare.exe`). `tools/stage-install.ps1` copies a release build into
//! `<root>\staged\` under the names it will have in `current\`, and writes `staged.json` last:
//! `{commit, built_at, files: {name: sha256}}`. Nothing here builds or downloads anything.
//!
//! - **Ready** ([`check`]): the manifest reads, every name is a plain relative path, the running exe is one of
//!   them, and every staged file has the hash the manifest names. Only listed files are ever copied.
//! - **Swap** ([`swap`]): each file in `current\` that is replaced is renamed aside to `<name>.old` (Windows allows
//!   that for a running exe), the staged file is copied in and its hash checked. On any failure every file is put
//!   back as it was. A good swap writes `install.json` in the root and removes `staged.json`.
//! - **After the restart** ([`on_start`], [`confirm`]): the first start says "installed"; one that stays up
//!   [`START_GRACE_MS`] (or quits cleanly) confirms the build, and the `.old` files and `staged\` go. When the new exe
//!   dies early twice, the next start puts the `.old` files back and starts the old exe instead.
//!
//! Every function here touches the disk; the app calls them on a worker or at start, before the window.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The folder beside `current\` that holds a staged build.
pub const STAGED_DIR: &str = "staged";
/// The manifest, written last by the stage script.
pub const MANIFEST: &str = "staged.json";
/// The record of the last swap, in the install root.
pub const MARKER: &str = "install.json";
/// How long a new build must stay up before it counts as good.
pub const START_GRACE_MS: u64 = 20_000;
/// How many early deaths of a new build are allowed; the start after that rolls back.
pub const EARLY_DEATHS: u32 = 2;

/// Where one installed copy lives, and the name of its exe in `current\`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    root: PathBuf,
    exe_name: String,
}

impl Layout {
    pub fn new(root: impl Into<PathBuf>, exe_name: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            exe_name: exe_name.into(),
        }
    }

    /// The layout of a running exe: only one that runs from a folder named `current` is an installed copy.
    /// A build run from `target\release` is not, so it never offers, swaps or guards anything.
    pub fn from_exe(exe: &Path) -> Option<Self> {
        let dir = exe.parent()?;
        let named_current = dir
            .file_name()
            .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("current"));
        if !named_current {
            return None;
        }
        let root = dir.parent()?;
        let exe_name = exe.file_name()?.to_string_lossy().into_owned();
        Some(Self::new(root, exe_name))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn exe_name(&self) -> &str {
        &self.exe_name
    }

    pub fn current(&self) -> PathBuf {
        self.root.join("current")
    }

    pub fn staged(&self) -> PathBuf {
        self.root.join(STAGED_DIR)
    }

    pub fn manifest(&self) -> PathBuf {
        self.staged().join(MANIFEST)
    }

    pub fn marker(&self) -> PathBuf {
        self.root.join(MARKER)
    }

    /// The exe the copy starts: `current\<exe name>`.
    pub fn exe(&self) -> PathBuf {
        self.current().join(&self.exe_name)
    }
}

/// `staged.json`: which build, when it was made, and every file with its SHA-256 (lowercase hex).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub commit: String,
    #[serde(default)]
    pub built_at: String,
    pub files: BTreeMap<String, String>,
}

impl Manifest {
    /// The commit as the chip and the notices say it: the first 7 characters.
    pub fn short_commit(&self) -> String {
        short(&self.commit)
    }

    /// One string per build: a second stage of another build (or the same one again) reads as new.
    pub fn signature(&self) -> String {
        let mut h = Sha256::new();
        h.update(self.commit.as_bytes());
        h.update(self.built_at.as_bytes());
        for (k, v) in &self.files {
            h.update(k.as_bytes());
            h.update(v.as_bytes());
        }
        hex(&h.finalize())
    }
}

/// The first 7 characters of a commit (all of it when shorter).
pub fn short(commit: &str) -> String {
    commit.chars().take(7).collect()
}

/// Why a staged build is not ready.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotReady {
    /// No `staged.json` (nothing staged, or the stage is still copying).
    Missing,
    /// The manifest could not be read or parsed.
    Unreadable(String),
    /// A name that is not a plain relative path inside `current\`.
    BadName(String),
    /// The manifest does not name the exe this copy runs.
    NoExe(String),
    /// A listed file is missing from `staged\`.
    MissingFile(String),
    /// A listed file's hash is not the manifest's.
    Mismatch(String),
}

impl std::fmt::Display for NotReady {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotReady::Missing => write!(f, "nothing is staged"),
            NotReady::Unreadable(e) => write!(f, "staged.json cannot be read: {e}"),
            NotReady::BadName(n) => write!(f, "staged.json names a file outside the install: {n}"),
            NotReady::NoExe(n) => write!(f, "staged.json does not hold {n}"),
            NotReady::MissingFile(n) => write!(f, "{n} is listed but not staged"),
            NotReady::Mismatch(n) => write!(f, "{n} does not have the hash staged.json names"),
        }
    }
}

/// A manifest name as a path under `current\`: plain parts only (no root, no drive, no `..`, no `.`, no stream).
pub fn safe_name(name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains(':') || name.contains('\0') {
        return None;
    }
    let norm = name.replace('/', "\\");
    let p = Path::new(&norm);
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(part) => {
                let s = part.to_string_lossy();
                if s.trim().is_empty() || s.ends_with('.') || s.ends_with(' ') {
                    return None;
                }
                out.push(part);
            }
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// Reads and checks the manifest's shape (the names, the exe), without hashing anything.
pub fn read_manifest(layout: &Layout) -> Result<Manifest, NotReady> {
    let path = layout.manifest();
    // sync-ok: one small file, read on a worker or at start
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(NotReady::Missing),
        Err(e) => return Err(NotReady::Unreadable(e.to_string())),
    };
    let m: Manifest = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| NotReady::Unreadable(e.to_string()))?;
    if m.commit.trim().is_empty() {
        return Err(NotReady::Unreadable("no commit".into()));
    }
    for name in m.files.keys() {
        if safe_name(name).is_none() {
            return Err(NotReady::BadName(name.clone()));
        }
    }
    let has_exe = m
        .files
        .keys()
        .any(|k| k.eq_ignore_ascii_case(layout.exe_name()));
    if !has_exe {
        return Err(NotReady::NoExe(layout.exe_name().to_string()));
    }
    Ok(m)
}

/// The staged build, when it is ready: [`read_manifest`] and every file's hash.
pub fn check(layout: &Layout) -> Result<Manifest, NotReady> {
    let m = read_manifest(layout)?;
    let staged = layout.staged();
    for (name, want) in &m.files {
        let rel = safe_name(name).ok_or_else(|| NotReady::BadName(name.clone()))?;
        let got = match sha256_file(&staged.join(rel)) {
            Ok(h) => h,
            Err(_) => return Err(NotReady::MissingFile(name.clone())),
        };
        if !got.eq_ignore_ascii_case(want.trim()) {
            return Err(NotReady::Mismatch(name.clone()));
        }
    }
    Ok(m)
}

/// The SHA-256 of a file, lowercase hex.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = fs::File::open(path)?; // sync-ok: install files, hashed on a worker or at start
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// One file the swap replaced or added: its name in `current\`, and the name the old one was put aside under
/// (`None` when the file is new).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Moved {
    pub name: String,
    pub old: Option<String>,
}

/// Where the last swap stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkerState {
    /// Swapped, not yet confirmed: the new build has not stayed up long enough.
    Pending,
    /// The new build is good; only the leftovers in `left` remain to be removed.
    Good,
    /// The new build died early too often and the old files are back.
    RolledBack,
}

/// `install.json`: the record of the last swap.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marker {
    pub commit: String,
    pub state: MarkerState,
    /// Starts of the new build since the swap.
    #[serde(default)]
    pub starts: u32,
    pub moved: Vec<Moved>,
    /// Files in `current\` still to delete (in use the last time).
    #[serde(default)]
    pub left: Vec<String>,
}

/// Why a swap failed. Every file was put back as it was.
#[derive(Debug)]
pub enum SwapError {
    NotReady(NotReady),
    Io { name: String, error: String },
    Hash(String),
    Marker(String),
}

impl std::fmt::Display for SwapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SwapError::NotReady(n) => write!(f, "{n}"),
            SwapError::Io { name, error } => write!(f, "{name}: {error}"),
            SwapError::Hash(n) => write!(f, "{n} did not copy whole"),
            SwapError::Marker(e) => write!(f, "install.json: {e}"),
        }
    }
}

/// The first free aside name for `name` in `dir`: `<name>.<tag>`, else `<name>.<tag>1`, `<name>.<tag>2`...
fn aside_name(dir: &Path, name: &str, tag: &str) -> String {
    let first = format!("{name}.{tag}");
    if !dir.join(&first).exists() {
        return first;
    }
    // An old leftover that can go makes room for the plain name.
    if fs::remove_file(dir.join(&first)).is_ok() {
        return first;
    }
    (1..)
        .map(|n| format!("{name}.{tag}{n}"))
        .find(|c| !dir.join(c).exists())
        .unwrap_or(first)
}

/// Puts the staged build into `current\`. Checks it first (the hashes, right before the copy), then per file:
/// the old one is renamed aside, the staged one copied in and its hash checked. Any failure puts every file
/// back; the error says what failed. Success writes `install.json` (pending) and removes `staged.json`.
pub fn swap(layout: &Layout) -> Result<Marker, SwapError> {
    swap_with(layout, &|from, to| fs::copy(from, to).map(|_| ()))
}

/// [`swap`] with the copy step handed in (the tests make it fail).
pub fn swap_with(
    layout: &Layout,
    copy: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<Marker, SwapError> {
    let m = check(layout).map_err(SwapError::NotReady)?;
    let cur = layout.current();
    let staged = layout.staged();
    let mut moved: Vec<Moved> = Vec::new();
    let result = (|| -> Result<(), SwapError> {
        for (name, want) in &m.files {
            let rel = safe_name(name)
                .ok_or_else(|| SwapError::NotReady(NotReady::BadName(name.clone())))?;
            let target = cur.join(&rel);
            let io = |e: std::io::Error| SwapError::Io {
                name: name.clone(),
                error: e.to_string(),
            };
            let dir = target.parent().unwrap_or(&cur).to_path_buf();
            fs::create_dir_all(&dir).map_err(io)?;
            let file = rel
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default();
            let old = if target.exists() {
                let aside = aside_name(&dir, &file, "old");
                fs::rename(&target, dir.join(&aside)).map_err(io)?;
                Some(rel.with_file_name(&aside).to_string_lossy().into_owned())
            } else {
                None
            };
            moved.push(Moved {
                name: rel.to_string_lossy().into_owned(),
                old,
            });
            copy(&staged.join(&rel), &target).map_err(io)?;
            let got = sha256_file(&target).map_err(io)?;
            if !got.eq_ignore_ascii_case(want.trim()) {
                return Err(SwapError::Hash(name.clone()));
            }
        }
        Ok(())
    })();
    if let Err(e) = result {
        undo(layout, &moved);
        return Err(e);
    }
    let marker = Marker {
        commit: m.commit.clone(),
        state: MarkerState::Pending,
        starts: 0,
        moved,
        left: Vec::new(),
    };
    if let Err(e) = write_marker(layout, &marker) {
        undo(layout, &marker.moved);
        return Err(SwapError::Marker(e));
    }
    // The build is in place: it is no longer on offer (the folder itself goes after the restart).
    let _ = fs::remove_file(layout.manifest());
    Ok(marker)
}

/// Puts back what a swap moved, last first: the new file goes aside (a running exe can be renamed, not deleted)
/// and the old one takes its name again. Returns the aside names of the new files, to delete later.
pub fn undo(layout: &Layout, moved: &[Moved]) -> Vec<String> {
    let cur = layout.current();
    let mut left = Vec::new();
    for mv in moved.iter().rev() {
        let target = cur.join(&mv.name);
        if target.exists() && fs::remove_file(&target).is_err() {
            let dir = target.parent().unwrap_or(&cur).to_path_buf();
            let file = Path::new(&mv.name)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default();
            let bad = aside_name(&dir, &file, "bad");
            if fs::rename(&target, dir.join(&bad)).is_ok() {
                left.push(
                    Path::new(&mv.name)
                        .with_file_name(&bad)
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
        if let Some(old) = &mv.old {
            let _ = fs::rename(cur.join(old), &target);
        }
    }
    left
}

/// Reads `install.json`; `None` when there is none or it cannot be read.
pub fn read_marker(layout: &Layout) -> Option<Marker> {
    // sync-ok: one small file, read at start or on a worker
    let text = fs::read_to_string(layout.marker()).ok()?;
    serde_json::from_str(&text).ok()
}

/// Writes `install.json` next to itself first, then renames over it, so a reader never sees half of it.
pub fn write_marker(layout: &Layout, m: &Marker) -> Result<(), String> {
    let text = serde_json::to_string_pretty(m).map_err(|e| e.to_string())?;
    let tmp = layout.root().join(format!("{MARKER}.tmp"));
    fs::write(&tmp, text).map_err(|e| e.to_string())?; // sync-ok: one small file, at start or on a worker
    fs::rename(&tmp, layout.marker()).map_err(|e| e.to_string())
}

/// What a start finds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Start {
    /// Nothing was installed since the last good start.
    Plain,
    /// The first start of a new build: say "Installed <commit>" once.
    Installed { commit: String },
    /// A later start of a new build that has not stayed up yet.
    Trying { commit: String, starts: u32 },
    /// The new build died early too often: the old files are back, and the caller starts the old exe and exits.
    RollBack { commit: String },
    /// The old build runs again after a roll back: say so once.
    RolledBack { commit: String },
}

/// The last-good guard, run once per start of the copy (after the single-instance check, so a hand-off does not
/// count). It counts the starts of a pending build; past [`EARLY_DEATHS`] it rolls back.
pub fn on_start(layout: &Layout) -> Start {
    let Some(mut m) = read_marker(layout) else {
        return Start::Plain;
    };
    match m.state {
        MarkerState::Good => {
            sweep(layout, &mut m);
            Start::Plain
        }
        MarkerState::RolledBack => {
            let commit = m.commit.clone();
            // The new build's files and the staged copy go; this is the old build again.
            let _ = fs::remove_dir_all(layout.staged());
            m.state = MarkerState::Good;
            m.moved.clear();
            sweep(layout, &mut m);
            Start::RolledBack { commit }
        }
        MarkerState::Pending => {
            m.starts += 1;
            if m.starts > EARLY_DEATHS {
                let left = undo(layout, &m.moved);
                m.state = MarkerState::RolledBack;
                m.left.extend(left);
                let commit = m.commit.clone();
                let _ = write_marker(layout, &m);
                return Start::RollBack { commit };
            }
            let _ = write_marker(layout, &m);
            if m.starts == 1 {
                // The staged copy is in place now: the folder can go at once, so it is never offered again.
                let _ = fs::remove_dir_all(layout.staged());
                Start::Installed { commit: m.commit }
            } else {
                Start::Trying {
                    commit: m.commit,
                    starts: m.starts,
                }
            }
        }
    }
}

/// The new build stayed up (or quit cleanly): it is good. The `.old` files and `staged\` go; what is still in use
/// is kept in `install.json` and tried again at the next start. Does nothing unless a swap is pending.
pub fn confirm(layout: &Layout) {
    let Some(mut m) = read_marker(layout) else {
        return;
    };
    if m.state != MarkerState::Pending {
        return;
    }
    m.state = MarkerState::Good;
    let olds: Vec<String> = m.moved.iter().filter_map(|mv| mv.old.clone()).collect();
    m.left.extend(olds);
    m.moved.clear();
    let _ = fs::remove_dir_all(layout.staged());
    sweep(layout, &mut m);
}

/// Deletes the leftovers a good marker lists; removes the marker once none is left.
fn sweep(layout: &Layout, m: &mut Marker) {
    let cur = layout.current();
    m.left.retain(|name| {
        let p = cur.join(name);
        p.exists() && fs::remove_file(&p).is_err()
    });
    if m.left.is_empty() {
        let _ = fs::remove_file(layout.marker());
    } else {
        let _ = write_marker(layout, m);
    }
}

/// "Discard": the staged build goes.
pub fn discard(layout: &Layout) -> std::io::Result<()> {
    match fs::remove_dir_all(layout.staged()) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temp install: `current\` with an exe and a helper, and a staged build of the same names plus a new file.
    struct Inst {
        _dir: tempfile::TempDir,
        layout: Layout,
    }

    impl Inst {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let layout = Layout::new(dir.path().join("StableFlare"), "FlareStable.exe");
            fs::create_dir_all(layout.current()).unwrap();
            fs::write(layout.current().join("FlareStable.exe"), b"old exe").unwrap();
            fs::write(layout.current().join("flare-mcp.exe"), b"old mcp").unwrap();
            fs::create_dir_all(layout.current().join("claude-plugin")).unwrap();
            fs::write(
                layout.current().join("claude-plugin\\hooks.json"),
                b"old hooks",
            )
            .unwrap();
            Self { _dir: dir, layout }
        }

        fn stage(&self, files: &[(&str, &[u8])]) -> Manifest {
            let s = self.layout.staged();
            let _ = fs::remove_dir_all(&s);
            fs::create_dir_all(&s).unwrap();
            let mut map = BTreeMap::new();
            for (name, bytes) in files {
                let p = s.join(name);
                fs::create_dir_all(p.parent().unwrap()).unwrap();
                fs::write(&p, bytes).unwrap();
                map.insert((*name).replace('\\', "/"), sha256_file(&p).unwrap());
            }
            let m = Manifest {
                commit: "abcdef0123456789".into(),
                built_at: "2026-10-29T10:00:00Z".into(),
                files: map,
            };
            fs::write(self.layout.manifest(), serde_json::to_string(&m).unwrap()).unwrap();
            m
        }

        fn stage_new(&self) -> Manifest {
            self.stage(&[
                ("FlareStable.exe", b"new exe"),
                ("flare-mcp.exe", b"new mcp"),
                ("claude-plugin\\hooks.json", b"new hooks"),
                ("LICENSE", b"license"),
            ])
        }

        fn read(&self, name: &str) -> Vec<u8> {
            fs::read(self.layout.current().join(name)).unwrap_or_default()
        }
    }

    #[test]
    fn a_copy_is_installed_only_when_it_runs_from_a_folder_named_current() {
        let l =
            Layout::from_exe(Path::new("C:\\x\\StableFlare\\current\\FlareStable.exe")).unwrap();
        assert_eq!(l.root(), Path::new("C:\\x\\StableFlare"));
        assert_eq!(l.exe_name(), "FlareStable.exe");
        assert_eq!(
            l.manifest(),
            Path::new("C:\\x\\StableFlare\\staged\\staged.json")
        );
        assert!(Layout::from_exe(Path::new("C:\\repo\\target\\release\\flare.exe")).is_none());
        assert!(Layout::from_exe(Path::new("C:\\Flare\\Current\\Flare.exe")).is_some());
    }

    #[test]
    fn only_plain_relative_names_are_safe() {
        assert_eq!(
            safe_name("flare-mcp.exe"),
            Some(PathBuf::from("flare-mcp.exe"))
        );
        assert_eq!(
            safe_name("claude-plugin/hooks/a.json"),
            Some(PathBuf::from("claude-plugin\\hooks\\a.json"))
        );
        for bad in [
            "",
            "..\\x.exe",
            "a/../../b",
            "C:\\x.exe",
            "\\x.exe",
            "a.exe:stream",
            "./a",
            "a.",
            "a ",
        ] {
            assert_eq!(safe_name(bad), None, "{bad}");
        }
    }

    #[test]
    fn nothing_staged_is_missing_and_a_good_stage_is_ready() {
        let i = Inst::new();
        assert_eq!(check(&i.layout), Err(NotReady::Missing));
        let m = i.stage_new();
        assert_eq!(check(&i.layout), Ok(m.clone()));
        assert_eq!(m.short_commit(), "abcdef0");
    }

    #[test]
    fn a_changed_file_a_missing_file_a_bad_name_or_no_exe_is_not_ready() {
        let i = Inst::new();
        i.stage_new();
        fs::write(i.layout.staged().join("flare-mcp.exe"), b"tampered").unwrap();
        assert_eq!(
            check(&i.layout),
            Err(NotReady::Mismatch("flare-mcp.exe".into()))
        );

        i.stage_new();
        fs::remove_file(i.layout.staged().join("LICENSE")).unwrap();
        assert_eq!(
            check(&i.layout),
            Err(NotReady::MissingFile("LICENSE".into()))
        );

        let mut m = i.stage_new();
        m.files.insert("..\\evil.exe".into(), "00".into());
        fs::write(i.layout.manifest(), serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(
            check(&i.layout),
            Err(NotReady::BadName("..\\evil.exe".into()))
        );

        i.stage(&[("flare-mcp.exe", b"x")]);
        assert_eq!(
            check(&i.layout),
            Err(NotReady::NoExe("FlareStable.exe".into()))
        );

        fs::write(i.layout.manifest(), "{not json").unwrap();
        assert!(matches!(check(&i.layout), Err(NotReady::Unreadable(_))));
    }

    #[test]
    fn the_signature_tells_two_stages_apart() {
        let i = Inst::new();
        let a = i.stage_new();
        let b = i.stage(&[("FlareStable.exe", b"newer exe")]);
        assert_ne!(a.signature(), b.signature());
        assert_eq!(a.signature(), a.clone().signature());
    }

    #[test]
    fn a_swap_puts_the_new_files_in_keeps_the_old_aside_and_records_it() {
        let i = Inst::new();
        i.stage_new();
        let m = swap(&i.layout).unwrap();
        assert_eq!(i.read("FlareStable.exe"), b"new exe");
        assert_eq!(i.read("FlareStable.exe.old"), b"old exe");
        assert_eq!(i.read("claude-plugin\\hooks.json"), b"new hooks");
        assert_eq!(i.read("claude-plugin\\hooks.json.old"), b"old hooks");
        assert_eq!(i.read("LICENSE"), b"license");
        assert_eq!(m.state, MarkerState::Pending);
        assert_eq!(read_marker(&i.layout), Some(m.clone()));
        let lic = m.moved.iter().find(|mv| mv.name == "LICENSE").unwrap();
        assert_eq!(lic.old, None, "a new file has nothing put aside");
        assert!(
            !i.layout.manifest().exists(),
            "a swapped build is no longer on offer"
        );
    }

    fn assert_all_old(i: &Inst) {
        assert_eq!(i.read("FlareStable.exe"), b"old exe");
        assert_eq!(i.read("flare-mcp.exe"), b"old mcp");
        assert_eq!(i.read("claude-plugin\\hooks.json"), b"old hooks");
        assert!(!i.layout.current().join("LICENSE").exists());
        assert!(!i.layout.current().join("FlareStable.exe.old").exists());
        assert!(!i.layout.current().join("flare-mcp.exe.old").exists());
        assert!(
            !i.layout
                .current()
                .join("claude-plugin\\hooks.json.old")
                .exists()
        );
        assert!(read_marker(&i.layout).is_none());
        assert!(i.layout.manifest().exists(), "the build stays staged");
    }

    #[test]
    fn a_failed_copy_puts_every_file_back_and_writes_no_marker() {
        let i = Inst::new();
        i.stage_new();
        // The files go in name order (FlareStable.exe, LICENSE, claude-plugin, flare-mcp.exe): the third fails.
        let n = std::cell::Cell::new(0);
        let err = swap_with(&i.layout, &|from, to| {
            n.set(n.get() + 1);
            if n.get() == 3 {
                return Err(std::io::Error::other("disk full"));
            }
            fs::copy(from, to).map(|_| ())
        })
        .unwrap_err();
        assert!(
            matches!(err, SwapError::Io { ref name, .. } if name == "claude-plugin/hooks.json"),
            "{err}"
        );
        assert_all_old(&i);
    }

    #[test]
    fn a_copy_that_lands_wrong_is_caught_by_its_hash_and_undone() {
        let i = Inst::new();
        i.stage_new();
        let err = swap_with(&i.layout, &|_, to| fs::write(to, b"half")).unwrap_err();
        assert!(
            matches!(err, SwapError::Hash(ref n) if n == "FlareStable.exe"),
            "{err}"
        );
        assert_all_old(&i);
    }

    #[cfg(windows)]
    #[test]
    fn a_file_held_open_without_sharing_cannot_be_put_aside_and_nothing_changes() {
        use std::os::windows::fs::OpenOptionsExt;
        let i = Inst::new();
        i.stage_new();
        let held = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(i.layout.current().join("flare-mcp.exe"))
            .unwrap();
        let err = swap(&i.layout).unwrap_err();
        drop(held);
        assert!(
            matches!(err, SwapError::Io { ref name, .. } if name == "flare-mcp.exe"),
            "{err}"
        );
        assert_all_old(&i);
    }

    #[test]
    fn a_tampered_stage_is_refused_before_anything_moves() {
        let i = Inst::new();
        i.stage_new();
        fs::write(i.layout.staged().join("FlareStable.exe"), b"evil").unwrap();
        assert!(matches!(
            swap(&i.layout),
            Err(SwapError::NotReady(NotReady::Mismatch(_)))
        ));
        assert_eq!(i.read("FlareStable.exe"), b"old exe");
        assert!(!i.layout.current().join("FlareStable.exe.old").exists());
    }

    #[test]
    fn a_leftover_old_file_in_use_gets_a_numbered_aside_name() {
        let i = Inst::new();
        fs::create_dir_all(i.layout.current().join("FlareStable.exe.old")).unwrap();
        fs::write(i.layout.current().join("FlareStable.exe.old\\keep"), b"x").unwrap();
        i.stage_new();
        let m = swap(&i.layout).unwrap();
        let exe = m
            .moved
            .iter()
            .find(|mv| mv.name == "FlareStable.exe")
            .unwrap();
        assert_eq!(exe.old.as_deref(), Some("FlareStable.exe.old1"));
        assert_eq!(i.read("FlareStable.exe.old1"), b"old exe");
    }

    #[test]
    fn the_first_start_says_installed_and_a_good_run_cleans_up() {
        let i = Inst::new();
        i.stage_new();
        swap(&i.layout).unwrap();
        assert_eq!(
            on_start(&i.layout),
            Start::Installed {
                commit: "abcdef0123456789".into()
            }
        );
        assert!(
            !i.layout.staged().exists(),
            "the staged folder goes at the first start"
        );
        confirm(&i.layout);
        assert!(!i.layout.current().join("FlareStable.exe.old").exists());
        assert!(
            !i.layout
                .current()
                .join("claude-plugin\\hooks.json.old")
                .exists()
        );
        assert!(read_marker(&i.layout).is_none());
        assert_eq!(on_start(&i.layout), Start::Plain);
        assert_eq!(i.read("FlareStable.exe"), b"new exe");
    }

    #[test]
    fn two_early_deaths_and_the_next_start_rolls_back() {
        let i = Inst::new();
        i.stage_new();
        swap(&i.layout).unwrap();
        assert!(matches!(on_start(&i.layout), Start::Installed { .. }));
        // It died before 20 s: no confirm. Started again by hand, it died again.
        assert_eq!(
            on_start(&i.layout),
            Start::Trying {
                commit: "abcdef0123456789".into(),
                starts: 2
            }
        );
        assert_eq!(
            on_start(&i.layout),
            Start::RollBack {
                commit: "abcdef0123456789".into()
            }
        );
        assert_eq!(i.read("FlareStable.exe"), b"old exe");
        assert_eq!(i.read("flare-mcp.exe"), b"old mcp");
        assert_eq!(i.read("claude-plugin\\hooks.json"), b"old hooks");
        assert!(
            !i.layout.current().join("LICENSE").exists(),
            "a file the build added goes"
        );
        // The old exe starts and says so once.
        assert_eq!(
            on_start(&i.layout),
            Start::RolledBack {
                commit: "abcdef0123456789".into()
            }
        );
        assert_eq!(on_start(&i.layout), Start::Plain);
        assert!(read_marker(&i.layout).is_none());
    }

    #[test]
    fn a_confirm_without_a_pending_swap_does_nothing() {
        let i = Inst::new();
        confirm(&i.layout);
        assert!(read_marker(&i.layout).is_none());
        assert_eq!(i.read("FlareStable.exe"), b"old exe");
    }

    #[test]
    fn discard_removes_the_staged_build_and_is_fine_when_none_is_there() {
        let i = Inst::new();
        i.stage_new();
        discard(&i.layout).unwrap();
        assert!(!i.layout.staged().exists());
        discard(&i.layout).unwrap();
    }

    #[test]
    fn leftovers_that_cannot_go_are_kept_and_retried() {
        let i = Inst::new();
        let m = Marker {
            commit: "c".into(),
            state: MarkerState::Good,
            starts: 1,
            moved: Vec::new(),
            left: vec!["gone.old".into(), "busy.old".into()],
        };
        // A folder with a file in it cannot be removed as a file: it stands in for one in use.
        fs::create_dir_all(i.layout.current().join("busy.old")).unwrap();
        fs::write(i.layout.current().join("busy.old\\x"), b"x").unwrap();
        write_marker(&i.layout, &m).unwrap();
        assert_eq!(on_start(&i.layout), Start::Plain);
        assert_eq!(
            read_marker(&i.layout).unwrap().left,
            vec!["busy.old".to_string()]
        );
        fs::remove_dir_all(i.layout.current().join("busy.old")).unwrap();
        assert_eq!(on_start(&i.layout), Start::Plain);
        assert!(read_marker(&i.layout).is_none());
    }
}
