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
//!   that for a running exe), the staged file is copied in and its hash checked. Each move is written to
//!   `install.json` before it happens, so a swap cut off by a crash is put back at the next start. On any failure
//!   every file is put back as it was. A file already the staged one is left alone. `staged.json` stays until the
//!   new build's first start. Two processes never swap at once (`install.lock`).
//! - **After the restart** ([`on_start`], [`confirm`]): the first start says "installed"; one that stays up
//!   [`START_GRACE_MS`] (or quits cleanly) confirms the build, and the `.old` files go (a stage made since stays). When the new exe
//!   dies early twice, the next start puts the `.old` files back and starts the old exe instead.
//!
//! **macOS (2026-11-11, rule 1132).** The unit is the whole bundle. An installed copy is a `.app` in an
//! `Applications` folder (`/Applications/Flare.app/Contents/MacOS/Flare`); its root is
//! `~/Library/Application Support/Flare` (or `FLARE_STAGE_ROOT`), which holds `staged/Flare.app`, `staged/staged.json`
//! (the same manifest, its names relative to the bundle: `Contents/MacOS/Flare`), `install.json` and `install.lock`.
//! The stage must be on the bundle's volume ([`NotReady::OtherVolume`]): the swap renames the running bundle aside to
//! `Flare.app.old` (macOS allows that while it runs), renames the staged bundle in, and checks every hash in place.
//! A swap undone moves the new bundle back to `staged/` while the stage is on offer, so it stays on offer; the confirm
//! deletes `Flare.app.old`.
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
/// The staged bundle's name in `staged/` (macOS).
pub const STAGED_APP: &str = "Flare.app";
/// The record of the last swap, in the install root.
pub const MARKER: &str = "install.json";
/// How long a new build must stay up before it counts as good.
pub const START_GRACE_MS: u64 = 20_000;
/// How many early deaths of a new build are allowed; the start after that rolls back.
pub const EARLY_DEATHS: u32 = 2;

/// Where one installed copy lives, and the name of its exe in `current\` (Windows) or in the bundle's
/// `Contents/MacOS` (macOS, where `bundle` is the installed `.app`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    root: PathBuf,
    exe_name: String,
    bundle: Option<PathBuf>,
}

impl Layout {
    pub fn new(root: impl Into<PathBuf>, exe_name: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            exe_name: exe_name.into(),
            bundle: None,
        }
    }

    /// A macOS copy: the installed `bundle` (`/Applications/Flare.app`), the program's name in its
    /// `Contents/MacOS`, and the `root` that holds the stage, `install.json` and the lock.
    pub fn mac(
        root: impl Into<PathBuf>,
        bundle: impl Into<PathBuf>,
        exe_name: impl Into<String>,
    ) -> Self {
        Self {
            root: root.into(),
            exe_name: exe_name.into(),
            bundle: Some(bundle.into()),
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

    /// The macOS layout of a running exe, by its path alone: `<X>.app/Contents/MacOS/<exe>` with the `.app` in a
    /// folder named `Applications` (`/Applications`, `~/Applications`). A bundle anywhere else (`target/mac-release`,
    /// an e2e run's temp bundle) is no installed copy.
    pub fn from_bundle_exe(exe: &Path, root: &Path) -> Option<Self> {
        let app = bundle_of(exe)?;
        let in_applications = app
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|n| n == "Applications");
        if !in_applications {
            return None;
        }
        let exe_name = exe.file_name()?.to_string_lossy().into_owned();
        Some(Self::mac(root, app, exe_name))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn exe_name(&self) -> &str {
        &self.exe_name
    }

    /// The installed `.app` (macOS); None on Windows.
    pub fn bundle(&self) -> Option<&Path> {
        self.bundle.as_deref()
    }

    /// The folder the marker's names are relative to: `<root>\current` (Windows), or the folder that holds the
    /// bundle (macOS: `/Applications`, whose `Flare.app` is the one name a swap moves).
    pub fn current(&self) -> PathBuf {
        match &self.bundle {
            Some(b) => b.parent().map(Path::to_path_buf).unwrap_or_default(),
            None => self.root.join("current"),
        }
    }

    pub fn staged(&self) -> PathBuf {
        self.root.join(STAGED_DIR)
    }

    /// The folder the manifest's names are relative to: `staged\` (Windows), `staged/Flare.app` (macOS).
    pub fn staged_files(&self) -> PathBuf {
        match &self.bundle {
            Some(_) => self.staged().join(STAGED_APP),
            None => self.staged(),
        }
    }

    pub fn manifest(&self) -> PathBuf {
        self.staged().join(MANIFEST)
    }

    pub fn marker(&self) -> PathBuf {
        self.root.join(MARKER)
    }

    /// The exe the copy starts: `current\<exe name>`, or `<bundle>/Contents/MacOS/<exe name>`.
    pub fn exe(&self) -> PathBuf {
        match &self.bundle {
            Some(b) => b.join("Contents").join("MacOS").join(&self.exe_name),
            None => self.current().join(&self.exe_name),
        }
    }

    /// The exe's name in the manifest: `FlareStable.exe`, or `Contents/MacOS/Flare` in a bundle.
    pub fn exe_key(&self) -> String {
        match &self.bundle {
            Some(_) => format!("Contents/MacOS/{}", self.exe_name),
            None => self.exe_name.clone(),
        }
    }

    /// A moved name is the one this copy runs from (the exe, or the whole bundle).
    fn runs_from(&self, moved: &str) -> bool {
        match &self.bundle {
            Some(b) => b.file_name().is_some_and(|n| n.to_string_lossy() == moved),
            None => moved.eq_ignore_ascii_case(&self.exe_name),
        }
    }
}

/// The `<X>.app` of `exe` when its path is `<X>.app/Contents/MacOS/<exe>` (names only, no file is touched).
pub fn bundle_of(exe: &Path) -> Option<PathBuf> {
    exe.file_name()?;
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;
    (macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && app.extension()? == "app")
        .then(|| app.to_path_buf())
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
    /// macOS: the stage is on another volume than the bundle, so the swap could not be a rename: `(stage, app folder)`.
    OtherVolume(String, String),
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
            NotReady::OtherVolume(s, a) => write!(
                f,
                "the stage {s} is on another volume than {a}; set FLARE_STAGE_ROOT to a folder on that volume"
            ),
        }
    }
}

/// A manifest name as a path under `current\`: plain parts only (no root, no drive, no `..`, no `.`, no stream).
pub fn safe_name(name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains(':') || name.contains('\0') {
        return None;
    }
    // One separator for the platform's path parser, so `..` or a root hides behind neither slash.
    #[cfg(windows)]
    let norm = name.replace('/', "\\");
    #[cfg(not(windows))]
    let norm = name.replace('\\', "/");
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
    // Windows names ignore case and either slash: two keys for one file would put the first one's new copy aside
    // as the old one, and the last good build would be lost.
    let mut seen = std::collections::BTreeSet::new();
    for name in m.files.keys() {
        let Some(p) = safe_name(name) else {
            return Err(NotReady::BadName(name.clone()));
        };
        if !seen.insert(p.to_string_lossy().to_lowercase()) {
            return Err(NotReady::BadName(name.clone()));
        }
    }
    let exe_key = layout.exe_key();
    let has_exe = m.files.keys().any(|k| {
        safe_name(k).is_some_and(|p| {
            p.to_string_lossy()
                .replace('\\', "/")
                .eq_ignore_ascii_case(&exe_key)
        })
    });
    if !has_exe {
        return Err(NotReady::NoExe(exe_key));
    }
    Ok(m)
}

/// The staged build, when it is ready: [`read_manifest`] and every file's hash.
/// On macOS the bundle's folder must be on the stage's volume too ([`same_volume`]).
pub fn check(layout: &Layout) -> Result<Manifest, NotReady> {
    let m = read_manifest(layout)?;
    if layout.bundle().is_some() {
        let (stage, app_dir) = (layout.staged(), layout.current());
        same_volume(volume_of(&stage), volume_of(&app_dir), &stage, &app_dir)?;
    }
    hashes_match(&m, &layout.staged_files())?;
    Ok(m)
}

/// Every file the manifest names, under `dir`, has its hash.
fn hashes_match(m: &Manifest, dir: &Path) -> Result<(), NotReady> {
    for (name, want) in &m.files {
        let rel = safe_name(name).ok_or_else(|| NotReady::BadName(name.clone()))?;
        let got = match sha256_file(&dir.join(rel)) {
            Ok(h) => h,
            Err(_) => return Err(NotReady::MissingFile(name.clone())),
        };
        if !got.eq_ignore_ascii_case(want.trim()) {
            return Err(NotReady::Mismatch(name.clone()));
        }
    }
    Ok(())
}

/// The volume (device id) a path is on; None when it cannot be read, and always on Windows (no bundle there).
pub fn volume_of(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(path).ok().map(|m| m.dev()) // sync-ok: one stat, on a worker
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// The macOS swap is two renames, so the stage and the bundle's folder must be on one volume. Two known, different
/// volumes refuse; an unknown one is let through (the rename then fails and everything is put back).
pub fn same_volume(
    stage: Option<u64>,
    app_dir: Option<u64>,
    stage_path: &Path,
    app_dir_path: &Path,
) -> Result<(), NotReady> {
    match (stage, app_dir) {
        (Some(a), Some(b)) if a != b => Err(NotReady::OtherVolume(
            stage_path.display().to_string(),
            app_dir_path.display().to_string(),
        )),
        _ => Ok(()),
    }
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

/// One file the swap replaced or added: its name in `current\`, and the name the old one is put aside under
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
    /// The swap is under way (or was cut off): `moved` lists every file it has touched or is about to touch, so a
    /// start that finds this puts them back.
    Swapping,
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
    /// macOS: the bundle this record is about. A run of another bundle that shares the root leaves it alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

/// The record is this copy's: always on Windows; on macOS when it names this bundle (or none).
fn is_mine(layout: &Layout, m: &Marker) -> bool {
    match (layout.bundle(), &m.app) {
        (Some(b), Some(a)) => Path::new(a) == b,
        _ => true,
    }
}

/// Why a swap failed. Every file was put back as it was.
#[derive(Debug)]
pub enum SwapError {
    NotReady(NotReady),
    Io {
        name: String,
        error: String,
    },
    Hash(String),
    Marker(String),
    /// The last swap is not confirmed yet (its `.old` files are the last good build) or another process swaps.
    Busy(String),
}

impl std::fmt::Display for SwapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SwapError::NotReady(n) => write!(f, "{n}"),
            SwapError::Io { name, error } => write!(f, "{name}: {error}"),
            SwapError::Hash(n) => write!(f, "{n} did not copy whole"),
            SwapError::Marker(e) => write!(f, "install.json: {e}"),
            SwapError::Busy(e) => write!(f, "{e}"),
        }
    }
}

/// The lock a swap, an undo and the start-up guard hold, so two processes on one install (two profiles) never
/// move the same files at once. Windows (no sharing) and macOS (`flock`) let go of it when the process ends, so it
/// never goes stale.
pub struct InstallLock {
    _file: fs::File,
}

/// Takes the install's lock; `None` while another process holds it.
pub fn lock(layout: &Layout) -> Option<InstallLock> {
    let mut o = fs::OpenOptions::new();
    o.create(true).truncate(false).write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        o.share_mode(0);
    }
    // sync-ok: one small file, at start or on a worker
    let f = o.open(layout.root().join(LOCK)).ok()?;
    #[cfg(not(windows))]
    f.try_lock().ok()?;
    Some(InstallLock { _file: f })
}

/// The lock file, in the install root.
pub const LOCK: &str = "install.lock";

/// The first free aside name for `name` in `dir`: `<name>.<tag>`, else `<name>.<tag>1`, `<name>.<tag>2`...
fn aside_name(layout: &Layout, dir: &Path, name: &str, tag: &str) -> String {
    let first = format!("{name}.{tag}");
    if !dir.join(&first).exists() {
        return first;
    }
    // An old leftover that can go makes room for the plain name.
    if remove_leftover(layout, &dir.join(&first)).is_ok() {
        return first;
    }
    (1..)
        .map(|n| format!("{name}.{tag}{n}"))
        .find(|c| !dir.join(c).exists())
        .unwrap_or(first)
}

/// Deletes a leftover: a file, or on macOS a whole bundle (`Flare.app.old`). On Windows a folder is never removed
/// here (a file in use cannot be either; the tests stand one in for the other).
fn remove_leftover(layout: &Layout, p: &Path) -> std::io::Result<()> {
    if layout.bundle().is_some() && p.is_dir() && !p.is_symlink() {
        fs::remove_dir_all(p)
    } else {
        fs::remove_file(p)
    }
}

/// Puts the staged build into `current\`. Checks it first (the hashes, right before the copy), then per file:
/// the move is written to `install.json` (state swapping) before it happens, the old file is renamed aside, the
/// staged one copied in and its hash checked. A file that is already the staged one is left alone. Any failure puts
/// every file back; the error says what failed. Success leaves `install.json` pending. `staged.json` stays: the
/// first start of the new build removes the stage, and a swap the window takes back (an agent started working)
/// keeps it on offer.
///
/// Refused while the last swap is not confirmed: its `.old` files are the last good build, which a second swap
/// would replace with the unconfirmed one.
///
/// macOS: the same, for one name, the bundle: it is renamed aside to `Flare.app.old`, `staged/Flare.app` is renamed
/// in (one volume, [`check`]), and every hash is checked in place.
pub fn swap(layout: &Layout) -> Result<Marker, SwapError> {
    if layout.bundle().is_some() {
        swap_with(layout, &|from, to| fs::rename(from, to))
    } else {
        swap_with(layout, &|from, to| fs::copy(from, to).map(|_| ()))
    }
}

/// [`swap`] with the copy step handed in (the tests make it fail). On macOS it moves the staged bundle in.
pub fn swap_with(
    layout: &Layout,
    copy: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<Marker, SwapError> {
    let _lock =
        lock(layout).ok_or_else(|| SwapError::Busy("another Flare is installing".into()))?;
    let carried = match read_marker(layout) {
        Some(m) if matches!(m.state, MarkerState::Pending | MarkerState::Swapping) => {
            return Err(SwapError::Busy(format!(
                "build {} is not confirmed yet",
                short(&m.commit)
            )));
        }
        Some(m) => m.left,
        None => Vec::new(),
    };
    let m = check(layout).map_err(SwapError::NotReady)?;
    let cur = layout.current();
    let staged = layout.staged();
    let mut marker = Marker {
        commit: m.commit.clone(),
        state: MarkerState::Swapping,
        starts: 0,
        moved: Vec::new(),
        left: carried,
        app: layout.bundle().map(|b| b.to_string_lossy().into_owned()),
    };
    let result = (|| -> Result<(), SwapError> {
        if let Some(bundle) = layout.bundle() {
            return swap_bundle(layout, bundle, &m, &mut marker, copy);
        }
        for (name, want) in &m.files {
            let rel = safe_name(name)
                .ok_or_else(|| SwapError::NotReady(NotReady::BadName(name.clone())))?;
            let target = cur.join(&rel);
            let io = |e: std::io::Error| SwapError::Io {
                name: name.clone(),
                error: e.to_string(),
            };
            let exists = target.is_file();
            if exists && sha256_file(&target).is_ok_and(|h| h.eq_ignore_ascii_case(want.trim())) {
                continue;
            }
            let dir = target.parent().unwrap_or(&cur).to_path_buf();
            fs::create_dir_all(&dir).map_err(io)?;
            let file = rel
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default();
            let aside = target
                .exists()
                .then(|| aside_name(layout, &dir, &file, "old"));
            // The journal first: a swap cut off after this line is put back by the next start.
            marker.moved.push(Moved {
                name: rel.to_string_lossy().into_owned(),
                old: aside
                    .as_ref()
                    .map(|a| rel.with_file_name(a).to_string_lossy().into_owned()),
            });
            write_marker(layout, &marker).map_err(SwapError::Marker)?;
            if let Some(a) = &aside {
                fs::rename(&target, dir.join(a)).map_err(io)?;
            }
            copy(&staged.join(&rel), &target).map_err(io)?;
            let got = sha256_file(&target).map_err(io)?;
            if !got.eq_ignore_ascii_case(want.trim()) {
                return Err(SwapError::Hash(name.clone()));
            }
        }
        Ok(())
    })();
    if let Err(e) = result {
        put_back(layout, &mut marker);
        return Err(e);
    }
    marker.state = MarkerState::Pending;
    if let Err(e) = write_marker(layout, &marker) {
        put_back(layout, &mut marker);
        return Err(SwapError::Marker(e));
    }
    Ok(marker)
}

/// The macOS swap: the journal, the running bundle renamed aside, the staged one moved in, every hash checked there.
fn swap_bundle(
    layout: &Layout,
    bundle: &Path,
    m: &Manifest,
    marker: &mut Marker,
    mover: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<(), SwapError> {
    let dir = layout.current();
    let name = bundle
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    let io = |e: std::io::Error| SwapError::Io {
        name: name.clone(),
        error: e.to_string(),
    };
    let aside = bundle
        .exists()
        .then(|| aside_name(layout, &dir, &name, "old"));
    // The journal first: a swap cut off after this line is put back by the next start.
    marker.moved.push(Moved {
        name: name.clone(),
        old: aside.clone(),
    });
    write_marker(layout, marker).map_err(SwapError::Marker)?;
    if let Some(a) = &aside {
        fs::rename(bundle, dir.join(a)).map_err(io)?;
    }
    mover(&layout.staged_files(), bundle).map_err(io)?;
    match hashes_match(m, bundle) {
        Ok(()) => Ok(()),
        Err(NotReady::Mismatch(n) | NotReady::MissingFile(n) | NotReady::BadName(n)) => {
            Err(SwapError::Hash(n))
        }
        Err(e) => Err(SwapError::NotReady(e)),
    }
}

/// A swap that failed (or was cut off): every moved file goes back. What could not be put back stays in the record
/// (state swapping), so the next start tries again; otherwise only the leftovers stay listed.
fn put_back(layout: &Layout, m: &mut Marker) {
    let undone = undo(layout, &m.moved);
    m.left.extend(undone.left);
    m.moved = undone.stuck;
    if m.moved.is_empty() {
        m.state = MarkerState::Good;
        sweep(layout, m);
    } else {
        log::warn!(
            "{} file(s) of the old build could not be put back yet",
            m.moved.len()
        );
        m.state = MarkerState::Swapping;
        let _ = write_marker(layout, m);
    }
}

/// What [`undo`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Undone {
    /// The aside names of new files that could not be deleted (in use), to delete later.
    pub left: Vec<String>,
    /// Moves that could not be put back.
    pub stuck: Vec<Moved>,
}

/// Puts back what a swap moved, last first: the new file goes (or aside, as a running exe can be renamed, not
/// deleted) and the old one takes its name again. A move whose old file was never put aside (the swap was cut off
/// before the rename) leaves the file where it is.
pub fn undo(layout: &Layout, moved: &[Moved]) -> Undone {
    let cur = layout.current();
    let mut out = Undone::default();
    for mv in moved.iter().rev() {
        let target = cur.join(&mv.name);
        let dir = target.parent().unwrap_or(&cur).to_path_buf();
        if let Some(old) = &mv.old
            && !cur.join(old).exists()
        {
            // Never renamed: the file at the name is still the old one (or the rename is what failed).
            continue;
        }
        if target.exists() && !clear_new(layout, &target) {
            let file = Path::new(&mv.name)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default();
            let bad = aside_name(layout, &dir, &file, "bad");
            if fs::rename(&target, dir.join(&bad)).is_ok() {
                out.left.push(
                    Path::new(&mv.name)
                        .with_file_name(&bad)
                        .to_string_lossy()
                        .into_owned(),
                );
            } else {
                out.stuck.push(mv.clone());
                continue;
            }
        }
        if let Some(old) = &mv.old
            && fs::rename(cur.join(old), &target).is_err()
        {
            out.stuck.push(mv.clone());
        }
    }
    out.stuck.reverse();
    out
}

/// Clears the name the old file gets back: the new file is deleted (Windows: false for a running exe, which is put
/// aside instead). macOS: the new bundle goes back to `staged/` while that stage is on offer, so a swap taken back
/// keeps it on offer; else it is deleted.
fn clear_new(layout: &Layout, target: &Path) -> bool {
    if layout.bundle().is_some() {
        let back = layout.staged_files();
        if layout.manifest().is_file() && !back.exists() && fs::rename(target, &back).is_ok() {
            return true;
        }
        return remove_leftover(layout, target).is_ok();
    }
    fs::remove_file(target).is_ok()
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
    /// A swap was cut off (a crash or a power cut mid-copy): its files were put back. When this exe was one of
    /// them (`restart`), the caller starts the old exe and exits; else it runs on. The stage stays on offer.
    Interrupted { commit: String, restart: bool },
}

/// The last-good guard, run once per start of the copy (after the single-instance check, so a hand-off does not
/// count). It counts the starts of a pending build; past [`EARLY_DEATHS`] it rolls back. A swap that was cut off is
/// put back. While another process holds the install's lock (it is swapping) this does nothing.
pub fn on_start(layout: &Layout) -> Start {
    if read_marker(layout).is_none() {
        return Start::Plain;
    }
    let Some(_lock) = lock(layout) else {
        return Start::Plain;
    };
    let Some(mut m) = read_marker(layout) else {
        return Start::Plain;
    };
    if !is_mine(layout, &m) {
        return Start::Plain;
    }
    match m.state {
        MarkerState::Swapping => {
            let commit = m.commit.clone();
            let exe_moved = m.moved.iter().any(|mv| layout.runs_from(&mv.name));
            put_back(layout, &mut m);
            // Only a whole put-back restarts: one that left files stuck keeps the record, and restarting on it
            // would start this again and again.
            let restart = exe_moved && m.state != MarkerState::Swapping;
            Start::Interrupted { commit, restart }
        }
        MarkerState::Good => {
            sweep(layout, &mut m);
            Start::Plain
        }
        MarkerState::RolledBack => {
            let commit = m.commit.clone();
            // The new build's files and the staged copy go; this is the old build again.
            let _ = fs::remove_file(layout.manifest());
            let _ = fs::remove_dir_all(layout.staged());
            m.state = MarkerState::Good;
            m.moved.clear();
            sweep(layout, &mut m);
            Start::RolledBack { commit }
        }
        MarkerState::Pending => {
            m.starts += 1;
            if m.starts > EARLY_DEATHS {
                let undone = undo(layout, &m.moved);
                m.left.extend(undone.left);
                if !undone.stuck.is_empty() {
                    log::warn!(
                        "{} file(s) of the last good build could not be put back",
                        undone.stuck.len()
                    );
                }
                m.state = MarkerState::RolledBack;
                let commit = m.commit.clone();
                let _ = write_marker(layout, &m);
                return Start::RollBack { commit };
            }
            let _ = write_marker(layout, &m);
            if m.starts == 1 {
                // The staged copy is in place now: the manifest goes first, so it is never offered again even when
                // the folder cannot be removed whole.
                let _ = fs::remove_file(layout.manifest());
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

/// The new build stayed up (or quit cleanly): it is good. The `.old` files go; what is still in use
/// is kept in `install.json` and tried again at the next start. Does nothing unless a swap is pending.
pub fn confirm(layout: &Layout) {
    let Some(_lock) = lock(layout) else {
        return;
    };
    let Some(mut m) = read_marker(layout) else {
        return;
    };
    if m.state != MarkerState::Pending || !is_mine(layout, &m) {
        return;
    }
    m.state = MarkerState::Good;
    let olds: Vec<String> = m.moved.iter().filter_map(|mv| mv.old.clone()).collect();
    m.left.extend(olds);
    m.moved.clear();
    // `staged\` is not touched: the first start removed this build's stage, so what is there now is a newer one.
    sweep(layout, &mut m);
}

/// Deletes the leftovers a good marker lists; removes the marker once none is left.
fn sweep(layout: &Layout, m: &mut Marker) {
    let cur = layout.current();
    m.left.retain(|name| {
        let p = cur.join(name);
        p.exists() && remove_leftover(layout, &p).is_err()
    });
    if m.left.is_empty() {
        let _ = fs::remove_file(layout.marker());
    } else {
        let _ = write_marker(layout, m);
    }
}

/// Takes back a swap this run made and has not restarted from: the old files go back and the record goes, so the
/// old build runs on as if nothing happened. The stage stays on offer.
pub fn take_back(layout: &Layout) {
    let Some(_lock) = lock(layout) else {
        return;
    };
    let Some(mut m) = read_marker(layout) else {
        return;
    };
    if m.state != MarkerState::Pending || m.starts != 0 || !is_mine(layout, &m) {
        return;
    }
    put_back(layout, &mut m);
}

/// A good swap whose new exe could not be started: the old files go back, the record goes, and so does the stage
/// (an exe that will not start must not be offered again).
pub fn abandon(layout: &Layout) {
    take_back(layout);
    let _ = fs::remove_file(layout.manifest());
    if layout.bundle().is_some() {
        // The bundle that would not start went back to `staged/`: it goes too.
        let _ = fs::remove_dir_all(layout.staged());
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

    #[cfg(windows)]
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
            Some(Path::new("claude-plugin").join("hooks").join("a.json"))
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

    #[cfg_attr(
        not(windows),
        ignore = "the staged install names files with backslashes (the Windows dev install); macOS phase 8 of the port plan"
    )]
    #[test]
    fn nothing_staged_is_missing_and_a_good_stage_is_ready() {
        let i = Inst::new();
        assert_eq!(check(&i.layout), Err(NotReady::Missing));
        let m = i.stage_new();
        assert_eq!(check(&i.layout), Ok(m.clone()));
        assert_eq!(m.short_commit(), "abcdef0");
    }

    #[cfg_attr(
        not(windows),
        ignore = "the staged install names files with backslashes (the Windows dev install); macOS phase 8 of the port plan"
    )]
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

    #[cfg_attr(
        not(windows),
        ignore = "the staged install names files with backslashes (the Windows dev install); macOS phase 8 of the port plan"
    )]
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
            i.layout.manifest().exists(),
            "the stage stays until the new build's first start (a swap taken back keeps it on offer)"
        );
    }

    #[cfg_attr(
        not(windows),
        ignore = "the staged install names files with backslashes (the Windows dev install); macOS phase 8 of the port plan"
    )]
    #[test]
    fn a_stage_made_after_the_first_start_survives_the_confirm() {
        let i = Inst::new();
        i.stage_new();
        swap(&i.layout).unwrap();
        on_start(&i.layout);
        i.stage(&[("FlareStable.exe", b"newer exe")]);
        confirm(&i.layout);
        assert!(
            check(&i.layout).is_ok(),
            "the newer stage is still on offer"
        );
    }

    #[test]
    fn two_names_for_one_file_are_refused() {
        let i = Inst::new();
        let mut m = i.stage_new();
        let h = m.files["FlareStable.exe"].clone();
        m.files.insert("flarestable.EXE".into(), h);
        fs::write(i.layout.manifest(), serde_json::to_string(&m).unwrap()).unwrap();
        assert!(matches!(check(&i.layout), Err(NotReady::BadName(_))));
        let mut m = i.stage_new();
        let h = m.files["claude-plugin/hooks.json"].clone();
        m.files.insert("claude-plugin\\hooks.json".into(), h);
        fs::write(i.layout.manifest(), serde_json::to_string(&m).unwrap()).unwrap();
        assert!(matches!(check(&i.layout), Err(NotReady::BadName(_))));
    }

    #[test]
    fn a_file_already_the_staged_one_is_left_alone() {
        let i = Inst::new();
        i.stage(&[
            ("FlareStable.exe", b"new exe"),
            ("flare-mcp.exe", b"old mcp"),
        ]);
        let m = swap(&i.layout).unwrap();
        assert_eq!(m.moved.len(), 1, "{:?}", m.moved);
        assert!(!i.layout.current().join("flare-mcp.exe.old").exists());
        assert_eq!(i.read("flare-mcp.exe"), b"old mcp");
    }

    #[cfg_attr(
        not(windows),
        ignore = "the staged install names files with backslashes (the Windows dev install); macOS phase 8 of the port plan"
    )]
    #[test]
    fn a_second_swap_waits_until_the_first_is_confirmed() {
        let i = Inst::new();
        i.stage_new();
        swap(&i.layout).unwrap();
        i.stage(&[("FlareStable.exe", b"newer exe")]);
        assert!(matches!(swap(&i.layout), Err(SwapError::Busy(_))));
        assert_eq!(i.read("FlareStable.exe"), b"new exe");
        assert_eq!(
            i.read("FlareStable.exe.old"),
            b"old exe",
            "the last good build stays put aside"
        );
        on_start(&i.layout);
        confirm(&i.layout);
        i.stage(&[("FlareStable.exe", b"newer exe")]);
        swap(&i.layout).unwrap();
        assert_eq!(i.read("FlareStable.exe.old"), b"new exe");
    }

    #[cfg_attr(
        not(windows),
        ignore = "the staged install names files with backslashes (the Windows dev install); macOS phase 8 of the port plan"
    )]
    #[test]
    fn leftovers_still_listed_are_kept_by_the_next_swap() {
        let i = Inst::new();
        fs::write(i.layout.current().join("busy.old"), b"x").unwrap();
        let held = Marker {
            commit: "c".into(),
            state: MarkerState::Good,
            starts: 1,
            moved: Vec::new(),
            left: vec!["busy.old".into()],
            app: None,
        };
        write_marker(&i.layout, &held).unwrap();
        i.stage_new();
        let m = swap(&i.layout).unwrap();
        assert_eq!(m.left, vec!["busy.old".to_string()]);
    }

    #[test]
    fn a_swap_cut_off_halfway_is_put_back_at_the_next_start() {
        let i = Inst::new();
        i.stage_new();
        // What a crash after the exe and LICENSE went in, with the plugin file journaled but not yet renamed, leaves.
        fs::rename(
            i.layout.current().join("FlareStable.exe"),
            i.layout.current().join("FlareStable.exe.old"),
        )
        .unwrap();
        fs::write(i.layout.current().join("FlareStable.exe"), b"new e").unwrap();
        fs::write(i.layout.current().join("LICENSE"), b"lic").unwrap();
        let m = Marker {
            commit: "abcdef0123456789".into(),
            state: MarkerState::Swapping,
            starts: 0,
            moved: vec![
                Moved {
                    name: "FlareStable.exe".into(),
                    old: Some("FlareStable.exe.old".into()),
                },
                Moved {
                    name: "LICENSE".into(),
                    old: None,
                },
                Moved {
                    name: "claude-plugin\\hooks.json".into(),
                    old: Some("claude-plugin\\hooks.json.old".into()),
                },
            ],
            left: Vec::new(),
            app: None,
        };
        write_marker(&i.layout, &m).unwrap();
        assert_eq!(
            on_start(&i.layout),
            Start::Interrupted {
                commit: "abcdef0123456789".into(),
                restart: true
            }
        );
        assert_all_old(&i);
        assert_eq!(on_start(&i.layout), Start::Plain);
    }

    #[test]
    fn a_swap_cut_off_before_the_exe_runs_on_without_a_restart() {
        let i = Inst::new();
        i.stage_new();
        fs::write(i.layout.current().join("LICENSE"), b"lic").unwrap();
        let m = Marker {
            commit: "c".into(),
            state: MarkerState::Swapping,
            starts: 0,
            moved: vec![Moved {
                name: "LICENSE".into(),
                old: None,
            }],
            left: Vec::new(),
            app: None,
        };
        write_marker(&i.layout, &m).unwrap();
        assert_eq!(
            on_start(&i.layout),
            Start::Interrupted {
                commit: "c".into(),
                restart: false
            }
        );
        assert_all_old(&i);
    }

    #[cfg(windows)]
    #[test]
    fn while_another_process_holds_the_lock_nothing_moves() {
        let i = Inst::new();
        i.stage_new();
        let held = lock(&i.layout).unwrap();
        assert!(matches!(swap(&i.layout), Err(SwapError::Busy(_))));
        assert_eq!(i.read("FlareStable.exe"), b"old exe");
        drop(held);
        swap(&i.layout).unwrap();
        let held = lock(&i.layout).unwrap();
        assert_eq!(on_start(&i.layout), Start::Plain, "a start leaves it alone");
        assert_eq!(read_marker(&i.layout).unwrap().starts, 0);
        drop(held);
        assert!(matches!(on_start(&i.layout), Start::Installed { .. }));
    }

    #[cfg_attr(
        not(windows),
        ignore = "the staged install names files with backslashes (the Windows dev install); macOS phase 8 of the port plan"
    )]
    #[test]
    fn a_swap_taken_back_keeps_the_stage_and_an_abandoned_one_drops_it() {
        let i = Inst::new();
        i.stage_new();
        swap(&i.layout).unwrap();
        take_back(&i.layout);
        assert_all_old(&i);
        assert!(check(&i.layout).is_ok(), "still on offer");
        swap(&i.layout).unwrap();
        abandon(&i.layout);
        assert_eq!(i.read("FlareStable.exe"), b"old exe");
        assert!(read_marker(&i.layout).is_none());
        assert_eq!(check(&i.layout), Err(NotReady::Missing));
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

    #[cfg_attr(
        not(windows),
        ignore = "the staged install names files with backslashes (the Windows dev install); macOS phase 8 of the port plan"
    )]
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

    #[cfg_attr(
        not(windows),
        ignore = "the staged install names files with backslashes (the Windows dev install); macOS phase 8 of the port plan"
    )]
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

    #[cfg_attr(
        not(windows),
        ignore = "the staged install names files with backslashes (the Windows dev install); macOS phase 8 of the port plan"
    )]
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

    #[cfg_attr(
        not(windows),
        ignore = "the staged install names files with backslashes (the Windows dev install); macOS phase 8 of the port plan"
    )]
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

    #[cfg_attr(
        not(windows),
        ignore = "the staged install names files with backslashes (the Windows dev install); macOS phase 8 of the port plan"
    )]
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

    #[cfg_attr(
        not(windows),
        ignore = "the staged install names files with backslashes (the Windows dev install); macOS phase 8 of the port plan"
    )]
    #[test]
    fn a_restart_that_cannot_start_puts_the_old_build_back() {
        let i = Inst::new();
        i.stage_new();
        swap(&i.layout).unwrap();
        abandon(&i.layout);
        assert_eq!(i.read("FlareStable.exe"), b"old exe");
        assert_eq!(i.read("claude-plugin\\hooks.json"), b"old hooks");
        assert!(!i.layout.current().join("LICENSE").exists());
        assert!(read_marker(&i.layout).is_none());
        assert_eq!(on_start(&i.layout), Start::Plain);
    }

    #[test]
    fn a_confirm_without_a_pending_swap_does_nothing() {
        let i = Inst::new();
        confirm(&i.layout);
        assert!(read_marker(&i.layout).is_none());
        assert_eq!(i.read("FlareStable.exe"), b"old exe");
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
            app: None,
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

/// macOS (rule 1132): the whole bundle is the unit. Temp folders with fake bundles; nothing is launched.
#[cfg(all(test, unix))]
mod mac_tests {
    use super::*;

    /// `<tmp>/Applications/Flare.app` with an old build, and the root `<tmp>/Support/Flare`.
    struct Mac {
        _dir: tempfile::TempDir,
        layout: Layout,
    }

    const OLD: &[(&str, &[u8])] = &[
        ("Contents/MacOS/Flare", b"old exe"),
        ("Contents/MacOS/flare-mcp", b"old mcp"),
        ("Contents/Info.plist", b"old plist"),
    ];
    const NEW: &[(&str, &[u8])] = &[
        ("Contents/MacOS/Flare", b"new exe"),
        ("Contents/MacOS/flare-mcp", b"new mcp"),
        ("Contents/Info.plist", b"new plist"),
        ("Contents/Frameworks/new.dylib", b"new lib"),
    ];

    fn write_bundle(dir: &Path, files: &[(&str, &[u8])]) {
        for (name, bytes) in files {
            let p = dir.join(name);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, bytes).unwrap();
        }
    }

    impl Mac {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let app = dir.path().join("Applications").join("Flare.app");
            write_bundle(&app, OLD);
            let root = dir.path().join("Support").join("Flare");
            fs::create_dir_all(&root).unwrap();
            let layout = Layout::from_bundle_exe(&app.join("Contents/MacOS/Flare"), &root).unwrap();
            Self { _dir: dir, layout }
        }

        fn app(&self) -> PathBuf {
            self.layout.bundle().unwrap().to_path_buf()
        }

        fn old_app(&self) -> PathBuf {
            self.layout.current().join("Flare.app.old")
        }

        /// What `tools/release/mac/stage-install.sh` writes: `staged/Flare.app`, then `staged/staged.json`.
        fn stage(&self, files: &[(&str, &[u8])]) -> Manifest {
            let _ = fs::remove_dir_all(self.layout.staged());
            let b = self.layout.staged_files();
            write_bundle(&b, files);
            let mut map = BTreeMap::new();
            for (name, _) in files {
                map.insert((*name).to_string(), sha256_file(&b.join(name)).unwrap());
            }
            let m = Manifest {
                commit: "fedcba9876543210".into(),
                built_at: "2026-11-11T10:00:00Z".into(),
                files: map,
            };
            fs::write(self.layout.manifest(), serde_json::to_string(&m).unwrap()).unwrap();
            m
        }

        fn read(&self, name: &str) -> Vec<u8> {
            fs::read(self.app().join(name)).unwrap_or_default()
        }

        fn assert_old(&self) {
            for (name, bytes) in OLD {
                assert_eq!(self.read(name), *bytes, "{name}");
            }
            assert!(!self.app().join("Contents/Frameworks/new.dylib").exists());
            assert!(!self.old_app().exists(), "no Flare.app.old is left");
            assert!(read_marker(&self.layout).is_none());
        }

        fn assert_new(&self) {
            for (name, bytes) in NEW {
                assert_eq!(self.read(name), *bytes, "{name}");
            }
        }
    }

    #[test]
    fn an_installed_copy_is_a_bundle_in_an_applications_folder() {
        let root = Path::new("/Users/me/Library/Application Support/Flare");
        let l = Layout::from_bundle_exe(
            Path::new("/Applications/Flare.app/Contents/MacOS/Flare"),
            root,
        )
        .unwrap();
        assert_eq!(l.bundle(), Some(Path::new("/Applications/Flare.app")));
        assert_eq!(l.current(), Path::new("/Applications"));
        assert_eq!(
            l.exe(),
            Path::new("/Applications/Flare.app/Contents/MacOS/Flare")
        );
        assert_eq!(l.exe_key(), "Contents/MacOS/Flare");
        assert_eq!(l.staged_files(), root.join("staged").join("Flare.app"));
        assert_eq!(l.manifest(), root.join("staged").join("staged.json"));
        assert_eq!(l.marker(), root.join("install.json"));
        assert!(
            Layout::from_bundle_exe(
                Path::new("/Users/me/Applications/Flare.app/Contents/MacOS/Flare"),
                root
            )
            .is_some()
        );
        for p in [
            "/Users/me/Flare/target/mac-release/Flare.app/Contents/MacOS/Flare",
            "/Users/me/Flare/target/release/flare",
            "/Applications/Flare.app/Contents/Resources/Flare",
            "/Applications/Flare/Contents/MacOS/Flare",
        ] {
            assert!(Layout::from_bundle_exe(Path::new(p), root).is_none(), "{p}");
        }
    }

    #[test]
    fn nothing_staged_is_missing_a_good_bundle_is_ready_and_a_bad_one_is_not() {
        let i = Mac::new();
        assert_eq!(check(&i.layout), Err(NotReady::Missing));
        let m = i.stage(NEW);
        assert_eq!(check(&i.layout), Ok(m));
        fs::write(i.layout.staged_files().join("Contents/Info.plist"), b"evil").unwrap();
        assert_eq!(
            check(&i.layout),
            Err(NotReady::Mismatch("Contents/Info.plist".into()))
        );
        i.stage(&[("Contents/Info.plist", b"x")]);
        assert_eq!(
            check(&i.layout),
            Err(NotReady::NoExe("Contents/MacOS/Flare".into()))
        );
    }

    #[test]
    fn a_stage_on_another_volume_than_the_bundle_is_refused() {
        let (s, a) = (
            Path::new("/Users/me/stage"),
            Path::new("/Volumes/X/Applications"),
        );
        assert_eq!(
            same_volume(Some(1), Some(2), s, a),
            Err(NotReady::OtherVolume(
                "/Users/me/stage".into(),
                "/Volumes/X/Applications".into()
            ))
        );
        assert_eq!(same_volume(Some(3), Some(3), s, a), Ok(()));
        assert_eq!(same_volume(None, Some(3), s, a), Ok(()));
        // The temp install is on one volume, so a real check passes it.
        let i = Mac::new();
        i.stage(NEW);
        assert!(check(&i.layout).is_ok());
    }

    #[test]
    fn a_swap_renames_the_running_bundle_aside_and_moves_the_stage_in() {
        let i = Mac::new();
        i.stage(NEW);
        let m = swap(&i.layout).unwrap();
        i.assert_new();
        assert_eq!(
            fs::read(i.old_app().join("Contents/MacOS/Flare")).unwrap(),
            b"old exe"
        );
        assert_eq!(m.state, MarkerState::Pending);
        assert_eq!(
            m.moved,
            vec![Moved {
                name: "Flare.app".into(),
                old: Some("Flare.app.old".into())
            }]
        );
        assert_eq!(m.app.as_deref(), Some(i.app().to_str().unwrap()));
        assert_eq!(read_marker(&i.layout), Some(m));
        assert!(
            !i.layout.staged_files().exists(),
            "the stage was moved, not copied"
        );
        assert!(
            i.layout.manifest().exists(),
            "the manifest stays until the first start"
        );
    }

    #[test]
    fn a_bundle_that_lands_with_a_bad_hash_is_rolled_back() {
        let i = Mac::new();
        i.stage(NEW);
        let err = swap_with(&i.layout, &|from, to| {
            fs::rename(from, to)?;
            fs::write(to.join("Contents/MacOS/flare-mcp"), b"half")
        })
        .unwrap_err();
        assert!(
            matches!(err, SwapError::Hash(ref n) if n == "Contents/MacOS/flare-mcp"),
            "{err}"
        );
        i.assert_old();
        assert_eq!(
            check(&i.layout),
            Err(NotReady::Mismatch("Contents/MacOS/flare-mcp".into())),
            "the bad bundle went back to the stage, which is no longer ready"
        );
    }

    #[test]
    fn a_move_that_fails_puts_the_old_bundle_back_and_keeps_the_stage() {
        let i = Mac::new();
        i.stage(NEW);
        let err = swap_with(&i.layout, &|_, _| Err(std::io::Error::other("no room"))).unwrap_err();
        assert!(
            matches!(err, SwapError::Io { ref name, .. } if name == "Flare.app"),
            "{err}"
        );
        i.assert_old();
        assert!(check(&i.layout).is_ok(), "still on offer");
    }

    #[test]
    fn a_swap_cut_off_by_a_crash_is_put_back_at_the_next_start() {
        let i = Mac::new();
        i.stage(NEW);
        // A crash after both renames, before the marker said pending.
        let marker = Marker {
            commit: "fedcba9876543210".into(),
            state: MarkerState::Swapping,
            starts: 0,
            moved: vec![Moved {
                name: "Flare.app".into(),
                old: Some("Flare.app.old".into()),
            }],
            left: Vec::new(),
            app: Some(i.app().to_string_lossy().into_owned()),
        };
        write_marker(&i.layout, &marker).unwrap();
        fs::rename(i.app(), i.old_app()).unwrap();
        fs::rename(i.layout.staged_files(), i.app()).unwrap();
        assert_eq!(
            on_start(&i.layout),
            Start::Interrupted {
                commit: "fedcba9876543210".into(),
                restart: true
            }
        );
        i.assert_old();
        assert!(check(&i.layout).is_ok(), "the stage is on offer again");

        // A crash between the two renames: the bundle is aside, nothing is in its place.
        write_marker(&i.layout, &marker).unwrap();
        fs::rename(i.app(), i.old_app()).unwrap();
        assert!(matches!(
            on_start(&i.layout),
            Start::Interrupted { restart: true, .. }
        ));
        i.assert_old();
        assert!(check(&i.layout).is_ok());
    }

    #[test]
    fn the_first_start_says_installed_and_the_confirm_deletes_the_old_bundle() {
        let i = Mac::new();
        i.stage(NEW);
        swap(&i.layout).unwrap();
        assert_eq!(
            on_start(&i.layout),
            Start::Installed {
                commit: "fedcba9876543210".into()
            }
        );
        assert!(
            !i.layout.staged().exists(),
            "the stage goes at the first start"
        );
        assert!(
            i.old_app().exists(),
            "the last good build stays until the confirm"
        );
        confirm(&i.layout);
        assert!(
            !i.old_app().exists(),
            "Flare.app.old is deleted after the confirm"
        );
        assert!(read_marker(&i.layout).is_none());
        i.assert_new();
        assert_eq!(on_start(&i.layout), Start::Plain);
    }

    #[test]
    fn two_early_deaths_put_the_old_bundle_back() {
        let i = Mac::new();
        i.stage(NEW);
        swap(&i.layout).unwrap();
        assert!(matches!(on_start(&i.layout), Start::Installed { .. }));
        assert!(matches!(
            on_start(&i.layout),
            Start::Trying { starts: 2, .. }
        ));
        assert!(matches!(on_start(&i.layout), Start::RollBack { .. }));
        for (name, bytes) in OLD {
            assert_eq!(i.read(name), *bytes, "{name}");
        }
        assert!(!i.old_app().exists());
        assert!(matches!(on_start(&i.layout), Start::RolledBack { .. }));
        assert_eq!(on_start(&i.layout), Start::Plain);
        i.assert_old();
        assert_eq!(
            check(&i.layout),
            Err(NotReady::Missing),
            "the bad build is not offered again"
        );
    }

    #[test]
    fn a_swap_taken_back_keeps_the_stage_and_an_abandoned_one_drops_it() {
        let i = Mac::new();
        i.stage(NEW);
        swap(&i.layout).unwrap();
        take_back(&i.layout);
        i.assert_old();
        assert!(check(&i.layout).is_ok(), "still on offer");
        swap(&i.layout).unwrap();
        abandon(&i.layout);
        i.assert_old();
        assert_eq!(check(&i.layout), Err(NotReady::Missing));
        assert!(!i.layout.staged().exists());
    }

    #[test]
    fn a_record_of_another_bundle_is_left_alone() {
        let i = Mac::new();
        i.stage(NEW);
        swap(&i.layout).unwrap();
        let other = Layout::mac(
            i.layout.root(),
            i.layout.current().join("Other.app"),
            "Flare",
        );
        assert_eq!(on_start(&other), Start::Plain);
        confirm(&other);
        take_back(&other);
        assert_eq!(read_marker(&i.layout).unwrap().starts, 0);
        assert!(i.old_app().exists());
    }

    #[test]
    fn while_another_process_holds_the_lock_nothing_moves() {
        let i = Mac::new();
        i.stage(NEW);
        let held = lock(&i.layout).unwrap();
        assert!(matches!(swap(&i.layout), Err(SwapError::Busy(_))));
        assert_eq!(i.read("Contents/MacOS/Flare"), b"old exe");
        drop(held);
        swap(&i.layout).unwrap();
        i.assert_new();
    }

    #[test]
    fn a_leftover_old_bundle_from_before_is_cleared_for_the_new_one() {
        let i = Mac::new();
        write_bundle(&i.old_app(), &[("Contents/MacOS/Flare", b"older")]);
        i.stage(NEW);
        let m = swap(&i.layout).unwrap();
        assert_eq!(m.moved[0].old.as_deref(), Some("Flare.app.old"));
        assert_eq!(
            fs::read(i.old_app().join("Contents/MacOS/Flare")).unwrap(),
            b"old exe"
        );
    }
}
