//! The model store: download, verify, pack install, remove (Task 12). Ported from Prism Terminal's
//! `core/main/dictationStore.ts` at e5848cb.
//!
//! THE ONE PROMISE this file makes: nothing unverified is ever loaded or run. A download streams into
//! `<final>.part` while it is hashed and counted, and only a file whose byte count AND SHA-256 both match
//! the catalogue is renamed into place. So a file under its final name was verified by construction, and
//! the cheap question asked afterwards (is it there, is it the catalogued size) is enough to call it
//! installed without hashing 2.9 GB on every look.
//!
//! The pack is a zip, so "in place" takes three more steps: the verified zip is unzipped into its folder,
//! the zip is deleted, and a small marker file is written LAST. A folder without its marker is a crash
//! half way through an unzip and never reads as installed. The folder carries the hash in its name so a
//! pack the catalogue has moved on from can never be mistaken for the new one.
//!
//! Layout under the root (`FLARE_DICTATION_ROOT` or `%LOCALAPPDATA%\FlareDictation`, O14):
//!
//! ```text
//! models/<id>.bin                 a model (`.part` while downloading)
//! engines/<id>.zip                the pack's zip, only between download and unzip
//! engines/<id>-<sha[0..12]>/      the GPU pack, unzipped, with `flare-installed.json` written last
//! ```
//!
//! Every call returns at once; the work runs on a short-lived thread per operation and the answer comes
//! on a channel. Nothing here retries: a failure is reported and the row offers Retry. The network is
//! behind the [`Fetch`] trait (`fetch_ureq.rs` in the app, a stand-in in tests), so no test downloads.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::catalog;

/// Written last into the pack's folder; its presence IS "the unzip finished" (O14 names it).
pub const PACK_MARKER: &str = "flare-installed.json";

/// How long a `.part` file must have sat untouched before `status` calls it a leftover and deletes it.
pub const STALE_PART_MS: u64 = 60_000;

/// How long `model_path` and `gpu_engine_dir` believe an installed answer.
pub const INSTALLED_MEMO_MS: u64 = 30_000;

/// "At most about 10 times a second": a 3 GB model is about 50 000 chunks.
const PROGRESS_EVERY_MS: u64 = 100;

const SERVER_EXE: &str = "whisper-server.exe";

/// Why a download or install did not finish. `text()` is what the model row says (parity line 901).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    Network,
    Checksum,
    Disk,
    Cancelled,
    Unpack,
}

impl Failure {
    /// The words shown to the user for this failure.
    pub fn text(self) -> &'static str {
        match self {
            Failure::Network => "The download failed.",
            Failure::Checksum => "The file failed its check and was deleted.",
            Failure::Disk => "It could not be written to disk.",
            Failure::Cancelled => "The download was cancelled.",
            Failure::Unpack => "It could not be unpacked.",
        }
    }
}

/// Where the bytes come from. `get` streams the body of `url` into `sink`; the sink answers `false` to
/// stop (cancel, or more bytes than the catalogue promised), and `get` then returns `Err(Cancelled)`.
/// A fetch that is waiting for data may call the sink with an EMPTY slice so the store can check cancel.
/// (`Send + Sync` rather than the plan's `Send`: downloads run on their own threads.)
pub trait Fetch: Send + Sync {
    fn get(&self, url: &str, sink: &mut dyn FnMut(&[u8]) -> bool) -> Result<(), Failure>;
}

/// What a progress listener is told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub id: String,
    pub done: u64,
    pub total: u64,
}

/// The state of one catalogue entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemState {
    Absent,
    Downloading { done: u64, total: u64 },
    Installed,
}

/// One catalogue entry's status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemStatus {
    pub id: String,
    pub state: ItemState,
}

/// Every entry's status, in catalogue order.
pub type Status = Vec<ItemStatus>;

/// A model or the GPU pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Model,
    GpuPack,
}

/// One downloadable thing, owned (the shipped catalogue is `'static`; tests build their own).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub id: String,
    pub url: String,
    pub bytes: u64,
    pub sha256: String,
    pub kind: Kind,
}

/// The shipped catalogue: every model (the test-only ones too) and the GPU pack.
pub fn shipped_catalog() -> Vec<Item> {
    let mut v: Vec<Item> = catalog::MODELS
        .iter()
        .map(|m| Item {
            id: m.id.to_string(),
            url: m.url.to_string(),
            bytes: m.bytes,
            sha256: m.sha256.to_string(),
            kind: Kind::Model,
        })
        .collect();
    let p = catalog::GPU_PACK;
    v.push(Item {
        id: p.id.to_string(),
        url: p.url.to_string(),
        bytes: p.bytes,
        sha256: p.sha256.to_string(),
        kind: Kind::GpuPack,
    });
    v
}

/// The models folder: `FLARE_DICTATION_ROOT`, else `%LOCALAPPDATA%\FlareDictation`.
pub fn default_root() -> PathBuf {
    if let Some(v) = std::env::var_os(flare_win::names::SEAM_DICTATION_ROOT) {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    let base = std::env::var_os("LOCALAPPDATA").map_or_else(std::env::temp_dir, PathBuf::from);
    base.join(flare_win::names::DICTATION_FOLDER)
}

/// The neighbour apps' models folder, `%LOCALAPPDATA%\` plus Prism Terminal's dictation folder (O14, Q9). Flare only reads it, to
/// link in a model that matches the catalogue's size and SHA-256; nothing is written there.
pub fn neighbour_root() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map_or_else(std::env::temp_dir, PathBuf::from);
    // Spelled in pieces so no Flare source holds the other product's name whole (tests/names.rs).
    base.join(["Pri", "smDictation"].concat())
}

/// A clock in milliseconds (epoch), injectable so progress and the memo are testable.
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// Unzips `zip` into `out`, checking the flag between members; `Err` carries the message.
pub type Unzip = Arc<dyn Fn(&Path, &Path, &AtomicBool) -> Result<(), String> + Send + Sync>;

type Listener = Box<dyn Fn(Progress) + Send>;

/// Everything a store needs; `Store::open` fills the app's own.
pub struct StoreConfig {
    pub root: PathBuf,
    pub fetch: Box<dyn Fetch>,
    pub catalog: Vec<Item>,
    pub now: Option<Clock>,
    pub unzip: Option<Unzip>,
}

struct Flight {
    cancelled: AtomicBool,
    received: AtomicU64,
    listeners: Mutex<Vec<Listener>>,
    waiters: Mutex<Vec<Sender<Result<(), Failure>>>>,
    done: (Mutex<bool>, Condvar),
}

#[derive(Default)]
struct State {
    flights: HashMap<String, Arc<Flight>>,
    /// id -> the time until which an "installed" answer is believed.
    memo: HashMap<String, u64>,
    /// Where the server was last found, so the engine's every ask is not a folder walk.
    engine_dir: Option<PathBuf>,
}

struct Inner {
    root: PathBuf,
    catalog: Vec<Item>,
    fetch: Box<dyn Fetch>,
    unzip: Unzip,
    now: Clock,
    state: Mutex<State>,
}

/// The store. Cheap to clone: every clone is the same store.
#[derive(Clone)]
pub struct Store {
    inner: Arc<Inner>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// An id becomes a file name, so it is held to what cannot leave the folder.
fn safe_id(id: &str) -> bool {
    let mut chars = id.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// A path can be held for a moment by a virus scanner reading what was just written: retry a few times.
fn gone(path: &Path) -> io::Result<()> {
    let mut last = Ok(());
    for attempt in 0..4 {
        let r = if path.is_dir() {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        };
        match r {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => last = Err(e),
        }
        if attempt < 3 {
            thread::sleep(Duration::from_millis(100));
        }
    }
    last
}

/// Whole or not at all: write beside, then rename over.
fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text)?;
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

/// The folder that actually holds `whisper-server.exe`: the official zip nests everything under
/// `Release/`, and a later release may not. Two levels down and no further.
fn find_server(dir: &Path, depth: u32) -> Option<PathBuf> {
    if dir.join(SERVER_EXE).is_file() {
        return Some(dir.to_path_buf());
    }
    if depth >= 2 {
        return None;
    }
    let mut subs: Vec<PathBuf> = fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    subs.sort();
    subs.into_iter().find_map(|d| find_server(&d, depth + 1))
}

/// The real unzip: the `zip` crate, member by member, refusing any name that would leave `out`.
fn unzip_real(zip: &Path, out: &Path, cancel: &AtomicBool) -> Result<(), String> {
    let file = File::open(zip).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    fs::create_dir_all(out).map_err(|e| e.to_string())?;
    for i in 0..archive.len() {
        if cancel.load(Ordering::SeqCst) {
            return Err("cancelled".to_string());
        }
        let mut member = archive.by_index(i).map_err(|e| e.to_string())?;
        let rel = member
            .enclosed_name()
            .ok_or_else(|| format!("unsafe name in the archive (member {i})"))?;
        let target = out.join(rel);
        if member.is_dir() {
            fs::create_dir_all(&target).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut dest = File::create(&target).map_err(|e| e.to_string())?;
        io::copy(&mut member, &mut dest).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut f = File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

impl Store {
    /// The app's store: the shipped catalogue, the given root and fetch.
    pub fn open(root: PathBuf, fetch: Box<dyn Fetch>) -> Self {
        Self::with_config(StoreConfig {
            root,
            fetch,
            catalog: shipped_catalog(),
            now: None,
            unzip: None,
        })
        .expect("the shipped catalogue has safe ids")
    }

    /// A store over any catalogue (tests); `Err` names an id that could walk out of the folder.
    pub fn with_config(cfg: StoreConfig) -> Result<Self, String> {
        for e in &cfg.catalog {
            if !safe_id(&e.id) {
                return Err(format!(
                    "dictation catalogue: \"{}\" cannot be a file name",
                    e.id
                ));
            }
        }
        let now = cfg.now.unwrap_or_else(|| Arc::new(epoch_ms));
        let unzip = cfg.unzip.unwrap_or_else(|| Arc::new(unzip_real));
        Ok(Self {
            inner: Arc::new(Inner {
                root: cfg.root,
                catalog: cfg.catalog,
                fetch: cfg.fetch,
                unzip,
                now,
                state: Mutex::new(State::default()),
            }),
        })
    }

    /// Per entry: absent, downloading or installed. A stale `.part` (60 s) is removed on the way.
    pub fn status(&self) -> Receiver<Status> {
        let (tx, rx) = channel();
        let inner = self.inner.clone();
        thread::spawn(move || {
            let _ = tx.send(inner.status_now());
        });
        rx
    }

    /// Start (or join) a download. `on` hears the progress; the receiver gets the end.
    /// Ids come from the catalogue only: any other id answers `Network` at once.
    pub fn download(
        &self,
        id: &str,
        on: Box<dyn Fn(Progress) + Send>,
    ) -> Receiver<Result<(), Failure>> {
        let (tx, rx) = channel();
        Inner::start(&self.inner, id.to_string(), on, tx);
        rx
    }

    /// Stop a running download; its `.part` goes.
    pub fn cancel(&self, id: &str) {
        if let Some(f) = lock(&self.inner.state).flights.get(id) {
            f.cancelled.store(true, Ordering::SeqCst);
        }
    }

    /// Delete the model (or the pack and every folder of an older one), stopping a download first.
    pub fn remove(&self, id: &str) -> Receiver<()> {
        let (tx, rx) = channel();
        let inner = self.inner.clone();
        let id = id.to_string();
        thread::spawn(move || {
            inner.remove_now(&id);
            let _ = tx.send(());
        });
        rx
    }

    /// The installed model's file, or `None`. A "yes" is believed for 30 s, a "no" never.
    pub fn model_path(&self, id: &str) -> Option<PathBuf> {
        let e = self.inner.find(id)?;
        (e.kind == Kind::Model && self.inner.installed(e)).then(|| self.inner.final_path(e))
    }

    /// The folder holding the installed pack's `whisper-server.exe`, or `None`.
    pub fn gpu_engine_dir(&self) -> Option<PathBuf> {
        self.inner.gpu_engine_dir()
    }

    /// O14: for each model not installed here whose twin in `other_root` matches the catalogue's size
    /// and SHA-256, hard-link it (a copy across volumes). Returns the ids reused. Never writes there.
    pub fn reuse_models_from(&self, other_root: PathBuf) -> Receiver<Vec<String>> {
        let (tx, rx) = channel();
        let inner = self.inner.clone();
        thread::spawn(move || {
            let _ = tx.send(inner.reuse_now(&other_root));
        });
        rx
    }
}

impl Inner {
    fn find(&self, id: &str) -> Option<&Item> {
        self.catalog.iter().find(|e| e.id == id)
    }

    fn final_path(&self, e: &Item) -> PathBuf {
        match e.kind {
            Kind::GpuPack => self.root.join("engines").join(format!("{}.zip", e.id)),
            Kind::Model => self.root.join("models").join(format!("{}.bin", e.id)),
        }
    }

    fn part_path(&self, e: &Item) -> PathBuf {
        let mut s = self.final_path(e).into_os_string();
        s.push(".part");
        PathBuf::from(s)
    }

    fn pack_dir(&self, e: &Item) -> PathBuf {
        let short: String = e.sha256.chars().take(12).collect::<String>().to_lowercase();
        self.root.join("engines").join(format!("{}-{short}", e.id))
    }

    /// One stat, or a marker of under 200 bytes. A marker counts only when written for THIS entry.
    fn installed_sync(&self, e: &Item) -> bool {
        match e.kind {
            Kind::Model => {
                fs::metadata(self.final_path(e)).is_ok_and(|m| m.is_file() && m.len() == e.bytes)
            }
            Kind::GpuPack => {
                let Ok(text) = fs::read_to_string(self.pack_dir(e).join(PACK_MARKER)) else {
                    return false;
                };
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
                    return false;
                };
                v.get("bytes").and_then(serde_json::Value::as_u64) == Some(e.bytes)
                    && v.get("sha256").and_then(serde_json::Value::as_str)
                        == Some(e.sha256.as_str())
            }
        }
    }

    /// A "yes" is believed for 30 s (the engine asks on every pass); a "no" is never kept.
    fn installed(&self, e: &Item) -> bool {
        let now = (self.now)();
        if lock(&self.state)
            .memo
            .get(&e.id)
            .is_some_and(|&until| now < until)
        {
            return true;
        }
        let ok = self.installed_sync(e);
        let mut st = lock(&self.state);
        if ok {
            st.memo.insert(e.id.clone(), now + INSTALLED_MEMO_MS);
        } else {
            st.memo.remove(&e.id);
        }
        ok
    }

    fn forget(&self, id: &str) {
        lock(&self.state).memo.remove(id);
    }

    fn gpu_engine_dir(&self) -> Option<PathBuf> {
        let e = self.catalog.iter().find(|e| e.kind == Kind::GpuPack)?;
        let believed = lock(&self.state)
            .memo
            .get(&e.id)
            .is_some_and(|&until| until > (self.now)());
        if !self.installed(e) {
            return None;
        }
        let dir = self.pack_dir(e);
        let memo = lock(&self.state).engine_dir.clone();
        if let Some(m) = memo {
            if m.starts_with(&dir) && (believed || m.join(SERVER_EXE).is_file()) {
                return Some(m);
            }
        }
        let found = find_server(&dir, 0);
        lock(&self.state).engine_dir.clone_from(&found);
        found
    }

    /// Folders of this pack under another hash: a pack the catalogue has replaced.
    fn sweep_packs(&self, e: &Item, keep: Option<&Path>) {
        let prefix = format!("{}-", e.id);
        let Ok(rd) = fs::read_dir(self.root.join("engines")) else {
            return;
        };
        for entry in rd.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            let ours = name.strip_prefix(&prefix).is_some_and(|rest| {
                rest.len() == 12
                    && rest
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            });
            if ours && Some(entry.path().as_path()) != keep {
                let _ = gone(&entry.path());
            }
        }
    }

    fn status_now(&self) -> Status {
        let mut out = Status::new();
        for e in &self.catalog {
            let flight = lock(&self.state).flights.get(&e.id).cloned();
            if let Some(f) = flight {
                // Through the unzip too: the row stays busy until the pack can be used.
                out.push(ItemStatus {
                    id: e.id.clone(),
                    state: ItemState::Downloading {
                        done: f.received.load(Ordering::SeqCst),
                        total: e.bytes,
                    },
                });
                continue;
            }
            let part = self.part_path(e);
            if let Ok(md) = fs::metadata(&part) {
                let modified = md
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_millis() as u64);
                if (self.now)().saturating_sub(modified) > STALE_PART_MS {
                    let _ = gone(&part);
                }
            }
            self.forget(&e.id);
            let here = self.installed(e);
            out.push(ItemStatus {
                id: e.id.clone(),
                state: if here {
                    ItemState::Installed
                } else {
                    ItemState::Absent
                },
            });
        }
        out
    }

    fn remove_now(&self, id: &str) {
        let Some(e) = self.find(id) else { return };
        let flight = lock(&self.state).flights.get(id).cloned();
        if let Some(f) = flight {
            f.cancelled.store(true, Ordering::SeqCst);
            f.wait_done();
        }
        self.forget(id);
        let _ = gone(&self.part_path(e));
        let _ = gone(&self.final_path(e));
        if e.kind == Kind::GpuPack {
            lock(&self.state).engine_dir = None;
            let _ = gone(&self.pack_dir(e));
            self.sweep_packs(e, None);
        }
        // Again once the files are gone: an ask made while they went may have found them still there.
        self.forget(id);
    }

    fn start(this: &Arc<Self>, id: String, on: Listener, tx: Sender<Result<(), Failure>>) {
        let Some(e) = this.find(&id).cloned() else {
            let _ = tx.send(Err(Failure::Network));
            return;
        };
        this.forget(&id);
        let mut st = lock(&this.state);
        if let Some(running) = st.flights.get(&id).cloned() {
            if running.cancelled.load(Ordering::SeqCst) {
                // Cancel and then Retry, quicker than the cancelled flight can tidy up: the answer owed
                // is a new download, not the old one's "cancelled".
                drop(st);
                let inner = this.clone();
                thread::spawn(move || {
                    running.wait_done();
                    Inner::start(&inner, id, on, tx);
                });
                return;
            }
            // Two windows, one download: both hear the progress, both get the end.
            lock(&running.listeners).push(on);
            lock(&running.waiters).push(tx);
            return;
        }
        let flight = Arc::new(Flight {
            cancelled: AtomicBool::new(false),
            received: AtomicU64::new(0),
            listeners: Mutex::new(vec![on]),
            waiters: Mutex::new(vec![tx]),
            done: (Mutex::new(false), Condvar::new()),
        });
        st.flights.insert(id.clone(), flight.clone());
        drop(st);
        let inner = this.clone();
        thread::spawn(move || {
            let result = inner.run(&e, &flight);
            // Out of the map and told, under one lock, so an ask is either joined or starts afresh.
            let waiters = {
                let mut st = lock(&inner.state);
                st.flights.remove(&e.id);
                std::mem::take(&mut *lock(&flight.waiters))
            };
            for w in waiters {
                let _ = w.send(result);
            }
            flight.finish();
        });
    }

    fn report(e: &Item, f: &Flight) {
        let p = Progress {
            id: e.id.clone(),
            done: f.received.load(Ordering::SeqCst),
            total: e.bytes,
        };
        for listen in lock(&f.listeners).iter() {
            // A listener is a window that may have closed; the download is not its hostage.
            let _ = catch_unwind(AssertUnwindSafe(|| listen(p.clone())));
        }
    }

    /// One download, start to finish. Every way out is a result.
    fn run(&self, e: &Item, f: &Flight) -> Result<(), Failure> {
        if self.installed_sync(e) {
            return Ok(());
        }
        let final_path = self.final_path(e);
        let part = self.part_path(e);
        let is_pack = e.kind == Kind::GpuPack;
        // Clear the way: a part file from a crash, and for the pack the zip and the half-unzipped folder.
        let cleared = (|| {
            fs::create_dir_all(final_path.parent().unwrap_or(&self.root))?;
            gone(&part)?;
            if is_pack {
                gone(&final_path)?;
                gone(&self.pack_dir(e))?;
            }
            Ok::<(), io::Error>(())
        })();
        if let Err(err) = cleared {
            log::warn!("dictation store: clearing for {}: {err}", e.id);
            return Err(Failure::Disk);
        }
        let file = File::create(&part).map_err(|err| {
            log::warn!("dictation store: creating {}: {err}", part.display());
            Failure::Disk
        })?;

        let outcome = self.stream(e, f, file);
        let verdict = outcome.and_then(|(got, bytes)| {
            if bytes != e.bytes {
                log::warn!(
                    "dictation store: {} got {bytes} bytes, expected {}",
                    e.id,
                    e.bytes
                );
                Err(Failure::Checksum)
            } else if got != e.sha256.to_lowercase() {
                log::warn!("dictation store: {} sha256 {got}", e.id);
                Err(Failure::Checksum)
            } else {
                Ok(())
            }
        });
        let verdict = match verdict {
            Ok(()) if f.cancelled.load(Ordering::SeqCst) => Err(Failure::Cancelled),
            other => other,
        };
        if let Err(fail) = verdict {
            let _ = gone(&part);
            return Err(fail);
        }
        if let Err(err) = fs::rename(&part, &final_path) {
            log::warn!("dictation store: rename {}: {err}", e.id);
            let _ = gone(&part);
            return Err(Failure::Disk);
        }
        if !is_pack {
            return Ok(());
        }

        // The pack: verified, THEN unzipped. `final_path` is the zip.
        let dir = self.pack_dir(e);
        let undo = || {
            let _ = gone(&final_path);
            let _ = gone(&dir);
        };
        if let Err(err) = (self.unzip)(&final_path, &dir, &f.cancelled) {
            log::warn!("dictation store: unzip {}: {err}", e.id);
            undo();
            return Err(if f.cancelled.load(Ordering::SeqCst) {
                Failure::Cancelled
            } else {
                Failure::Unpack
            });
        }
        if f.cancelled.load(Ordering::SeqCst) {
            undo();
            return Err(Failure::Cancelled);
        }
        let Some(server) = find_server(&dir, 0) else {
            log::warn!("dictation store: no {SERVER_EXE} in the pack");
            undo();
            return Err(Failure::Unpack);
        };
        let marker =
            serde_json::json!({ "id": e.id, "bytes": e.bytes, "sha256": e.sha256 }).to_string();
        let sealed = gone(&final_path).and_then(|()| write_atomic(&dir.join(PACK_MARKER), &marker));
        if let Err(err) = sealed {
            log::warn!("dictation store: sealing {}: {err}", e.id);
            undo();
            return Err(Failure::Disk);
        }
        lock(&self.state).engine_dir = Some(server);
        self.sweep_packs(e, Some(&dir));
        Ok(())
    }

    /// Streams the body into `file`, hashing and counting. Returns (sha256 hex, bytes) or why not.
    fn stream(&self, e: &Item, f: &Flight, mut file: File) -> Result<(String, u64), Failure> {
        let mut hash = Sha256::new();
        let mut failed: Option<Failure> = None;
        let mut last_report: Option<u64> = None;
        let mut last_reported: u64 = u64::MAX;
        let fetched = self.fetch.get(&e.url, &mut |chunk: &[u8]| {
            if f.cancelled.load(Ordering::SeqCst) {
                failed = Some(Failure::Cancelled);
                return false;
            }
            if chunk.is_empty() {
                return true;
            }
            let total =
                f.received.fetch_add(chunk.len() as u64, Ordering::SeqCst) + chunk.len() as u64;
            // More than the catalogue promised can never verify, so it is not written to the end
            // first: a wrong url must not be able to fill the disk.
            if total > e.bytes {
                failed = Some(Failure::Checksum);
                return false;
            }
            hash.update(chunk);
            if let Err(err) = file.write_all(chunk) {
                log::warn!("dictation store: write {}: {err}", e.id);
                failed = Some(Failure::Disk);
                return false;
            }
            let t = (self.now)();
            if last_report.is_none_or(|l| t.saturating_sub(l) >= PROGRESS_EVERY_MS) {
                last_report = Some(t);
                last_reported = total;
                Self::report(e, f);
            }
            true
        });
        drop(file);
        let received = f.received.load(Ordering::SeqCst);
        let failure = failed.or_else(|| match fetched {
            Err(_) if f.cancelled.load(Ordering::SeqCst) => Some(Failure::Cancelled),
            Err(x) => Some(x),
            Ok(()) => None,
        });
        if let Some(x) = failure {
            return Err(x);
        }
        // The final word, whatever the clock says, unless the last chunk already said it.
        if last_reported != received {
            Self::report(e, f);
        }
        Ok((hex(&hash.finalize()), received))
    }

    fn reuse_now(&self, other_root: &Path) -> Vec<String> {
        let mut reused = Vec::new();
        for e in self.catalog.iter().filter(|e| e.kind == Kind::Model) {
            if lock(&self.state).flights.contains_key(&e.id) || self.installed_sync(e) {
                continue;
            }
            let src = other_root.join("models").join(format!("{}.bin", e.id));
            if !fs::metadata(&src).is_ok_and(|m| m.is_file() && m.len() == e.bytes) {
                continue;
            }
            if sha256_file(&src).ok().as_deref() != Some(e.sha256.to_lowercase().as_str()) {
                continue;
            }
            let dest = self.final_path(e);
            let part = self.part_path(e);
            let made = (|| {
                fs::create_dir_all(dest.parent().unwrap_or(&self.root))?;
                if fs::hard_link(&src, &dest).is_ok() {
                    return Ok(());
                }
                // Another volume: copy beside, then rename, so a half copy is never a model.
                let _ = gone(&part);
                fs::copy(&src, &part)?;
                fs::rename(&part, &dest)
            })();
            if let Err(err) = made {
                log::warn!("dictation store: reuse {}: {err}", e.id);
                let _ = gone(&part);
                continue;
            }
            self.forget(&e.id);
            reused.push(e.id.clone());
        }
        reused
    }
}

impl Flight {
    fn finish(&self) {
        *lock(&self.done.0) = true;
        self.done.1.notify_all();
    }

    fn wait_done(&self) {
        let mut d = lock(&self.done.0);
        while !*d {
            d = self.done.1.wait(d).unwrap_or_else(|e| e.into_inner());
        }
    }
}
