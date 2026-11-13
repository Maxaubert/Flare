//! Files across a remote link (remote sessions PR 6): a file link on a remote tab fetched from the host, and a dropped
//! file or a pasted image copied to the host.
//!
//! **The host's side.** `remote.fetch` reads one chunk of a file a followed tab's link names (rule 303): the path is
//! resolved on the host from the tab's own folder, as a click there would, and is sent only when the file, as the file
//! system names it (links followed), lies in the tab's folder or the host's temp folder. Anything else (missing, a
//! folder, a device, a share, a link out of those folders, too big) is `not_found` or `too_large`, so a probe learns
//! nothing more. `remote.put` writes one chunk of a viewer's file into the tab's own upload folder on the host
//! (`<temp>/flare-remote/<run>/<tab>`), under a name the host cleans (`put_name`) and makes unique; the last chunk
//! answers the host path and that path quoted for the tab's shell, which the viewer pastes (rule 304). The upload
//! folder goes when the tab closes, and a run's folders left by a crash go a day later. A dropped folder (rule 1213) is
//! made there too (`make_folder`), and its folders and files go inside it by `under`, every step a real folder, within
//! the folder's caps.
//!
//! **The viewer's side.** A fetched file is kept as a copy in this machine's temp folder (`<temp>/flare-remote-copies/
//! <run>/<tab>`), shown in the side panel when it is a picture or a text file and opened like any clicked file
//! otherwise (the click allowlist: a program is never run, rule 14). The copies go when the tab closes.
//!
//! Everything here touches the disk: the control channel's worker and the link's worker call it, never the UI thread
//! (rule 34).

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use flare_ctl::remote::{
    FILE_CHUNK, FILE_CHUNKS_MAX, FILE_MAX, FOLDER_DIRS_MAX, FOLDER_FILES_MAX, FOLDER_MAX,
    chunks_of, is_device_name, put_inner_name, put_name, under_parts,
};
use flare_ctl::{CtlError, code};

fn not_found() -> CtlError {
    CtlError::new(
        code::NOT_FOUND,
        "the host has no such file in the tab's folder or its temp folder",
    )
}

fn bad(msg: impl Into<String>) -> CtlError {
    CtlError::new(code::BAD_PARAMS, msg)
}

/// The run's own name for its folders: fresh at every start, so two runs (or a restarted host whose tab ids begin
/// again at 1) never share one. No secret: a separate token.
pub fn run_name() -> String {
    flare_term::session::new_ctl_token()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(12)
        .collect()
}

/// A tab id as a folder name: only letters, digits, `-` and `_` (Flare's own ids are; anything else is refused).
fn tab_folder(tab: &str) -> Result<&str, CtlError> {
    if tab.is_empty()
        || tab.len() > 64
        || !tab
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(bad("tab is the id of one of the host's tabs"));
    }
    Ok(tab)
}

/// The file system's own name for a path (links followed), without Windows' `\\?\` prefix. None when it names nothing.
fn real(p: &Path) -> Option<PathBuf> {
    // sync-ok: the control channel's worker resolves a fetched path, never the UI thread
    let c = std::fs::canonicalize(p).ok()?;
    let s = c.to_string_lossy();
    Some(match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => c,
    })
}

/// Whether `p` is a network path (`\\server\share`, `\\?\UNC\..`): never read for a link (rule 14).
fn is_share(p: &Path) -> bool {
    let s = p.to_string_lossy();
    s.starts_with(r"\\") || s.starts_with("//")
}

/// `remote.fetch`'s path check (rule 303): `text` as a click on the host reads it (relative to the tab's `folder`, `~`
/// from `home`), then the file as the file system names it, which must be a plain file inside the tab's folder or
/// `temp` (each as the file system names it, so `..` and a link out of them are refused), and no device or share.
/// `not_found` for anything else.
pub fn resolve_fetch(
    folder: &str,
    text: &str,
    home: &str,
    temp: &Path,
) -> Result<PathBuf, CtlError> {
    if text.is_empty() || text.len() > 4096 || text.chars().any(char::is_control) {
        return Err(bad("path is a file's path on the host"));
    }
    let abs = flare_term::path_open::resolve_term_path(folder, text, home).ok_or_else(not_found)?;
    let abs = PathBuf::from(abs);
    if is_share(&abs) {
        return Err(not_found());
    }
    let name = abs
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if name.is_empty() || is_device_name(&name) {
        return Err(not_found());
    }
    let file = real(&abs).ok_or_else(not_found)?;
    if is_share(&file) {
        return Err(not_found());
    }
    // sync-ok: the control channel's worker stats the fetched file, never the UI thread
    if !std::fs::metadata(&file).is_ok_and(|m| m.is_file()) {
        return Err(not_found());
    }
    let inside = |root: &Path| real(root).is_some_and(|r| file.starts_with(&r) && file != r);
    if folder.is_empty() || !(inside(Path::new(folder)) || inside(temp)) {
        return Err(not_found());
    }
    Ok(file)
}

/// One chunk of a fetched file, as `remote.fetch` answers it.
#[derive(Debug, PartialEq, Eq)]
pub struct FetchChunk {
    pub name: String,
    pub size: u64,
    pub chunk: u32,
    pub chunks: u32,
    pub data: Vec<u8>,
}

/// Reads chunk `chunk` of `file` (checked by [`resolve_fetch`]). A file over `FILE_MAX` is `too_large`; a chunk past
/// the end is `bad_params`.
pub fn read_chunk(file: &Path, chunk: u32) -> Result<FetchChunk, CtlError> {
    let gone = |_| not_found();
    // sync-ok: the control channel's worker reads the fetched file, never the UI thread
    let mut f = std::fs::File::open(file).map_err(gone)?;
    let size = f.metadata().map_err(gone)?.len();
    if size > FILE_MAX {
        return Err(CtlError::new(
            code::TOO_LARGE,
            format!(
                "that file is over {} MB: open it on the host",
                FILE_MAX / (1024 * 1024)
            ),
        ));
    }
    let chunks = chunks_of(size);
    if chunk >= chunks {
        return Err(bad(format!("that file has {chunks} chunks")));
    }
    let at = u64::from(chunk) * FILE_CHUNK as u64;
    f.seek(SeekFrom::Start(at)).map_err(gone)?;
    let want = (size - at).min(FILE_CHUNK as u64) as usize;
    let mut data = vec![0u8; want];
    f.read_exact(&mut data).map_err(gone)?;
    Ok(FetchChunk {
        name: file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        size,
        chunk,
        chunks,
        data,
    })
}

/// One file being written by `remote.put`.
struct Part {
    next: u32,
    chunks: u32,
    bytes: u64,
    part: PathBuf,
    done: PathBuf,
}

/// What a `remote.put` chunk answers.
#[derive(Debug, PartialEq, Eq)]
pub struct PutAnswer {
    /// The name the host keeps the file under: the next chunks name it.
    pub name: String,
    /// The host path, once the last chunk is in.
    pub host_path: Option<PathBuf>,
}

/// The lock file in a run's folder. The run holds it locked for as long as it lives, so another Flare's sweep never takes
/// a live run's folder, however long ago it last changed (rule 1191). The system lets go of it when the run ends, a
/// crash too.
const RUN_LOCK: &str = ".flare-run.lock";

/// A run's hold on its own folder: taken when the folder is first made, kept until the run ends.
struct RunLock {
    root: PathBuf,
    held: Mutex<Option<std::fs::File>>, // sync-ok: a type, no call
}

impl RunLock {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            held: Mutex::new(None),
        }
    }

    /// Makes the run's folder and locks its lock file, once. A worker (never the UI thread, rule 34).
    fn hold(&self) -> std::io::Result<()> {
        let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        if held.is_some() {
            return Ok(());
        }
        // sync-ok: a worker makes the run's folder and its lock file, never the UI thread
        std::fs::create_dir_all(&self.root)?;
        // sync-ok: a worker opens the run's lock file, never the UI thread
        let f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.root.join(RUN_LOCK))?;
        f.try_lock().map_err(std::io::Error::other)?;
        *held = Some(f);
        Ok(())
    }
}

/// Whether the run that owns `dir` is still alive: its lock file is locked by someone else. A folder without one (an
/// older Flare's) is judged by its age alone; a lock file that cannot be looked at counts as alive.
fn run_alive(dir: &Path) -> bool {
    // sync-ok: a cleanup thread of its own
    let f = match std::fs::OpenOptions::new()
        .read(true)
        .open(dir.join(RUN_LOCK))
    {
        Ok(f) => f,
        Err(e) => return e.kind() != std::io::ErrorKind::NotFound,
    };
    // A lock taken here is let go when `f` drops, before the folder is removed.
    f.try_lock().is_err()
}

/// The files viewers copy to this host, per tab, in the run's own folder under the temp folder.
pub struct Uploads {
    root: PathBuf,
    lock: RunLock,
    parts: Mutex<HashMap<(String, String), Part>>,
    /// The tabs a file was copied to: only their folders are removed when they close.
    made: Mutex<std::collections::HashSet<String>>,
    /// The dropped folders being copied (rule 1213), by tab and the name the host kept for the top folder: what they
    /// hold so far, against the caps.
    trees: Mutex<HashMap<(String, String), Tree>>,
}

/// What one dropped folder holds on the host so far (rule 1213).
#[derive(Default)]
struct Tree {
    dirs: usize,
    files: usize,
    bytes: u64,
}

fn too_large(msg: impl Into<String>) -> CtlError {
    CtlError::new(code::TOO_LARGE, msg)
}

/// The folder `parts` names below `dir`, when every step is a real folder (no link, no junction: a link the host
/// finds there is refused, rule 1213). None otherwise.
fn real_dir_below(dir: &Path, parts: &[&str]) -> Option<PathBuf> {
    let mut at = dir.to_path_buf();
    for p in parts {
        at.push(p);
        // sync-ok: the control channel's worker checks a folder of the upload, never the UI thread
        let m = std::fs::symlink_metadata(&at).ok()?;
        if !m.is_dir() || m.file_type().is_symlink() || is_reparse(&m) {
            return None;
        }
    }
    Some(at)
}

/// Whether a file system entry is a reparse point (a junction or a link on Windows); never on other systems.
pub fn is_reparse(m: &std::fs::Metadata) -> bool {
    // sync-ok: a type, no call
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        m.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        let _ = m;
        false
    }
}

/// The suffix of a file still being written.
const PART: &str = ".flare-part";

impl Uploads {
    /// Uploads under `root` (the run's folder: `<temp>/flare-remote/<run>`). Nothing is made until a file comes.
    pub fn new(root: PathBuf) -> Self {
        Self {
            lock: RunLock::new(root.clone()),
            root,
            parts: Mutex::new(HashMap::new()),
            made: Mutex::new(std::collections::HashSet::new()),
            trees: Mutex::new(HashMap::new()),
        }
    }

    fn trees(&self) -> MutexGuard<'_, HashMap<(String, String), Tree>> {
        self.trees.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The folder `under` names in the tab's upload folder (rule 1213): clean names (`under_parts`), the first one a
    /// dropped folder this run made for the tab, every step a real folder. Also the top folder's key, for the caps.
    fn under_dir(&self, tab: &str, under: &str) -> Result<(PathBuf, (String, String)), CtlError> {
        let parts = under_parts(under)
            .ok_or_else(|| bad("under is a folder of clean names joined by /"))?;
        let key = (tab.to_string(), parts[0].to_string());
        if !self.trees().contains_key(&key) {
            return Err(bad(
                "under starts with a folder this link made: send folder first",
            ));
        }
        let dir = real_dir_below(&self.folder(tab), &parts)
            .ok_or_else(|| bad("under is not a folder of that copy"))?;
        Ok((dir, key))
    }

    /// Makes a folder of a dropped folder (rule 1213). Without `under` it is the dropped folder itself, made in the
    /// tab's own upload folder under a clean name made unique there (as a file's is), and its answer carries its host
    /// path; with `under` it is a folder inside it (made once, kept when it is there already). At most
    /// `FOLDER_DIRS_MAX` folders in one dropped folder.
    pub fn make_folder(
        &self,
        tab: &str,
        name: &str,
        under: Option<&str>,
    ) -> Result<PutAnswer, CtlError> {
        let tab = tab_folder(tab)?;
        let io = |e: std::io::Error| {
            CtlError::new(
                code::UNAVAILABLE,
                format!("the folder could not be made on the host: {e}"),
            )
        };
        let Some(under) = under else {
            self.made
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(tab.to_string());
            self.lock.hold().map_err(io)?;
            let dir = self.folder(tab);
            // sync-ok: the control channel's worker makes the tab's upload folder, never the UI thread
            std::fs::create_dir_all(&dir).map_err(io)?;
            let parts = self.lk();
            let kept = unique_name(&dir, &put_name(name), |n| {
                parts.contains_key(&(tab.to_string(), n.to_string()))
                    || self.trees().contains_key(&(tab.to_string(), n.to_string()))
            });
            drop(parts);
            let at = dir.join(&kept);
            std::fs::create_dir(&at).map_err(io)?; // sync-ok: the control channel's worker
            self.trees().insert(
                (tab.to_string(), kept.clone()),
                Tree {
                    dirs: 1,
                    ..Tree::default()
                },
            );
            return Ok(PutAnswer {
                name: kept,
                host_path: Some(at),
            });
        };
        let (dir, key) = self.under_dir(tab, under)?;
        let kept = put_inner_name(name);
        let at = dir.join(&kept);
        {
            let mut trees = self.trees();
            let t = trees.entry(key).or_default();
            if t.dirs >= FOLDER_DIRS_MAX {
                return Err(too_large(format!(
                    "a folder holds at most {FOLDER_DIRS_MAX} folders here"
                )));
            }
            t.dirs += 1;
        }
        // sync-ok: the control channel's worker checks the folder to make, never the UI thread
        match std::fs::symlink_metadata(&at) {
            Ok(m) if m.is_dir() && !m.file_type().is_symlink() && !is_reparse(&m) => {}
            Ok(_) => return Err(bad("a file of that name is there already")),
            Err(_) => std::fs::create_dir(&at).map_err(io)?, // sync-ok: the control channel's worker
        }
        Ok(PutAnswer {
            name: kept,
            host_path: Some(at),
        })
    }

    fn lk(&self) -> MutexGuard<'_, HashMap<(String, String), Part>> {
        self.parts.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A tab's upload folder.
    pub fn folder(&self, tab: &str) -> PathBuf {
        self.root.join(tab)
    }

    /// Takes chunk `chunk` of `chunks` of a file. Chunk 0 names the file (`name`, cleaned and made unique in the tab's
    /// folder); the next chunks name it as the answer to chunk 0 did, in order. The last chunk puts the file in place
    /// and answers its path. Over `FILE_MAX` in all, or a chunk out of turn, drops the file and is `bad_params` or
    /// `too_large`. The app calls [`Uploads::put_in`]; this is its form for a file of the tab's own folder (tests).
    #[cfg(test)]
    pub fn put(
        &self,
        tab: &str,
        name: &str,
        chunk: u32,
        chunks: u32,
        data: &[u8],
    ) -> Result<PutAnswer, CtlError> {
        self.put_in(tab, None, name, chunk, chunks, data)
    }

    /// [`Uploads::put`], into the folder `under` names inside a dropped folder this link made (rule 1213): the name
    /// is cleaned as an inner name (a leading dot kept), and the dropped folder's caps hold, `FOLDER_FILES_MAX` files
    /// and `FOLDER_MAX` bytes in all. Past either the file is dropped and the answer is `too_large`.
    pub fn put_in(
        &self,
        tab: &str,
        under: Option<&str>,
        name: &str,
        chunk: u32,
        chunks: u32,
        data: &[u8],
    ) -> Result<PutAnswer, CtlError> {
        let tab = tab_folder(tab)?;
        let (dir, tree) = match under {
            Some(u) => {
                let (d, k) = self.under_dir(tab, u)?;
                (Some(d), Some(k))
            }
            None => (None, None),
        };
        // A file inside a dropped folder is keyed by its place there, so two of one name in two folders never meet.
        let keyed = |n: &str| match under {
            Some(u) => format!("{u}/{n}"),
            None => n.to_string(),
        };
        if let Some(k) = &tree {
            let mut trees = self.trees();
            let t = trees.entry(k.clone()).or_default();
            if chunk == 0 && t.files >= FOLDER_FILES_MAX {
                return Err(too_large(format!(
                    "a folder holds at most {FOLDER_FILES_MAX} files here"
                )));
            }
            if t.bytes + data.len() as u64 > FOLDER_MAX {
                drop(trees);
                if chunk > 0
                    && let Some(p) = self.lk().remove(&(tab.to_string(), keyed(name)))
                {
                    let _ = std::fs::remove_file(&p.part); // sync-ok: the control channel's worker
                }
                return Err(too_large(format!(
                    "a folder is at most {} MB here",
                    FOLDER_MAX / (1024 * 1024)
                )));
            }
            t.bytes += data.len() as u64;
            if chunk == 0 {
                t.files += 1;
            }
        }
        if chunks == 0 || chunks > FILE_CHUNKS_MAX {
            return Err(CtlError::new(
                code::TOO_LARGE,
                format!("a file is at most {} MB here", FILE_MAX / (1024 * 1024)),
            ));
        }
        if chunk >= chunks {
            return Err(bad("chunk is less than chunks"));
        }
        if data.len() > FILE_CHUNK {
            return Err(bad(format!("a chunk is at most {} KB", FILE_CHUNK / 1024)));
        }
        let io = |e: std::io::Error| {
            CtlError::new(
                code::UNAVAILABLE,
                format!("the file could not be written on the host: {e}"),
            )
        };
        let inner = dir.is_some();
        let dir = dir.unwrap_or_else(|| self.folder(tab));
        let mut parts = self.lk();
        let key;
        if chunk == 0 {
            self.made
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(tab.to_string());
            self.lock.hold().map_err(io)?;
            // sync-ok: the control channel's worker makes the tab's upload folder, never the UI thread
            std::fs::create_dir_all(&dir).map_err(io)?;
            let clean = if inner {
                put_inner_name(name)
            } else {
                put_name(name)
            };
            let kept = unique_name(&dir, &clean, |n| {
                parts.contains_key(&(tab.to_string(), keyed(n)))
                    || (!inner && self.trees().contains_key(&(tab.to_string(), n.to_string())))
            });
            let done = dir.join(&kept);
            let part = dir.join(format!("{kept}{PART}"));
            // sync-ok: the control channel's worker writes the uploaded chunk, never the UI thread
            let mut f = std::fs::File::create(&part).map_err(io)?;
            f.write_all(data).map_err(io)?;
            key = (tab.to_string(), keyed(&kept));
            parts.insert(
                key.clone(),
                Part {
                    next: 1,
                    chunks,
                    bytes: data.len() as u64,
                    part,
                    done,
                },
            );
        } else {
            key = (tab.to_string(), keyed(name));
            let p = parts
                .get_mut(&key)
                .ok_or_else(|| bad("no file of that name is being copied to that tab"))?;
            if p.next != chunk || p.chunks != chunks {
                let p = parts.remove(&key).expect("checked above");
                let _ = std::fs::remove_file(&p.part); // sync-ok: the control channel's worker
                return Err(bad("a chunk came out of turn: send the file again"));
            }
            p.bytes += data.len() as u64;
            if p.bytes > FILE_MAX {
                let p = parts.remove(&key).expect("checked above");
                let _ = std::fs::remove_file(&p.part); // sync-ok: the control channel's worker
                return Err(CtlError::new(code::TOO_LARGE, "that file is too big"));
            }
            // sync-ok: the control channel's worker appends the uploaded chunk, never the UI thread
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&p.part)
                .map_err(io)?;
            f.write_all(data).map_err(io)?;
            p.next += 1;
        }
        // The answer names the file as the host kept it in its own folder.
        let plain = |k: &str| k.rsplit('/').next().unwrap_or(k).to_string();
        let p = parts.get(&key).expect("kept above");
        if p.next < p.chunks {
            return Ok(PutAnswer {
                name: plain(&key.1),
                host_path: None,
            });
        }
        let p = parts.remove(&key).expect("kept above");
        // sync-ok: the control channel's worker puts the finished file in place, never the UI thread
        std::fs::rename(&p.part, &p.done).map_err(io)?;
        Ok(PutAnswer {
            name: plain(&key.1),
            host_path: Some(p.done),
        })
    }

    /// The tab closed: its unfinished files are dropped, and its folder is handed back to be removed off the UI
    /// thread.
    pub fn forget(&self, tab: &str) -> Option<PathBuf> {
        self.lk().retain(|(t, _), _| t != tab);
        self.trees().retain(|(t, _), _| t != tab);
        let had = self
            .made
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(tab);
        had.then(|| tab_folder(tab).ok().map(|t| self.folder(t)))
            .flatten()
    }

    /// The run's folder (tests).
    #[cfg(test)]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// Rule 1215: a copied file that ran as a program on the viewer is made runnable on a Mac or Linux host: every reader
/// of it may also run it (`0o644` becomes `0o755`). A Windows host has no such bit: nothing changes. The control
/// channel's worker, once the file is in place.
pub fn make_runnable(file: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // sync-ok: the control channel's worker, on the file it just put in place
        let mode = std::fs::metadata(file)?.permissions().mode();
        let runnable = mode | ((mode & 0o444) >> 2);
        if runnable != mode {
            // sync-ok: the control channel's worker, on the file it just put in place
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(runnable))?;
        }
    }
    #[cfg(not(unix))]
    let _ = file;
    Ok(())
}

/// `name`, or `stem-2.ext`, `stem-3.ext` and so on: the first that is neither on disk in `dir` nor `taken`.
fn unique_name(dir: &Path, name: &str, taken: impl Fn(&str) -> bool) -> String {
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    let free = |n: &str| {
        // sync-ok: the control channel's worker looks for a free name, never the UI thread
        !taken(n) && !dir.join(n).exists() && !dir.join(format!("{n}{PART}")).exists()
    };
    if free(name) {
        return name.to_string();
    }
    (2..)
        .map(|i| format!("{stem}-{i}{ext}"))
        .find(|n| free(n))
        .expect("an unbounded search finds a free name")
}

/// Removes a folder and everything in it, on a thread of its own (never the UI thread, rule 34).
pub fn remove_later(dir: PathBuf) {
    let _ = std::thread::Builder::new()
        .name("flare-remote-files-cleanup".into())
        .spawn(move || {
            let _ = std::fs::remove_dir_all(&dir); // sync-ok: a cleanup thread of its own
        });
}

/// The folders of earlier runs under `parent` that are older than `age` and whose run is gone (left by a crash):
/// removed. A run still alive keeps its folder, however old (rule 1191). A cleanup thread.
pub fn sweep_old(parent: &Path, age: Duration) {
    // sync-ok: a cleanup thread of its own lists the earlier runs
    let Ok(rd) = std::fs::read_dir(parent) else {
        return;
    };
    let now = SystemTime::now();
    for e in rd.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|d| d > age);
        if old && e.path().is_dir() && !run_alive(&e.path()) {
            let _ = std::fs::remove_dir_all(e.path()); // sync-ok: a cleanup thread of its own
        }
    }
}

/// How long a run's leftover folders are kept before a later run removes them.
pub const LEFTOVER_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// Starts the sweep of leftover folders under the temp folder's `flare-remote` and `flare-remote-copies` on a thread.
pub fn sweep_leftovers_later() {
    let _ = std::thread::Builder::new()
        .name("flare-remote-files-sweep".into())
        .spawn(|| {
            let temp = std::env::temp_dir();
            sweep_old(&temp.join("flare-remote"), LEFTOVER_AGE);
            sweep_old(&temp.join("flare-remote-copies"), LEFTOVER_AGE);
        });
}

/// The viewer's copies of fetched files: `<temp>/flare-remote-copies/<run>/<tab>/<name>`.
pub struct Copies {
    root: PathBuf,
    lock: RunLock,
    /// The tabs a copy was kept for: only their folders are removed when they close.
    used: Mutex<std::collections::HashSet<String>>,
}

impl Copies {
    pub fn new(root: PathBuf) -> Self {
        Self {
            lock: RunLock::new(root.clone()),
            root,
            used: Mutex::new(std::collections::HashSet::new()),
        }
    }

    /// A closed tab's folder of copies, when it has one (removed off the UI thread by the caller).
    pub fn forget(&self, tab: &str) -> Option<PathBuf> {
        let had = self
            .used
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(tab);
        had.then(|| self.folder(tab)).flatten()
    }

    /// The real one, in this run's folder.
    pub fn in_temp() -> Self {
        Self::new(
            std::env::temp_dir()
                .join("flare-remote-copies")
                .join(run_name()),
        )
    }

    /// A tab's folder of copies (`None` for a tab id that is no folder name).
    pub fn folder(&self, tab: &str) -> Option<PathBuf> {
        tab_folder(tab).ok().map(|t| self.root.join(t))
    }

    /// Writes a fetched file as a copy in the tab's folder, under the host's name cleaned as an upload's is (rule
    /// 304's `put_name`), replacing an earlier copy of that name: the folder and the plain name of the copy. A worker.
    pub fn save(&self, tab: &str, name: &str, data: &[u8]) -> std::io::Result<(PathBuf, String)> {
        let dir = self
            .folder(tab)
            .ok_or_else(|| std::io::Error::other("not a tab id"))?;
        self.used
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(tab.to_string());
        self.lock.hold()?;
        std::fs::create_dir_all(&dir)?; // sync-ok: a worker writes the fetched copy, never the UI thread
        let name = put_name(name);
        let tmp = dir.join(format!("{name}{PART}"));
        std::fs::write(&tmp, data)?; // sync-ok: a worker writes the fetched copy, never the UI thread
        std::fs::rename(&tmp, dir.join(&name))?; // sync-ok: a worker, the copy put in place whole
        Ok((dir, name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    struct Dirs {
        _all: tempfile::TempDir,
        folder: PathBuf,
        temp: PathBuf,
        outside: PathBuf,
    }

    fn dirs() -> Dirs {
        let all = tempfile::tempdir().unwrap();
        let root = real(all.path()).unwrap();
        let folder = root.join("proj");
        let temp = root.join("tmp");
        let outside = root.join("secret");
        for d in [&folder, &temp, &outside] {
            std::fs::create_dir_all(d).unwrap();
        }
        write(&folder.join("notes.md"), b"# notes");
        write(&folder.join("src").join("main.rs"), b"fn main() {}");
        write(&temp.join("shot.png"), b"\x89PNG");
        write(&outside.join("key.txt"), b"secret");
        Dirs {
            _all: all,
            folder,
            temp,
            outside,
        }
    }

    fn s(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    fn code_of(r: Result<PathBuf, CtlError>) -> String {
        r.map(|p| format!("ok {}", p.display()))
            .unwrap_or_else(|e| e.code)
    }

    /// Rule 303: a fetched path is read from the tab's folder as a click there reads it, and only a plain file in the
    /// tab's folder or the host's temp folder is sent.
    #[test]
    fn a_fetch_reads_the_tabs_folder_and_the_temp_folder_and_nothing_else() {
        let d = dirs();
        let (folder, temp) = (s(&d.folder), d.temp.clone());
        let ok = |text: &str| resolve_fetch(&folder, text, "", &temp).unwrap();
        assert_eq!(ok("notes.md"), d.folder.join("notes.md"));
        assert_eq!(ok("src/main.rs"), d.folder.join("src").join("main.rs"));
        assert_eq!(
            ok(&s(&d.folder.join("notes.md"))),
            d.folder.join("notes.md")
        );
        assert_eq!(ok(&s(&d.temp.join("shot.png"))), d.temp.join("shot.png"));
        let refused = |text: &str| code_of(resolve_fetch(&folder, text, "", &temp));
        // Outside both, by an absolute path or by `..`.
        assert_eq!(refused(&s(&d.outside.join("key.txt"))), code::NOT_FOUND);
        assert_eq!(refused("../secret/key.txt"), code::NOT_FOUND);
        assert_eq!(refused("src/../../secret/key.txt"), code::NOT_FOUND);
        // Missing, a folder, the folder itself.
        assert_eq!(refused("gone.txt"), code::NOT_FOUND);
        assert_eq!(refused("src"), code::NOT_FOUND);
        assert_eq!(refused(&folder), code::NOT_FOUND);
        // Not a path at all.
        assert_eq!(refused(""), code::BAD_PARAMS);
        assert_eq!(refused("a\nb"), code::BAD_PARAMS);
        // No folder for the tab: nothing relative, and nothing outside temp.
        assert_eq!(
            code_of(resolve_fetch("", &s(&d.folder.join("notes.md")), "", &temp)),
            code::NOT_FOUND
        );
    }

    /// Rule 303: a link inside the tab's folder that points out of it is read as the file system names it, and refused.
    #[test]
    fn a_link_out_of_the_tabs_folder_is_refused() {
        let d = dirs();
        let link = d.folder.join("escape.txt");
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(d.outside.join("key.txt"), &link).is_ok();
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(d.outside.join("key.txt"), &link).is_ok();
        if !made {
            // Windows without the right to make links: nothing to test here.
            return;
        }
        assert_eq!(
            code_of(resolve_fetch(&s(&d.folder), "escape.txt", "", &d.temp)),
            code::NOT_FOUND
        );
    }

    /// Rule 303: Windows' device names and network paths are never read, whatever folder they seem to be in.
    #[test]
    fn device_names_and_shares_are_never_fetched() {
        let d = dirs();
        let folder = s(&d.folder);
        for text in ["NUL", "con.txt", "COM1", "aux.log", "src/LPT1.txt"] {
            assert_eq!(
                code_of(resolve_fetch(&folder, text, "", &d.temp)),
                code::NOT_FOUND,
                "{text}"
            );
        }
        for text in [r"\\server\share\x.txt", "//server/share/x.txt"] {
            assert_eq!(
                code_of(resolve_fetch(&folder, text, "", &d.temp)),
                code::NOT_FOUND,
                "{text}"
            );
        }
    }

    #[test]
    fn a_fetched_file_is_read_in_chunks_that_join_up_and_a_big_one_is_refused() {
        let d = dirs();
        let big: Vec<u8> = (0..(FILE_CHUNK * 2 + 10))
            .map(|i| (i % 251) as u8)
            .collect();
        let p = d.folder.join("big.bin");
        write(&p, &big);
        let first = read_chunk(&p, 0).unwrap();
        assert_eq!(
            (first.name.as_str(), first.size, first.chunks),
            ("big.bin", big.len() as u64, 3)
        );
        let mut all = first.data;
        for c in 1..3 {
            all.extend(read_chunk(&p, c).unwrap().data);
        }
        assert_eq!(all, big);
        assert_eq!(read_chunk(&p, 3).unwrap_err().code, code::BAD_PARAMS);
        let empty = d.folder.join("empty.txt");
        write(&empty, b"");
        let e = read_chunk(&empty, 0).unwrap();
        assert_eq!((e.chunks, e.data.len()), (1, 0));
        let huge = d.folder.join("huge.bin");
        let f = std::fs::File::create(&huge).unwrap();
        f.set_len(FILE_MAX + 1).unwrap();
        drop(f);
        assert_eq!(read_chunk(&huge, 0).unwrap_err().code, code::TOO_LARGE);
    }

    /// Rule 304: an upload is written into the tab's own folder under a clean, unique name, whole once its last chunk is
    /// in; a chunk out of turn drops it.
    #[test]
    fn an_upload_lands_whole_in_the_tabs_folder_under_a_clean_unique_name() {
        let root = tempfile::tempdir().unwrap();
        let up = Uploads::new(root.path().join("run"));
        let one = up.put("t1", "shot.png", 0, 1, b"png").unwrap();
        let path = one.host_path.clone().unwrap();
        assert_eq!(path, up.root().join("t1").join("shot.png"));
        assert_eq!(std::fs::read(&path).unwrap(), b"png");
        // The same name again is a new file, never the old one overwritten.
        let two = up.put("t1", "shot.png", 0, 1, b"two").unwrap();
        assert_eq!(two.name, "shot-2.png");
        assert_eq!(std::fs::read(&path).unwrap(), b"png");
        // A name with a folder in it and characters a shell acts on stays in the tab's folder, cleaned.
        let a = up.put("t1", "..\\..\\evil%.txt", 0, 2, b"ab").unwrap();
        assert_eq!((a.name.as_str(), a.host_path.as_ref()), ("evil_.txt", None));
        assert!(
            !up.root().join("t1").join("evil_.txt").exists(),
            "not in place before its last chunk"
        );
        let b = up.put("t1", &a.name, 1, 2, b"cd").unwrap();
        assert_eq!(std::fs::read(b.host_path.unwrap()).unwrap(), b"abcd");
        // Out of turn: the file is dropped and must be sent again.
        let c = up.put("t1", "x.bin", 0, 3, b"1").unwrap();
        assert_eq!(
            up.put("t1", &c.name, 2, 3, b"3").unwrap_err().code,
            code::BAD_PARAMS
        );
        assert_eq!(
            up.put("t1", &c.name, 1, 3, b"2").unwrap_err().code,
            code::BAD_PARAMS
        );
        assert!(!up.root().join("t1").join("x.bin.flare-part").exists());
        // Caps and bad tabs.
        assert_eq!(
            up.put("t1", "y", 0, FILE_CHUNKS_MAX + 1, b"")
                .unwrap_err()
                .code,
            code::TOO_LARGE
        );
        assert_eq!(
            up.put("t1", "y", 0, 1, &vec![0; FILE_CHUNK + 1])
                .unwrap_err()
                .code,
            code::BAD_PARAMS
        );
        assert_eq!(
            up.put("../t1", "y", 0, 1, b"").unwrap_err().code,
            code::BAD_PARAMS
        );
        assert_eq!(
            up.put("t1", "y", 1, 1, b"").unwrap_err().code,
            code::BAD_PARAMS
        );
    }

    /// Rule 1213: a dropped folder is made under a clean unique name, its folders and files go inside it (dot files keep
    /// their names), and nothing lands outside the copy: an `under` that steps out, names a folder that is not there or
    /// one this link did not make is refused.
    #[test]
    fn a_dropped_folder_lands_whole_in_the_tabs_folder_and_nothing_outside_it() {
        let root = tempfile::tempdir().unwrap();
        let up = Uploads::new(root.path().join("run"));
        let top = up.make_folder("t1", "my proj", None).unwrap();
        assert_eq!(top.name, "my proj");
        let top_path = up.root().join("t1").join("my proj");
        assert_eq!(top.host_path.unwrap(), top_path);
        // The same name again is a new folder, never the first one merged into.
        assert_eq!(
            up.make_folder("t1", "my proj", None).unwrap().name,
            "my proj-2"
        );
        let src = up.make_folder("t1", "src", Some("my proj")).unwrap();
        assert_eq!(src.host_path.unwrap(), top_path.join("src"));
        assert_eq!(
            up.make_folder("t1", ".git", Some("my proj")).unwrap().name,
            ".git"
        );
        let f = up
            .put_in("t1", Some("my proj/src"), "main.rs", 0, 1, b"fn main() {}")
            .unwrap();
        assert_eq!(f.name, "main.rs");
        assert_eq!(
            std::fs::read(f.host_path.unwrap()).unwrap(),
            b"fn main() {}"
        );
        let dot = up
            .put_in("t1", Some("my proj"), ".gitignore", 0, 2, b"tar")
            .unwrap();
        assert_eq!(
            (dot.name.as_str(), dot.host_path.as_ref()),
            (".gitignore", None)
        );
        let dot = up
            .put_in("t1", Some("my proj"), &dot.name, 1, 2, b"get")
            .unwrap();
        assert_eq!(std::fs::read(dot.host_path.unwrap()).unwrap(), b"target");
        for under in [
            "other",
            "my proj/../x",
            "my proj/nope",
            "../t2",
            "my proj\\src",
        ] {
            assert_eq!(
                up.put_in("t1", Some(under), "x", 0, 1, b"")
                    .unwrap_err()
                    .code,
                code::BAD_PARAMS,
                "{under}"
            );
        }
        assert_eq!(
            up.put_in("t2", Some("my proj"), "x", 0, 1, b"")
                .unwrap_err()
                .code,
            code::BAD_PARAMS,
            "another tab never writes into this tab's copy"
        );
        // A link inside the copy is never followed.
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(root.path(), top_path.join("out")).is_ok();
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_dir(root.path(), top_path.join("out")).is_ok();
        if made {
            assert_eq!(
                up.put_in("t1", Some("my proj/out"), "x", 0, 1, b"")
                    .unwrap_err()
                    .code,
                code::BAD_PARAMS
            );
        }
        up.forget("t1");
        assert_eq!(
            up.put_in("t1", Some("my proj"), "y", 0, 1, b"")
                .unwrap_err()
                .code,
            code::BAD_PARAMS,
            "a closed tab's copy takes nothing more"
        );
    }

    /// Rule 1213: a dropped folder holds at most `FOLDER_FILES_MAX` files and `FOLDER_MAX` bytes on the host too; past
    /// either the file is refused `too_large` and its part is gone.
    #[test]
    fn a_dropped_folder_past_its_caps_is_refused_on_the_host_too() {
        let root = tempfile::tempdir().unwrap();
        let up = Uploads::new(root.path().join("run"));
        up.make_folder("t1", "big", None).unwrap();
        let key = ("t1".to_string(), "big".to_string());
        up.trees().get_mut(&key).unwrap().files = FOLDER_FILES_MAX;
        assert_eq!(
            up.put_in("t1", Some("big"), "x", 0, 1, b"x")
                .unwrap_err()
                .code,
            code::TOO_LARGE
        );
        up.trees().get_mut(&key).unwrap().files = 0;
        let a = up.put_in("t1", Some("big"), "a.bin", 0, 2, b"aa").unwrap();
        up.trees().get_mut(&key).unwrap().bytes = FOLDER_MAX - 1;
        assert_eq!(
            up.put_in("t1", Some("big"), &a.name, 1, 2, b"bb")
                .unwrap_err()
                .code,
            code::TOO_LARGE
        );
        assert!(
            !up.root()
                .join("t1")
                .join("big")
                .join("a.bin.flare-part")
                .exists()
        );
    }

    /// Rule 304: a closed tab's upload folder goes, and an earlier run's leftovers go after a day.
    #[test]
    fn a_closed_tabs_uploads_are_removed_and_old_runs_are_swept() {
        let root = tempfile::tempdir().unwrap();
        let up = Uploads::new(root.path().join("run"));
        up.put("t1", "a.txt", 0, 1, b"a").unwrap();
        let half = up.put("t1", "b.txt", 0, 2, b"b").unwrap();
        let dir = up.forget("t1").unwrap();
        assert_eq!(dir, up.root().join("t1"));
        assert_eq!(
            up.put("t1", &half.name, 1, 2, b"c").unwrap_err().code,
            code::BAD_PARAMS
        );
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(up.forget("../x").is_none());
        // The sweep removes what is older than its age and keeps the rest.
        let parent = root.path().join("sweep");
        std::fs::create_dir_all(parent.join("old")).unwrap();
        std::fs::create_dir_all(parent.join("new")).unwrap();
        std::thread::sleep(Duration::from_millis(30)); // sync-ok: test code only, never the app
        sweep_old(&parent, Duration::from_millis(10));
        assert!(!parent.join("old").exists() && !parent.join("new").exists());
        std::fs::create_dir_all(parent.join("fresh")).unwrap();
        sweep_old(&parent, LEFTOVER_AGE);
        assert!(parent.join("fresh").exists());
    }

    /// Rule 1191: a run that is still alive keeps its folders, however long ago they last changed; once the run is gone
    /// (here: dropped, as a run that ends or crashes lets go of its lock) its old folders are swept.
    #[test]
    fn a_live_runs_folders_are_never_swept_however_old_and_a_gone_runs_are() {
        let root = tempfile::tempdir().unwrap();
        let ups = root.path().join("flare-remote");
        let cps = root.path().join("flare-remote-copies");
        let up = Uploads::new(ups.join("runA"));
        let shot = up
            .put("t3", "shot.png", 0, 1, b"png")
            .unwrap()
            .host_path
            .unwrap();
        let c = Copies::new(cps.join("runA"));
        let (dir, name) = c.save("t3", "notes.md", b"x").unwrap();
        // A folder an older Flare (or a crash before any lock) left: judged by its age alone.
        std::fs::create_dir_all(ups.join("runOld").join("t1")).unwrap();
        std::thread::sleep(Duration::from_millis(30)); // sync-ok: test code only, never the app
        sweep_old(&ups, Duration::from_millis(10));
        sweep_old(&cps, Duration::from_millis(10));
        assert!(shot.exists(), "the live run's upload is kept");
        assert!(dir.join(&name).exists(), "the live run's copy is kept");
        assert!(!ups.join("runOld").exists(), "a lockless leftover goes");
        drop(up);
        drop(c);
        sweep_old(&ups, Duration::from_millis(10));
        sweep_old(&cps, Duration::from_millis(10));
        assert!(!ups.join("runA").exists() && !cps.join("runA").exists());
    }

    #[test]
    fn a_fetched_copy_is_kept_per_tab_under_a_clean_name() {
        let root = tempfile::tempdir().unwrap();
        let c = Copies::new(root.path().join("run"));
        let (dir, name) = c.save("t3", "notes.md", b"one").unwrap();
        assert_eq!(
            (dir.clone(), name.as_str()),
            (root.path().join("run").join("t3"), "notes.md")
        );
        let (_, again) = c.save("t3", "notes.md", b"two").unwrap();
        assert_eq!(std::fs::read(dir.join(again)).unwrap(), b"two");
        let (_, odd) = c.save("t3", "a%b.txt", b"x").unwrap();
        assert_eq!(odd, "a_b.txt");
        assert!(c.save("../t3", "x", b"").is_err());
        // Only a tab that kept a copy has a folder to remove when it closes.
        assert_eq!(c.forget("t9"), None);
        assert_eq!(c.forget("t3"), Some(dir));
        assert_eq!(c.forget("t3"), None);
    }
}
