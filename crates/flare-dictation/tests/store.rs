//! The store's rules (Task 12), checked against a stand-in `Fetch` that serves bytes from memory and a
//! temp folder: nothing here reaches the network or the real `%LOCALAPPDATA%`. The stand-in can serve a
//! good file, a corrupt one, a short one, a 404, a refused connection, a line that drops half way and one
//! that stalls for ever (for cancel and the shared download). Unzipping is a fake everywhere but the last
//! test, which unzips a real zip built in the test, in a folder with a space.
//!
//! The 27 tests ported from Prism Terminal's `dictationStore.test.ts` keep their titles (snake-cased);
//! the three Flare adds are at the end.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use flare_dictation::fetch_ureq as net;
use flare_dictation::store::{
    Failure, Fetch, Item, ItemState, ItemStatus, Kind, Progress, Store, StoreConfig, Unzip,
    PACK_MARKER, STALE_PART_MS,
};
use sha2::{Digest, Sha256};

fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| ((i * 31 + (i >> 8)) & 0xff) as u8).collect()
}

fn sha(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

const PACK: &[u8] = b"not really a zip: the fake unzip never opens it";

enum Route {
    Body(Vec<u8>),
    NotFound,
    Refused,
    /// The head and then the line drops.
    Drop(Vec<u8>),
    /// The head and then nothing, for as long as it is asked.
    Stall(Vec<u8>),
}

#[derive(Default)]
struct Mem {
    routes: Mutex<HashMap<String, Route>>,
    hits: Mutex<HashMap<String, u32>>,
    asked: Mutex<Vec<String>>,
}

impl Mem {
    fn hits(&self, url: &str) -> u32 {
        self.hits.lock().unwrap().get(url).copied().unwrap_or(0)
    }
}

struct MemFetch(Arc<Mem>);

impl Fetch for MemFetch {
    fn get(&self, url: &str, sink: &mut dyn FnMut(&[u8]) -> bool) -> Result<(), Failure> {
        *self
            .0
            .hits
            .lock()
            .unwrap()
            .entry(url.to_string())
            .or_default() += 1;
        self.0.asked.lock().unwrap().push(url.to_string());
        let mut whole = |b: &[u8]| -> Result<(), Failure> {
            // In pieces, so progress has more than one chunk to count.
            for piece in b.chunks(64 * 1024) {
                if !sink(piece) {
                    return Err(Failure::Cancelled);
                }
            }
            Ok(())
        };
        let routes = self.0.routes.lock().unwrap();
        match routes.get(url) {
            None | Some(Route::NotFound) | Some(Route::Refused) => Err(Failure::Network),
            Some(Route::Body(b)) => {
                let b = b.clone();
                drop(routes);
                whole(&b)
            }
            Some(Route::Drop(head)) => {
                let head = head.clone();
                drop(routes);
                whole(&head)?;
                Err(Failure::Network)
            }
            Some(Route::Stall(head)) => {
                let head = head.clone();
                drop(routes);
                whole(&head)?;
                loop {
                    thread::sleep(Duration::from_millis(5));
                    if !sink(&[]) {
                        return Err(Failure::Cancelled);
                    }
                }
            }
        }
    }
}

struct Fx {
    _holder: tempfile::TempDir,
    root: PathBuf,
    mem: Arc<Mem>,
    good: Vec<u8>,
}

impl Fx {
    /// A root that does NOT exist yet, inside a temp folder (with a space in its name) that does.
    fn new() -> Self {
        let holder = tempfile::Builder::new()
            .prefix("flare dictation ")
            .tempdir()
            .unwrap();
        let root = holder.path().join("FlareDictation");
        let good = pattern(512 * 1024);
        let mem = Arc::new(Mem::default());
        {
            let mut r = mem.routes.lock().unwrap();
            let mut corrupt = good.clone();
            corrupt[1000] ^= 0xff;
            r.insert("/good".into(), Route::Body(good.clone()));
            r.insert("/corrupt".into(), Route::Body(corrupt));
            r.insert(
                "/short".into(),
                Route::Body(good[..good.len() / 2].to_vec()),
            );
            r.insert("/pack".into(), Route::Body(PACK.to_vec()));
            r.insert("/nothing-here".into(), Route::NotFound);
            r.insert("/refused".into(), Route::Refused);
            r.insert("/drop".into(), Route::Drop(good[..4096].to_vec()));
            r.insert("/slow".into(), Route::Stall(good[..4096].to_vec()));
        }
        Self {
            _holder: holder,
            root,
            mem,
            good,
        }
    }

    fn item(&self, id: &str, path: &str, body: &[u8], kind: Kind) -> Item {
        Item {
            id: id.into(),
            url: path.into(),
            bytes: body.len() as u64,
            sha256: sha(body),
            kind,
        }
    }

    fn model(&self, id: &str, path: &str) -> Item {
        self.item(id, path, &self.good.clone(), Kind::Model)
    }

    fn pack(&self, path: &str, body: &[u8]) -> Item {
        self.item("gpu", path, body, Kind::GpuPack)
    }

    fn store(&self, catalog: Vec<Item>) -> Store {
        self.store_with(catalog, None, None)
    }

    fn store_with(
        &self,
        catalog: Vec<Item>,
        now: Option<Arc<dyn Fn() -> u64 + Send + Sync>>,
        unzip: Option<Unzip>,
    ) -> Store {
        Store::with_config(StoreConfig {
            root: self.root.clone(),
            fetch: Box::new(MemFetch(self.mem.clone())),
            catalog,
            now,
            unzip,
        })
        .expect("a store")
    }

    fn models(&self) -> Vec<String> {
        names(&self.root.join("models"))
    }

    fn engines(&self) -> Vec<String> {
        names(&self.root.join("engines"))
    }
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn quiet() -> Box<dyn Fn(Progress) + Send> {
    Box::new(|_| {})
}

fn end(rx: Receiver<Result<(), Failure>>) -> Result<(), Failure> {
    rx.recv_timeout(Duration::from_secs(30))
        .expect("the download ended")
}

fn status(s: &Store) -> Vec<ItemStatus> {
    s.status()
        .recv_timeout(Duration::from_secs(30))
        .expect("status")
}

fn until(cond: impl Fn() -> bool) {
    let t0 = Instant::now();
    while !cond() {
        assert!(t0.elapsed() < Duration::from_secs(4), "waited too long");
        thread::sleep(Duration::from_millis(5));
    }
}

type UnzipCall = (PathBuf, PathBuf, String, bool);
type Seen = Arc<Mutex<Vec<Progress>>>;
type Listen = Box<dyn Fn(Progress) + Send>;

fn recorder() -> (Seen, Listen) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s2 = seen.clone();
    (seen, Box::new(move |p| s2.lock().unwrap().push(p)))
}

fn st(id: &str, state: ItemState) -> ItemStatus {
    ItemStatus {
        id: id.into(),
        state,
    }
}

fn pack_dir(root: &Path, e: &Item) -> PathBuf {
    root.join("engines")
        .join(format!("{}-{}", e.id, &e.sha256[..12]))
}

/// What the official zip looks like once open: the server is under Release/.
fn fake_unzip_into(out: &Path) {
    fs::create_dir_all(out.join("Release")).unwrap();
    fs::write(
        out.join("Release")
            .join(flare_dictation::engine::WHISPER_SERVER),
        "MZ",
    )
    .unwrap();
    fs::write(out.join("Release").join("ggml-cuda.dll"), "MZ").unwrap();
}

fn fake_unzip() -> Unzip {
    Arc::new(|_zip, out, _c| {
        fake_unzip_into(out);
        Ok(())
    })
}

fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

// ---- the dictation store: a model ----

#[test]
fn reports_every_catalog_entry_as_absent_in_a_root_that_does_not_exist_yet() {
    let fx = Fx::new();
    let s = fx.store(vec![fx.model("base", "/good"), fx.pack("/pack", PACK)]);
    assert_eq!(
        status(&s),
        vec![st("base", ItemState::Absent), st("gpu", ItemState::Absent)]
    );
    assert_eq!(s.model_path("base"), None);
    assert_eq!(s.gpu_engine_dir(), None);
}

#[test]
fn downloads_verifies_and_renames_into_models_id_bin_creating_the_folders() {
    let fx = Fx::new();
    let s = fx.store(vec![fx.model("base", "/good")]);
    assert_eq!(end(s.download("base", quiet())), Ok(()));
    let file = fx.root.join("models").join("base.bin");
    assert_eq!(s.model_path("base"), Some(file.clone()));
    assert_eq!(sha(&fs::read(&file).unwrap()), sha(&fx.good));
    assert!(!file.with_extension("bin.part").exists());
    assert_eq!(status(&s), vec![st("base", ItemState::Installed)]);
}

#[test]
fn does_not_fetch_again_what_is_already_installed() {
    let fx = Fx::new();
    let s = fx.store(vec![fx.model("base", "/good")]);
    assert_eq!(end(s.download("base", quiet())), Ok(()));
    assert_eq!(end(s.download("base", quiet())), Ok(()));
    assert_eq!(fx.mem.hits("/good"), 1);
}

#[test]
fn reports_progress_at_most_every_100_ms_plus_a_final_one_that_says_it_all_arrived() {
    // A clock that never moves: only the first chunk and the final report get out.
    let fx = Fx::new();
    let frozen = fx.store_with(
        vec![fx.model("base", "/good")],
        Some(Arc::new(|| 5000)),
        None,
    );
    let (seen, on) = recorder();
    assert_eq!(end(frozen.download("base", on)), Ok(()));
    let seen = seen.lock().unwrap().clone();
    assert!((1..=2).contains(&seen.len()), "{}", seen.len());
    let len = fx.good.len() as u64;
    assert_eq!(
        seen.last().unwrap(),
        &Progress {
            id: "base".into(),
            done: len,
            total: len
        }
    );

    // A clock that jumps a second per look: every chunk is reported, in order.
    let fx = Fx::new();
    let t = Arc::new(AtomicU64::new(0));
    let t2 = t.clone();
    let moving = fx.store_with(
        vec![fx.model("base", "/good")],
        Some(Arc::new(move || {
            t2.fetch_add(1000, Ordering::SeqCst) + 1000
        })),
        None,
    );
    let (many, on) = recorder();
    assert_eq!(end(moving.download("base", on)), Ok(()));
    let many: Vec<u64> = many.lock().unwrap().iter().map(|p| p.done).collect();
    assert!(many.len() > 2);
    let mut sorted = many.clone();
    sorted.sort_unstable();
    assert_eq!(many, sorted);
    assert_eq!(*many.last().unwrap(), len);
    sorted.dedup();
    assert_eq!(sorted.len(), many.len());
}

#[test]
fn survives_a_progress_listener_that_throws() {
    let fx = Fx::new();
    let s = fx.store(vec![fx.model("base", "/good")]);
    let r = end(s.download("base", Box::new(|_| panic!("a window went away"))));
    assert_eq!(r, Ok(()));
}

#[test]
fn deletes_a_file_whose_sha256_does_not_match_and_says_checksum() {
    let fx = Fx::new();
    // The right length, the wrong bytes.
    let s = fx.store(vec![fx.model("base", "/corrupt")]);
    assert_eq!(end(s.download("base", quiet())), Err(Failure::Checksum));
    assert!(fx.models().is_empty());
    assert_eq!(s.model_path("base"), None);
    assert_eq!(status(&s)[0].state, ItemState::Absent);
}

#[test]
fn compares_the_byte_count_as_well_as_the_hash() {
    let fx = Fx::new();
    // The hash of what is served is right; the size the catalogue promises is not.
    let short = fx.good[..fx.good.len() / 2].to_vec();
    let mut too_few = fx.item("base", "/short", &short, Kind::Model);
    too_few.bytes = fx.good.len() as u64;
    let a = fx.store(vec![too_few]);
    assert_eq!(end(a.download("base", quiet())), Err(Failure::Checksum));
    assert!(fx.models().is_empty());

    // And a body LONGER than promised is cut off rather than written to the end.
    let mut too_many = fx.model("base", "/good");
    too_many.bytes = 100;
    let b = fx.store(vec![too_many]);
    assert_eq!(end(b.download("base", quiet())), Err(Failure::Checksum));
    assert!(fx.models().is_empty());
}

#[test]
fn answers_network_for_a_404_a_refused_connection_and_a_line_that_drops() {
    let fx = Fx::new();
    let s = fx.store(vec![
        fx.model("gone", "/nothing-here"),
        fx.model("refused", "/refused"),
        fx.model("dropped", "/drop"),
    ]);
    assert_eq!(end(s.download("gone", quiet())), Err(Failure::Network));
    assert_eq!(end(s.download("refused", quiet())), Err(Failure::Network));
    assert_eq!(end(s.download("dropped", quiet())), Err(Failure::Network));
    // One ask each: a failure is reported, never retried behind the user's back.
    assert_eq!(fx.mem.hits("/nothing-here"), 1);
    assert_eq!(fx.mem.hits("/drop"), 1);
    assert!(fx.models().is_empty());
}

// REPLACED: PT checks the `fetch` it was handed gets a signal and no `redirect` option. Flare has no
// signal object: the sink's `false` is the signal (covered by the cancel tests), and redirects are the
// fetch's own business. What is held here is that the store asks the catalogue's url, once.
#[test]
fn fetches_through_the_fetch_it_was_given_with_a_signal_and_nothing_else() {
    let fx = Fx::new();
    fx.mem.routes.lock().unwrap().insert(
        "https://models.invalid/base.bin".into(),
        Route::Body(fx.good.clone()),
    );
    let s = fx.store(vec![fx.model("base", "https://models.invalid/base.bin")]);
    assert_eq!(end(s.download("base", quiet())), Ok(()));
    assert_eq!(
        *fx.mem.asked.lock().unwrap(),
        vec!["https://models.invalid/base.bin".to_string()]
    );
}

#[test]
fn answers_network_for_an_id_the_catalog_does_not_hold() {
    let fx = Fx::new();
    let s = fx.store(vec![]);
    assert_eq!(end(s.download("nope", quiet())), Err(Failure::Network));
    assert_eq!(s.model_path("nope"), None);
    s.remove("nope")
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    s.cancel("nope");
}

#[test]
fn answers_disk_when_it_cannot_write() {
    let fx = Fx::new();
    // A FILE where the models folder has to go.
    fs::create_dir_all(&fx.root).unwrap();
    fs::write(fx.root.join("models"), "in the way").unwrap();
    let s = fx.store(vec![fx.model("base", "/good")]);
    assert_eq!(end(s.download("base", quiet())), Err(Failure::Disk));
}

#[test]
fn shares_one_download_between_two_asks_shows_the_bytes_in_status_and_cancels_cleanly() {
    let fx = Fx::new();
    let s = fx.store(vec![fx.model("base", "/slow")]);
    let (first, on1) = recorder();
    let (_second, on2) = recorder();
    let p1 = s.download("base", on1);
    let p2 = s.download("base", on2);
    until(|| !first.lock().unwrap().is_empty());
    assert_eq!(fx.mem.hits("/slow"), 1);
    assert!(fx.root.join("models").join("base.bin.part").exists());
    let ItemState::Downloading { done, total } = status(&s)[0].state.clone() else {
        panic!("not downloading");
    };
    assert!(done > 0 && done < total);
    // The part file of a download in flight is not a leftover.
    assert!(fx.root.join("models").join("base.bin.part").exists());

    s.cancel("base");
    assert_eq!(end(p1), Err(Failure::Cancelled));
    assert_eq!(end(p2), Err(Failure::Cancelled));
    assert!(fx.models().is_empty());
    assert_eq!(status(&s), vec![st("base", ItemState::Absent)]);
}

#[test]
fn starts_afresh_when_asked_again_straight_after_a_cancel() {
    let fx = Fx::new();
    let s = fx.store(vec![fx.model("base", "/slow")]);
    let (seen, on) = recorder();
    let p1 = s.download("base", on);
    until(|| !seen.lock().unwrap().is_empty());
    s.cancel("base");
    // The cancelled flight may still be tidying up: it is not the one handed back.
    let p2 = s.download("base", quiet());
    assert_eq!(end(p1), Err(Failure::Cancelled));
    until(|| fx.mem.hits("/slow") == 2);
    s.cancel("base");
    assert_eq!(end(p2), Err(Failure::Cancelled));
}

#[test]
fn calls_a_wrong_size_file_absent_and_will_not_hand_out_its_path() {
    let fx = Fx::new();
    fs::create_dir_all(fx.root.join("models")).unwrap();
    fs::write(fx.root.join("models").join("base.bin"), "far too small").unwrap();
    let s = fx.store(vec![fx.model("base", "/good")]);
    assert_eq!(status(&s), vec![st("base", ItemState::Absent)]);
    assert_eq!(s.model_path("base"), None);
    // And a download puts the real one over it.
    assert_eq!(end(s.download("base", quiet())), Ok(()));
    assert_eq!(
        s.model_path("base"),
        Some(fx.root.join("models").join("base.bin"))
    );
}

#[test]
fn removes_a_leftover_part_file_nobody_is_writing_but_not_one_still_warm() {
    let fx = Fx::new();
    let part = fx.root.join("models").join("base.bin.part");
    fs::create_dir_all(part.parent().unwrap()).unwrap();
    fs::write(&part, "half a model").unwrap();
    let now = epoch_ms();
    let s = fx.store_with(
        vec![fx.model("base", "/good")],
        Some(Arc::new(move || now)),
        None,
    );
    // Written this second: another copy may be the one downloading it.
    assert_eq!(status(&s), vec![st("base", ItemState::Absent)]);
    assert!(part.exists());
    let old = SystemTime::now() - Duration::from_millis(STALE_PART_MS + 1000);
    fs::OpenOptions::new()
        .write(true)
        .open(&part)
        .unwrap()
        .set_modified(old)
        .unwrap();
    assert_eq!(status(&s), vec![st("base", ItemState::Absent)]);
    assert!(!part.exists());
}

#[test]
fn removes_an_installed_model_and_a_download_in_flight() {
    let fx = Fx::new();
    let s = fx.store(vec![fx.model("base", "/good"), fx.model("slow", "/slow")]);
    assert_eq!(end(s.download("base", quiet())), Ok(()));
    s.remove("base")
        .recv_timeout(Duration::from_secs(30))
        .unwrap()
        .unwrap();
    assert_eq!(s.model_path("base"), None);
    assert!(!fx.root.join("models").join("base.bin").exists());

    let (seen, on) = recorder();
    let p = s.download("slow", on);
    until(|| !seen.lock().unwrap().is_empty());
    s.remove("slow")
        .recv_timeout(Duration::from_secs(30))
        .unwrap()
        .unwrap();
    assert_eq!(end(p), Err(Failure::Cancelled));
    assert!(fx.models().is_empty());
}

// Perf review 2026-10-16: a stat for every dictation pass.
#[test]
fn believes_an_installed_model_for_30_s_a_missing_one_never_and_forgets_on_remove_and_status() {
    let fx = Fx::new();
    let t = Arc::new(AtomicU64::new(1_000_000));
    let t2 = t.clone();
    let s = fx.store_with(
        vec![fx.model("base", "/good")],
        Some(Arc::new(move || t2.load(Ordering::SeqCst))),
        None,
    );
    let file = fx.root.join("models").join("base.bin");
    assert_eq!(s.model_path("base"), None);
    // Put there by another copy: seen at once, a "no" is never kept.
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, &fx.good).unwrap();
    assert_eq!(s.model_path("base"), Some(file.clone()));
    // Gone behind the store's back: the disk is not asked again for 30 s.
    fs::remove_file(&file).unwrap();
    t.fetch_add(10_000, Ordering::SeqCst);
    assert_eq!(s.model_path("base"), Some(file.clone()));
    t.fetch_add(20_000, Ordering::SeqCst);
    assert_eq!(s.model_path("base"), None);
    // status() and remove() look again at once.
    fs::write(&file, &fx.good).unwrap();
    assert_eq!(s.model_path("base"), Some(file.clone()));
    fs::remove_file(&file).unwrap();
    assert_eq!(status(&s), vec![st("base", ItemState::Absent)]);
    assert_eq!(s.model_path("base"), None);
    fs::write(&file, &fx.good).unwrap();
    assert_eq!(s.model_path("base"), Some(file.clone()));
    s.remove("base")
        .recv_timeout(Duration::from_secs(30))
        .unwrap()
        .unwrap();
    assert_eq!(s.model_path("base"), None);
}

#[test]
fn refuses_a_catalog_id_that_could_walk_out_of_the_folder() {
    let fx = Fx::new();
    for bad in ["..\\evil", "../evil", "", ".hidden", "a b", "a/b"] {
        let r = Store::with_config(StoreConfig {
            root: fx.root.clone(),
            fetch: Box::new(MemFetch(fx.mem.clone())),
            catalog: vec![fx.model(bad, "/good")],
            now: None,
            unzip: None,
        });
        assert!(r.is_err(), "{bad:?}");
    }
}

// ---- the dictation store: the GPU pack ----

#[test]
fn downloads_the_zip_verifies_it_then_unzips_deletes_the_zip_and_writes_the_marker_last() {
    let fx = Fx::new();
    let e = fx.pack("/pack", PACK);
    let calls: Arc<Mutex<Vec<UnzipCall>>> = Arc::default();
    let c2 = calls.clone();
    let unzip: Unzip = Arc::new(move |zip, out, _| {
        let zip_sha = sha(&fs::read(zip).unwrap());
        c2.lock().unwrap().push((
            zip.to_path_buf(),
            out.to_path_buf(),
            zip_sha,
            out.join(PACK_MARKER).exists(),
        ));
        fake_unzip_into(out);
        Ok(())
    });
    let s = fx.store_with(vec![e.clone()], None, Some(unzip));
    assert_eq!(end(s.download("gpu", quiet())), Ok(()));
    assert_eq!(
        *calls.lock().unwrap(),
        vec![(
            fx.root.join("engines").join("gpu.zip"),
            pack_dir(&fx.root, &e),
            e.sha256.clone(),
            false
        )]
    );
    assert_eq!(fx.engines(), vec![format!("gpu-{}", &e.sha256[..12])]);
    assert!(pack_dir(&fx.root, &e).join(PACK_MARKER).exists());
    assert_eq!(
        s.gpu_engine_dir(),
        Some(pack_dir(&fx.root, &e).join("Release"))
    );
    assert_eq!(status(&s), vec![st("gpu", ItemState::Installed)]);
    // A pack is not a model.
    assert_eq!(s.model_path("gpu"), None);
}

#[test]
fn never_unzips_a_zip_that_failed_its_checksum() {
    let fx = Fx::new();
    let n = Arc::new(AtomicU64::new(0));
    let n2 = n.clone();
    let unzip: Unzip = Arc::new(move |_, _, _| {
        n2.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    let mut e = fx.model("gpu", "/corrupt");
    e.kind = Kind::GpuPack;
    let s = fx.store_with(vec![e], None, Some(unzip));
    assert_eq!(end(s.download("gpu", quiet())), Err(Failure::Checksum));
    assert_eq!(n.load(Ordering::SeqCst), 0);
    assert!(fx.engines().is_empty());
}

#[test]
fn answers_unpack_and_cleans_up_when_the_unzip_fails_or_holds_no_server() {
    let fx = Fx::new();
    let e = fx.pack("/pack", PACK);
    let failing: Unzip = Arc::new(|_, out, _| {
        fs::create_dir_all(out.join("Release")).unwrap();
        fs::write(out.join("Release").join("half.dll"), "MZ").unwrap();
        Err("the archive is damaged".into())
    });
    let a = fx.store_with(vec![e.clone()], None, Some(failing));
    assert_eq!(end(a.download("gpu", quiet())), Err(Failure::Unpack));
    assert!(fx.engines().is_empty());
    assert_eq!(a.gpu_engine_dir(), None);

    let hollow: Unzip = Arc::new(|_, out, _| {
        fs::create_dir_all(out).unwrap();
        Ok(())
    });
    let b = fx.store_with(vec![e], None, Some(hollow));
    assert_eq!(end(b.download("gpu", quiet())), Err(Failure::Unpack));
    assert!(fx.engines().is_empty());
}

#[test]
fn does_not_call_a_half_unzipped_folder_installed() {
    let fx = Fx::new();
    let e = fx.pack("/pack", PACK);
    // What a crash in the middle of the unzip leaves: the server is there, the marker is not.
    fake_unzip_into(&pack_dir(&fx.root, &e));
    let s = fx.store_with(vec![e.clone()], None, Some(fake_unzip()));
    assert_eq!(status(&s), vec![st("gpu", ItemState::Absent)]);
    assert_eq!(s.gpu_engine_dir(), None);
    // A marker written for some OTHER size of pack does not count either.
    fs::write(
        pack_dir(&fx.root, &e).join(PACK_MARKER),
        format!("{{\"id\":\"gpu\",\"bytes\":1,\"sha256\":\"{}\"}}", e.sha256),
    )
    .unwrap();
    assert_eq!(status(&s)[0].state, ItemState::Absent);
    assert_eq!(s.gpu_engine_dir(), None);
    // And the next download starts from a clean folder.
    fs::write(pack_dir(&fx.root, &e).join("stale.txt"), "from the crash").unwrap();
    assert_eq!(end(s.download("gpu", quiet())), Ok(()));
    assert!(!pack_dir(&fx.root, &e).join("stale.txt").exists());
    assert_eq!(
        s.gpu_engine_dir(),
        Some(pack_dir(&fx.root, &e).join("Release"))
    );
}

#[test]
fn finds_the_server_at_the_top_and_two_levels_down_and_no_deeper() {
    let at = |rel: &[&str]| -> Option<PathBuf> {
        let fx = Fx::new();
        let e = fx.pack("/pack", PACK);
        let rel: PathBuf = rel.iter().collect();
        let rel2 = rel.clone();
        let unzip: Unzip = Arc::new(move |_, out, _| {
            fs::create_dir_all(out.join(&rel2)).unwrap();
            fs::write(
                out.join(&rel2)
                    .join(flare_dictation::engine::WHISPER_SERVER),
                "MZ",
            )
            .unwrap();
            Ok(())
        });
        let s = fx.store_with(vec![e.clone()], None, Some(unzip));
        let r = end(s.download("gpu", quiet()));
        let dir = s.gpu_engine_dir();
        match &dir {
            Some(d) => assert_eq!(d, &pack_dir(&fx.root, &e).join(&rel)),
            None => assert_eq!(r, Err(Failure::Unpack)),
        }
        dir
    };
    assert!(at(&[]).is_some());
    assert!(at(&["Release"]).is_some());
    assert!(at(&["build", "Release"]).is_some());
    assert!(at(&["a", "b", "c"]).is_none());
}

#[test]
fn cancels_during_the_download_leaving_no_zip_behind() {
    let fx = Fx::new();
    let mut e = fx.model("gpu", "/slow");
    e.kind = Kind::GpuPack;
    let s = fx.store_with(vec![e], None, Some(fake_unzip()));
    let (seen, on) = recorder();
    let p = s.download("gpu", on);
    until(|| !seen.lock().unwrap().is_empty());
    assert!(fx.root.join("engines").join("gpu.zip.part").exists());
    s.cancel("gpu");
    assert_eq!(end(p), Err(Failure::Cancelled));
    assert!(fx.engines().is_empty());
}

#[test]
fn the_real_fetch_obeys_cancel_and_remove_at_once_while_the_line_stalls() {
    // Unlike the in-memory stand-in (which pings the sink itself), the real fetch is blocked in a read.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        use std::io::{Read, Write};
        while let Ok((mut c, _)) = listener.accept() {
            thread::spawn(move || {
                let mut b = [0u8; 2048];
                let _ = c.read(&mut b);
                let _ = c.write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-length: 524288\r\nconnection: close\r\n\r\n",
                );
                let _ = c.write_all(&[0u8; 4096]);
                thread::sleep(Duration::from_secs(30));
            });
        }
    });
    let fx = Fx::new();
    let e = fx.model("base", &format!("http://127.0.0.1:{port}/slow"));
    let s = Store::with_config(StoreConfig {
        root: fx.root.clone(),
        fetch: Box::new(net::UreqFetch::new()),
        catalog: vec![e],
        now: None,
        unzip: None,
    })
    .unwrap();
    let t = Instant::now();
    let (seen, on) = recorder();
    let p = s.download("base", on);
    until(|| !seen.lock().unwrap().is_empty());
    s.cancel("base");
    assert_eq!(end(p), Err(Failure::Cancelled));
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());

    let (seen, on) = recorder();
    let p = s.download("base", on);
    until(|| !seen.lock().unwrap().is_empty());
    let t = Instant::now();
    s.remove("base")
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap();
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    assert_eq!(end(p), Err(Failure::Cancelled));
    assert!(fx.models().is_empty());
}
#[test]
fn cancels_during_the_unzip_the_signal_reaches_it_and_nothing_is_left_installed() {
    let fx = Fx::new();
    let started = Arc::new(AtomicBool::new(false));
    let s2 = started.clone();
    let unzip: Unzip = Arc::new(move |_, out, cancel| {
        fs::create_dir_all(out).unwrap();
        s2.store(true, Ordering::SeqCst);
        while !cancel.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(5));
        }
        Err("aborted".into())
    });
    let s = fx.store_with(vec![fx.pack("/pack", PACK)], None, Some(unzip));
    let p = s.download("gpu", quiet());
    until(|| started.load(Ordering::SeqCst));
    assert_eq!(
        status(&s)[0],
        st(
            "gpu",
            ItemState::Downloading {
                done: PACK.len() as u64,
                total: PACK.len() as u64
            }
        )
    );
    s.cancel("gpu");
    assert_eq!(end(p), Err(Failure::Cancelled));
    assert!(fx.engines().is_empty());
}

#[test]
fn removes_the_pack_and_the_folder_of_a_pack_the_catalog_has_moved_on_from() {
    let fx = Fx::new();
    let e = fx.pack("/pack", PACK);
    let old = fx.root.join("engines").join("gpu-0123456789ab");
    fake_unzip_into(&old);
    let keep = fx.root.join("engines").join("gpu-notes");
    fs::create_dir_all(&keep).unwrap();
    let s = fx.store_with(vec![e], None, Some(fake_unzip()));
    assert_eq!(end(s.download("gpu", quiet())), Ok(()));
    // Installing the new pack is what retires the old one.
    assert!(!old.exists());
    s.remove("gpu")
        .recv_timeout(Duration::from_secs(30))
        .unwrap()
        .unwrap();
    assert_eq!(s.gpu_engine_dir(), None);
    assert_eq!(status(&s)[0].state, ItemState::Absent);
    // Only what the store made is the store's to delete.
    assert_eq!(fx.engines(), vec!["gpu-notes".to_string()]);
}

// REPLACED: PT runs Windows PowerShell's Expand-Archive; Flare unzips with the `zip` crate (D5). Same
// test, same shape: a real zip, a folder with a space in it.
#[test]
fn unzips_for_real_with_expand_archive_in_a_path_with_a_space() {
    use std::io::Write;
    let mut cur = std::io::Cursor::new(Vec::new());
    {
        let mut w = zip::ZipWriter::new(&mut cur);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        w.start_file(
            format!("Release/{}", flare_dictation::engine::WHISPER_SERVER),
            opts,
        )
        .unwrap();
        w.write_all(b"MZ pretend").unwrap();
        w.finish().unwrap();
    }
    let real_zip = cur.into_inner();
    let fx = Fx::new();
    fx.mem
        .routes
        .lock()
        .unwrap()
        .insert("/realzip".into(), Route::Body(real_zip.clone()));
    let e = fx.pack("/realzip", &real_zip);
    let s = fx.store(vec![e.clone()]);
    assert_eq!(end(s.download("gpu", quiet())), Ok(()));
    assert_eq!(
        s.gpu_engine_dir(),
        Some(pack_dir(&fx.root, &e).join("Release"))
    );
    assert_eq!(fx.engines(), vec![format!("gpu-{}", &e.sha256[..12])]);
}

// ---- Flare's own ----

fn other_root_with(fx: &Fx, body: &[u8]) -> PathBuf {
    let prism = fx._holder.path().join("PrismDictation");
    fs::create_dir_all(prism.join("models")).unwrap();
    fs::write(prism.join("models").join("base.bin"), body).unwrap();
    prism
}

fn tree(root: &Path) -> Vec<(PathBuf, u64, SystemTime)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap().filter_map(Result::ok) {
            let md = e.metadata().unwrap();
            // A folder's own timestamp is updated lazily by NTFS (a stat right after the write can
            // still show the old one), so only names are held for folders; files keep size and mtime.
            if md.is_dir() {
                out.push((e.path(), 0, UNIX_EPOCH));
            } else {
                out.push((e.path(), md.len(), md.modified().unwrap()));
            }
            if md.is_dir() {
                stack.push(e.path());
            }
        }
    }
    out.sort();
    out
}

#[test]
fn reuse_links_a_matching_prism_model_and_never_writes_there() {
    let fx = Fx::new();
    let prism = other_root_with(&fx, &fx.good);
    let before = tree(&prism);
    let s = fx.store(vec![fx.model("base", "/good"), fx.model("small", "/good")]);
    let reused = s
        .reuse_models_from(prism.clone())
        .recv_timeout(Duration::from_secs(30))
        .unwrap();
    assert_eq!(reused, vec!["base".to_string()]);
    let mine = fx.root.join("models").join("base.bin");
    assert_eq!(s.model_path("base"), Some(mine.clone()));
    assert_eq!(fs::read(&mine).unwrap(), fx.good);
    assert_eq!(s.model_path("small"), None);
    assert_eq!(tree(&prism), before, "Prism's folder is untouched");
    // Deleting Flare's copy leaves Prism's alone.
    s.remove("base")
        .recv_timeout(Duration::from_secs(30))
        .unwrap()
        .unwrap();
    assert_eq!(
        fs::read(prism.join("models").join("base.bin")).unwrap(),
        fx.good
    );
    assert_eq!(tree(&prism), before);
    assert_eq!(fx.mem.hits("/good"), 0, "reusing downloads nothing");
}

#[test]
fn reuse_skips_a_model_whose_hash_differs() {
    let fx = Fx::new();
    let mut other = fx.good.clone();
    other[1000] ^= 0xff; // the right size, the wrong bytes
    let prism = other_root_with(&fx, &other);
    let s = fx.store(vec![fx.model("base", "/good")]);
    let reused = s
        .reuse_models_from(prism)
        .recv_timeout(Duration::from_secs(30))
        .unwrap();
    assert!(reused.is_empty());
    assert_eq!(s.model_path("base"), None);
    assert!(fx.models().is_empty());
}

#[test]
fn nothing_is_downloaded_in_a_test() {
    // A scan: no `ureq` call outside fetch_ureq.rs (the needle is built so this file does not match itself).
    let needle = format!("{}{}::", "ur", "eq");
    let base = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut stack = vec![base.join("src"), base.join("tests")];
    let mut scanned = 0;
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap().filter_map(Result::ok) {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                scanned += 1;
                if p.file_name().is_some_and(|n| n == "fetch_ureq.rs") {
                    continue;
                }
                let text = fs::read_to_string(&p).unwrap();
                assert!(!text.contains(&needle), "{} calls ureq", p.display());
            }
        }
    }
    assert!(scanned > 10);
}

#[test]
fn the_shipped_catalogue_opens_and_the_failure_texts_are_the_parity_line() {
    let fx = Fx::new();
    let s = Store::open(fx.root.clone(), Box::new(MemFetch(fx.mem.clone())));
    let ids: Vec<String> = status(&s).into_iter().map(|i| i.id).collect();
    // The pack is offered on Windows only (rule 1082): a Mac's engine has Metal in it.
    assert_eq!(ids.len(), if cfg!(windows) { 8 } else { 7 });
    assert_eq!(ids.contains(&"gpu-pack".to_string()), cfg!(windows));
    assert_eq!(Failure::Network.text(), "The download failed.");
    assert_eq!(
        Failure::Checksum.text(),
        "The file failed its check and was deleted."
    );
    assert_eq!(Failure::Disk.text(), "It could not be written to disk.");
    assert_eq!(Failure::Unpack.text(), "It could not be unpacked.");
}

// Audit 2026-10-20 L28: an uninstall is not undone by the next switch-on's reuse of Prism Terminal's copy.
#[test]
fn a_removed_model_is_not_reused_again_until_it_is_asked_for_again() {
    let fx = Fx::new();
    let prism = other_root_with(&fx, &fx.good);
    let s = fx.store(vec![fx.model("base", "/good")]);
    let first = s
        .reuse_models_from(prism.clone())
        .recv_timeout(Duration::from_secs(30))
        .unwrap();
    assert_eq!(first, vec!["base".to_string()]);
    assert_eq!(
        s.remove("base")
            .recv_timeout(Duration::from_secs(30))
            .unwrap(),
        Ok(())
    );
    let again = s
        .reuse_models_from(prism.clone())
        .recv_timeout(Duration::from_secs(30))
        .unwrap();
    assert!(again.is_empty(), "the tombstone holds across switch-ons");
    assert_eq!(s.model_path("base"), None);
    // A fresh store over the same root (the next run) honours it too.
    let s2 = fx.store(vec![fx.model("base", "/good")]);
    assert!(s2
        .reuse_models_from(prism)
        .recv_timeout(Duration::from_secs(30))
        .unwrap()
        .is_empty());
    // Downloading it again clears the tombstone.
    assert_eq!(end(s2.download("base", quiet())), Ok(()));
    assert!(s2.model_path("base").is_some());
}

// Audit 2026-10-20 L29: a file still held (the killed engine) is waited for, and a failure is told.
#[cfg(windows)]
#[test]
fn remove_waits_for_a_held_file_and_reports_one_that_stays_held() {
    use std::os::windows::fs::OpenOptionsExt;
    let fx = Fx::new();
    let s = fx.store(vec![fx.model("base", "/good")]);
    assert_eq!(end(s.download("base", quiet())), Ok(()));
    let file = fx.root.join("models").join("base.bin");
    let hold = || {
        fs::OpenOptions::new()
            .read(true)
            .share_mode(1) // FILE_SHARE_READ only: no delete while open
            .open(&file)
            .unwrap()
    };
    // Let go after a moment: remove waits and succeeds.
    let held = hold();
    let rx = s.remove("base");
    thread::sleep(Duration::from_millis(400));
    drop(held);
    assert_eq!(rx.recv_timeout(Duration::from_secs(30)).unwrap(), Ok(()));
    assert!(!file.exists());
    // Held for good: the answer is an error, not silence.
    assert_eq!(end(s.download("base", quiet())), Ok(()));
    s.set_remove_wait(Duration::from_millis(300));
    let held = hold();
    assert_eq!(
        s.remove("base")
            .recv_timeout(Duration::from_secs(30))
            .unwrap(),
        Err(Failure::Disk)
    );
    drop(held);
}

// macOS (port phase 7, rule 1085): a file Flare fetches is data it wrote itself, so it carries no quarantine flag
// and Gatekeeper is never asked about it; and a model is never made a program.
#[cfg(target_os = "macos")]
#[test]
fn a_fetched_model_carries_no_quarantine_flag_and_is_not_executable() {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;
    let fx = Fx::new();
    let s = fx.store(vec![fx.model("base", "/good")]);
    assert_eq!(end(s.download("base", quiet())), Ok(()));
    let file = fx.root.join("models").join("base.bin");
    let path = std::ffi::CString::new(file.as_os_str().as_bytes()).unwrap();
    // SAFETY: both strings are live and NUL-terminated; a null buffer of size 0 only asks for the length.
    let got = unsafe {
        libc::getxattr(
            path.as_ptr(),
            c"com.apple.quarantine".as_ptr(),
            std::ptr::null_mut(),
            0,
            0,
            0,
        )
    };
    assert_eq!(got, -1, "the model carries a quarantine flag");
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ENOATTR)
    );
    let mode = fs::metadata(&file).unwrap().permissions().mode();
    assert_eq!(mode & 0o111, 0, "a model is never executable: {mode:o}");
}
