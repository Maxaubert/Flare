//! The once-only import run (M7 Task 26).
//!
//! Reads Prism Terminal's Local Storage (the Stable profile first, then the Main one) from a copy,
//! maps the keys with the table of `map.rs`, folds the legacy keys with `legacy.rs` and writes what
//! Flare does not hold yet. The run is recorded in `DONE_KEY`, so it happens once. Nothing under the
//! source folders is ever written. Call it from a worker thread: the whole import waits at most the
//! ceiling, shared by every candidate's read (M10 Task 53).

use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::import::ldb::{self, LocalStorage, ReadError};
use crate::import::legacy::{self, Source};
use crate::import::map::{self, Mapped};
use crate::kv::PrefStore;

pub const DONE_KEY: &str = "flare.import.done";
pub const ATTEMPTS_KEY: &str = "flare.import.attempts";
pub const MAX_ATTEMPTS: u32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Stable,
    Main,
}

impl Origin {
    fn word(self) -> &'static str {
        match self {
            Origin::Stable => "stable",
            Origin::Main => "main",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Imported,
    Nothing,
    Retry,
    GaveUp,
    AlreadyDone,
}

pub struct ImportPaths {
    pub appdata: PathBuf,
    pub temp_root: PathBuf,
}

pub struct ImportReport {
    pub status: Status,
    pub origin: Option<Origin>,
    pub written: Vec<String>,
    pub skipped: Vec<(String, String)>,
    pub already_set: Vec<String>,
    pub retired: usize,
    pub unknown: usize,
    pub elapsed_ms: u64,
}

impl ImportReport {
    fn new(status: Status) -> Self {
        ImportReport {
            status,
            origin: None,
            written: Vec::new(),
            skipped: Vec::new(),
            already_set: Vec::new(),
            retired: 0,
            unknown: 0,
            elapsed_ms: 0,
        }
    }
}

/// PrismTerminalStable then PrismTerminal, each only when its folder exists.
pub fn candidates(appdata: &Path) -> Vec<(Origin, PathBuf)> {
    [
        (Origin::Stable, "PrismTerminalStable"),
        (Origin::Main, "PrismTerminal"),
    ]
    .into_iter()
    .map(|(origin, name)| (origin, appdata.join(name)))
    .filter(|(_, folder)| folder.is_dir()) // sync-ok: one stat per profile folder, on the import worker
    .collect()
}

/// True while `DONE_KEY` is absent.
pub fn should_run(prefs: &PrefStore) -> bool {
    prefs.get(DONE_KEY).is_none()
}

/// One budget for a whole import (M10 Task 53): every candidate's read gets what is left of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Deadline {
    end: Instant,
}

impl Deadline {
    pub fn after(d: Duration, now: Instant) -> Self {
        Deadline { end: now + d }
    }

    pub fn remaining(&self, now: Instant) -> Duration {
        self.end.saturating_duration_since(now)
    }

    pub fn expired(&self, now: Instant) -> bool {
        now >= self.end
    }
}

/// The ceiling for the next read: the time left on `d`, or None once it has run out.
pub fn next_ceiling(d: &Deadline, now: Instant) -> Option<Duration> {
    if d.expired(now) {
        None
    } else {
        Some(d.remaining(now))
    }
}

#[cfg(test)]
thread_local! {
    /// Each candidate's ceiling in order, for the budget tests (the import runs on the calling thread).
    static CEILINGS_SEEN: std::cell::RefCell<Vec<Duration>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn record_ceiling(c: Duration) {
    CEILINGS_SEEN.with(|s| s.borrow_mut().push(c));
}

#[cfg(not(test))]
fn record_ceiling(_: Duration) {}

/// Runs the whole import within `ceiling`, the budget shared by every Prism profile.
pub fn run_once(prefs: &PrefStore, paths: &ImportPaths, ceiling: Duration) -> ImportReport {
    run_once_with(prefs, paths, ceiling, Arc::new(real_reader))
}

pub type Reader = Arc<dyn Fn(&Path, &Path) -> Result<LocalStorage, ReadError> + Send + Sync>;

/// (profile, temp_root)
pub fn real_reader(profile: &Path, temp_root: &Path) -> Result<LocalStorage, ReadError> {
    ldb::read_local_storage(profile, temp_root, &|d| std::thread::sleep(d)) // sync-ok: import worker
}

/// What one candidate's read came to within the ceiling.
enum Got {
    Read(Result<LocalStorage, ReadError>),
    Late,
}

/// Runs the read on its own thread and waits at most `ceiling`. A late answer is dropped.
fn read_with_ceiling(
    reader: &Reader,
    profile: PathBuf,
    temp_root: PathBuf,
    ceiling: Duration,
) -> Got {
    let (tx, rx) = mpsc::channel();
    let reader = Arc::clone(reader);
    std::thread::spawn(move || {
        let _ = tx.send(reader(&profile, &temp_root));
    });
    match rx.recv_timeout(ceiling) {
        // sync-ok: the import worker waits for its own read
        Ok(answer) => Got::Read(answer),
        Err(_) => Got::Late,
    }
}

fn has_prism_keys(ls: &LocalStorage) -> bool {
    ls.values.keys().any(|k| k.starts_with(map::PREFIX))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn record_done(prefs: &PrefStore, body: serde_json::Value) {
    prefs.set(DONE_KEY, &body.to_string());
    prefs.flush();
}

pub fn run_once_with(
    prefs: &PrefStore,
    paths: &ImportPaths,
    ceiling: Duration,
    reader: Reader,
) -> ImportReport {
    let started = Instant::now();
    let mut report = run_inner(prefs, paths, ceiling, &reader);
    report.elapsed_ms = started.elapsed().as_millis() as u64;
    report
}

fn run_inner(
    prefs: &PrefStore,
    paths: &ImportPaths,
    ceiling: Duration,
    reader: &Reader,
) -> ImportReport {
    if !should_run(prefs) {
        return ImportReport::new(Status::AlreadyDone);
    }
    let mut winner: Option<(Origin, LocalStorage)> = None;
    let mut failed = false;
    let deadline = Deadline::after(ceiling, Instant::now());
    for (origin, profile) in candidates(&paths.appdata) {
        // An expired budget: this profile is not started, and the run counts as a timeout.
        let Some(left) = next_ceiling(&deadline, Instant::now()) else {
            failed = true;
            break;
        };
        record_ceiling(left);
        match read_with_ceiling(reader, profile, paths.temp_root.clone(), left) {
            Got::Read(Ok(ls)) if has_prism_keys(&ls) => {
                winner = Some((origin, ls));
                break;
            }
            Got::Read(Ok(_)) | Got::Read(Err(ReadError::NoFolder)) => continue,
            Got::Read(Err(_)) | Got::Late => {
                failed = true;
                break;
            }
        }
    }
    if failed {
        return retry_or_give_up(prefs);
    }
    match winner {
        None => {
            record_done(
                prefs,
                serde_json::json!({"v": 1, "status": "nothing", "at": now_secs()}),
            );
            ImportReport::new(Status::Nothing)
        }
        Some((origin, ls)) => import(prefs, origin, ls),
    }
}

fn retry_or_give_up(prefs: &PrefStore) -> ImportReport {
    let attempts = prefs
        .get(ATTEMPTS_KEY)
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    log::warn!(
        "import: a read failed or timed out (attempt {})",
        attempts + 1
    );
    if attempts >= MAX_ATTEMPTS {
        record_done(
            prefs,
            serde_json::json!({"v": 1, "status": "gave-up", "at": now_secs()}),
        );
        return ImportReport::new(Status::GaveUp);
    }
    prefs.set(ATTEMPTS_KEY, &(attempts + 1).to_string());
    prefs.flush();
    ImportReport::new(Status::Retry)
}

/// Maps a winning profile's keys onto Flare's store: writes what Flare does not hold, records the rest.
fn import(prefs: &PrefStore, origin: Origin, ls: LocalStorage) -> ImportReport {
    let mut report = ImportReport::new(Status::Imported);
    report.origin = Some(origin);
    let mut source: Source = ls
        .values
        .into_iter()
        .filter(|(k, _)| k.starts_with(map::PREFIX))
        .collect();
    for (name, why) in ls.unreadable {
        if name.starts_with(map::PREFIX) {
            report.skipped.push((name, why.to_string()));
        }
    }
    legacy::migrate(&mut source);
    for (from, raw) in &source {
        match map::map_one(from, raw) {
            Mapped::Write { key, value } => {
                if prefs.get(key).is_some() {
                    report.already_set.push(key.to_string());
                } else {
                    prefs.set(key, &value);
                    report.written.push(key.to_string());
                }
            }
            Mapped::Skip { from, reason } => report.skipped.push((from, reason.to_string())),
            Mapped::Legacy => {}
            Mapped::Retired(_) | Mapped::Dropped(_) => report.retired += 1,
            Mapped::Unknown => report.unknown += 1,
        }
    }
    prefs.flush();
    record_done(
        prefs,
        serde_json::json!({
            "v": 1,
            "status": "imported",
            "origin": origin.word(),
            "written": report.written.len(),
            "skipped": report.skipped,
            "at": now_secs(),
        }),
    );
    log::info!(
        "import from {}: written {:?}, already set {:?}, skipped {:?}, retired {}, unknown {}",
        origin.word(),
        report.written,
        report.already_set,
        report.skipped.iter().map(|(k, _)| k).collect::<Vec<_>>(),
        report.retired,
        report.unknown
    );
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kv::PrefStore;
    use rusty_leveldb::compressor::SnappyCompressor;
    use rusty_leveldb::{CompressorId, DB, Options};
    use std::collections::BTreeMap;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Mutex, Once};
    use std::time::SystemTime;

    fn entry_key(name: &str) -> Vec<u8> {
        let mut k = b"_file://\x00\x01".to_vec();
        k.extend_from_slice(name.as_bytes());
        k
    }

    fn latin1(s: &str) -> Vec<u8> {
        let mut v = vec![1u8];
        v.extend(s.bytes());
        v
    }

    /// A Prism Terminal profile folder with `entries` in its Local Storage.
    fn profile(appdata: &Path, folder: &str, entries: &[(&str, &str)]) {
        let ldb = appdata.join(folder).join("Local Storage").join("leveldb");
        fs::create_dir_all(&ldb).unwrap();
        let o = Options {
            create_if_missing: true,
            compressor: SnappyCompressor::ID,
            ..Options::default()
        };
        let mut db = DB::open(&ldb, o).unwrap();
        for (k, v) in entries {
            db.put(&entry_key(k), &latin1(v)).unwrap();
        }
        db.flush().unwrap();
        db.close().unwrap();
    }

    /// A profile whose manifest is junk: the copy will not open.
    fn torn(appdata: &Path, folder: &str) {
        let ldb = appdata.join(folder).join("Local Storage").join("leveldb");
        fs::create_dir_all(&ldb).unwrap();
        fs::write(ldb.join("CURRENT"), b"MANIFEST-000001\n").unwrap();
        fs::write(ldb.join("MANIFEST-000001"), b"junk junk junk").unwrap();
    }

    fn paths(appdata: &Path, temp: &Path) -> ImportPaths {
        ImportPaths {
            appdata: appdata.to_path_buf(),
            temp_root: temp.to_path_buf(),
        }
    }

    const CEILING: Duration = Duration::from_secs(10);

    #[test]
    fn stable_wins_when_it_has_keys() {
        let a = tempfile::tempdir().unwrap();
        let t = tempfile::tempdir().unwrap();
        profile(
            a.path(),
            "PrismTerminalStable",
            &[("prism.term.theme", "dracula")],
        );
        profile(
            a.path(),
            "PrismTerminal",
            &[("prism.term.theme", "pt-default")],
        );
        let prefs = PrefStore::memory();
        let r = run_once(&prefs, &paths(a.path(), t.path()), CEILING);
        assert_eq!(r.status, Status::Imported);
        assert_eq!(r.origin, Some(Origin::Stable));
        assert_eq!(prefs.get("flare.term.theme").as_deref(), Some("dracula"));
    }

    #[test]
    fn main_is_used_when_stable_is_absent_or_empty() {
        let a = tempfile::tempdir().unwrap();
        let t = tempfile::tempdir().unwrap();
        profile(a.path(), "PrismTerminalStable", &[("unrelated", "x")]);
        profile(
            a.path(),
            "PrismTerminal",
            &[("prism.term.theme", "dracula")],
        );
        let prefs = PrefStore::memory();
        let r = run_once(&prefs, &paths(a.path(), t.path()), CEILING);
        assert_eq!(r.status, Status::Imported);
        assert_eq!(r.origin, Some(Origin::Main));
        assert_eq!(prefs.get("flare.term.theme").as_deref(), Some("dracula"));
    }

    #[test]
    fn a_failing_stable_does_not_fall_through_to_main() {
        let a = tempfile::tempdir().unwrap();
        let t = tempfile::tempdir().unwrap();
        torn(a.path(), "PrismTerminalStable");
        profile(
            a.path(),
            "PrismTerminal",
            &[("prism.term.theme", "dracula")],
        );
        let prefs = PrefStore::memory();
        let r = run_once(&prefs, &paths(a.path(), t.path()), CEILING);
        assert_eq!(r.status, Status::Retry);
        assert!(r.written.is_empty());
        assert_eq!(prefs.get("flare.term.theme"), None);
        assert_eq!(prefs.get(ATTEMPTS_KEY).as_deref(), Some("1"));
        assert_eq!(prefs.get(DONE_KEY), None);
    }

    #[test]
    fn a_timeout_is_a_retry() {
        let a = tempfile::tempdir().unwrap();
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(a.path().join("PrismTerminalStable")).unwrap();
        let slow: Reader = Arc::new(|_: &Path, _: &Path| {
            std::thread::sleep(Duration::from_millis(300));
            Ok(LocalStorage::default())
        });
        let prefs = PrefStore::memory();
        let r = run_once_with(
            &prefs,
            &paths(a.path(), t.path()),
            Duration::from_millis(50),
            slow,
        );
        assert_eq!(r.status, Status::Retry);
        assert!(r.written.is_empty());
        assert_eq!(prefs.get(ATTEMPTS_KEY).as_deref(), Some("1"));
    }

    #[test]
    fn three_failures_give_up_and_record_it() {
        let a = tempfile::tempdir().unwrap();
        let t = tempfile::tempdir().unwrap();
        torn(a.path(), "PrismTerminalStable");
        let prefs = PrefStore::memory();
        for _ in 0..3 {
            assert_eq!(
                run_once(&prefs, &paths(a.path(), t.path()), CEILING).status,
                Status::Retry
            );
        }
        let r = run_once(&prefs, &paths(a.path(), t.path()), CEILING);
        assert_eq!(r.status, Status::GaveUp);
        let done = prefs.get(DONE_KEY).unwrap();
        assert!(done.contains("\"gave-up\""));
    }

    #[test]
    fn imports_once() {
        let a = tempfile::tempdir().unwrap();
        let t = tempfile::tempdir().unwrap();
        profile(
            a.path(),
            "PrismTerminalStable",
            &[("prism.term.theme", "dracula")],
        );
        let prefs = PrefStore::memory();
        assert!(should_run(&prefs));
        assert_eq!(
            run_once(&prefs, &paths(a.path(), t.path()), CEILING).status,
            Status::Imported
        );
        assert!(!should_run(&prefs));
        prefs.set("flare.term.theme", "nord");
        let again = run_once(&prefs, &paths(a.path(), t.path()), CEILING);
        assert_eq!(again.status, Status::AlreadyDone);
        assert!(again.written.is_empty());
        assert_eq!(prefs.get("flare.term.theme").as_deref(), Some("nord"));
    }

    #[test]
    fn nothing_to_import_is_done_too() {
        let a = tempfile::tempdir().unwrap();
        let t = tempfile::tempdir().unwrap();
        let prefs = PrefStore::memory();
        let r = run_once(&prefs, &paths(a.path(), t.path()), CEILING);
        assert_eq!(r.status, Status::Nothing);
        assert!(prefs.get(DONE_KEY).unwrap().contains("\"nothing\""));
    }

    #[test]
    fn existing_flare_keys_win() {
        let a = tempfile::tempdir().unwrap();
        let t = tempfile::tempdir().unwrap();
        profile(
            a.path(),
            "PrismTerminalStable",
            &[("prism.term.theme", "dracula")],
        );
        let prefs = PrefStore::memory();
        prefs.set("flare.term.theme", "nord");
        let r = run_once(&prefs, &paths(a.path(), t.path()), CEILING);
        assert_eq!(prefs.get("flare.term.theme").as_deref(), Some("nord"));
        assert!(r.already_set.contains(&"flare.term.theme".to_string()));
    }

    #[test]
    fn legacy_opacity_is_folded_and_the_legacy_keys_are_never_written() {
        let a = tempfile::tempdir().unwrap();
        let t = tempfile::tempdir().unwrap();
        profile(
            a.path(),
            "PrismTerminalStable",
            &[("prism.term.acrylic", "1"), ("prism.term.opacity", "60")],
        );
        let prefs = PrefStore::memory();
        run_once(&prefs, &paths(a.path(), t.path()), CEILING);
        assert!(prefs.get("flare.term.seeThrough").is_some());
        let dump = prefs.get(DONE_KEY).unwrap();
        assert!(!dump.contains("opacity") && !dump.contains("acrylic"));
        for key in ["flare.term.opacity", "flare.term.acrylic"] {
            assert_eq!(prefs.get(key), None);
        }
    }

    #[test]
    fn retired_and_dropped_keys_are_counted_not_written() {
        let a = tempfile::tempdir().unwrap();
        let t = tempfile::tempdir().unwrap();
        profile(
            a.path(),
            "PrismTerminalStable",
            &[
                ("prism.window.tabStyle", "x"),
                ("prism.window.tabWidth", "1"),
                ("prism.window.titleBar", "1"),
                ("prism.dictation.mic", "{}"),
            ],
        );
        let prefs = PrefStore::memory();
        let r = run_once(&prefs, &paths(a.path(), t.path()), CEILING);
        assert_eq!(r.retired, 4);
        assert!(
            r.written
                .iter()
                .all(|k| !k.contains("tab") && !k.contains("itle") && !k.contains("mic"))
        );
    }

    #[test]
    fn skipped_values_are_reported_with_reasons_and_nothing_is_guessed() {
        let a = tempfile::tempdir().unwrap();
        let t = tempfile::tempdir().unwrap();
        profile(
            a.path(),
            "PrismTerminalStable",
            &[("prism.window.edges", "diagonal")],
        );
        let prefs = PrefStore::memory();
        let r = run_once(&prefs, &paths(a.path(), t.path()), CEILING);
        assert!(
            r.skipped
                .iter()
                .any(|(k, why)| k == "prism.window.edges" && !why.is_empty())
        );
        assert_eq!(prefs.get("flare.window.edges"), None);
    }

    /// Every file under `dir`: relative path, length, modified time and content.
    fn tree(dir: &Path) -> BTreeMap<PathBuf, (u64, SystemTime, Vec<u8>)> {
        fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, (u64, SystemTime, Vec<u8>)>) {
            for e in fs::read_dir(dir).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    walk(root, &p, out);
                } else {
                    let m = fs::metadata(&p).unwrap();
                    let rel = p.strip_prefix(root).unwrap().to_path_buf();
                    out.insert(rel, (m.len(), m.modified().unwrap(), fs::read(&p).unwrap()));
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(dir, dir, &mut out);
        out
    }

    #[test]
    fn never_writes_into_prism_terminals_folders() {
        let a = tempfile::tempdir().unwrap();
        let t = tempfile::tempdir().unwrap();
        profile(
            a.path(),
            "PrismTerminalStable",
            &[("prism.term.theme", "dracula")],
        );
        profile(
            a.path(),
            "PrismTerminal",
            &[("prism.term.theme", "pt-default")],
        );
        let before = tree(a.path());
        let prefs = PrefStore::memory();
        run_once(&prefs, &paths(a.path(), t.path()), CEILING);
        assert_eq!(before, tree(a.path()));
    }

    #[test]
    fn leaves_no_temp_folder_behind() {
        let a = tempfile::tempdir().unwrap();
        let t = tempfile::tempdir().unwrap();
        profile(
            a.path(),
            "PrismTerminalStable",
            &[("prism.term.theme", "dracula")],
        );
        let prefs = PrefStore::memory();
        run_once(&prefs, &paths(a.path(), t.path()), CEILING);
        assert_eq!(fs::read_dir(t.path()).unwrap().count(), 0);
    }

    static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());
    struct Capture;
    impl log::Log for Capture {
        fn enabled(&self, _: &log::Metadata) -> bool {
            true
        }
        fn log(&self, record: &log::Record) {
            LOG.lock().unwrap().push(format!("{}", record.args()));
        }
        fn flush(&self) {}
    }
    static CAPTURE: Capture = Capture;
    static INSTALL: Once = Once::new();

    #[test]
    fn the_report_lists_keys_never_values() {
        INSTALL.call_once(|| {
            let _ = log::set_logger(&CAPTURE);
            log::set_max_level(log::LevelFilter::Trace);
        });
        let a = tempfile::tempdir().unwrap();
        let t = tempfile::tempdir().unwrap();
        profile(
            a.path(),
            "PrismTerminalStable",
            &[
                ("prism.term.theme", "dracula"),
                ("prism.window.edges", "SECRETdiagonal"),
            ],
        );
        let prefs = PrefStore::memory();
        run_once(&prefs, &paths(a.path(), t.path()), CEILING);
        let lines = LOG.lock().unwrap().clone();
        assert!(lines.iter().any(|l| l.contains("prism.window.edges")));
        assert!(lines.iter().all(|l| !l.contains("SECRETdiagonal")));
    }

    fn seen() -> Vec<Duration> {
        CEILINGS_SEEN.with(|s| s.borrow().clone())
    }

    /// Two profile folders, both present, with no Prism keys in either.
    fn two_empty_profiles(appdata: &Path) {
        fs::create_dir_all(appdata.join("PrismTerminalStable")).unwrap();
        fs::create_dir_all(appdata.join("PrismTerminal")).unwrap();
    }

    /// A reader whose first call sleeps `first` and answers "no Prism keys"; later calls answer at once.
    fn slow_then_empty(first: Duration, calls: Arc<AtomicUsize>) -> Reader {
        Arc::new(move |_: &Path, _: &Path| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                std::thread::sleep(first); // sync-ok: a test reader
            }
            Ok(LocalStorage::default())
        })
    }

    #[test]
    fn next_ceiling_shrinks_with_the_clock_and_ends() {
        let base = Instant::now();
        let d = Deadline::after(Duration::from_millis(1500), base);
        assert_eq!(next_ceiling(&d, base), Some(Duration::from_millis(1500)));
        assert_eq!(
            next_ceiling(&d, base + Duration::from_millis(1400)),
            Some(Duration::from_millis(100))
        );
        assert_eq!(next_ceiling(&d, base + Duration::from_millis(1500)), None);
    }

    #[test]
    fn two_profiles_share_one_budget() {
        let a = tempfile::tempdir().unwrap();
        let t = tempfile::tempdir().unwrap();
        two_empty_profiles(a.path());
        let calls = Arc::new(AtomicUsize::new(0));
        let reader = slow_then_empty(Duration::from_millis(300), calls.clone());
        CEILINGS_SEEN.with(|s| s.borrow_mut().clear());
        let prefs = PrefStore::memory();
        let r = run_once_with(
            &prefs,
            &paths(a.path(), t.path()),
            Duration::from_millis(400),
            reader,
        );
        let seen = seen();
        assert_eq!(seen.len(), 2, "both profiles are read");
        assert!(
            seen[1] <= Duration::from_millis(100),
            "the second profile got {:?}",
            seen[1]
        );
        assert_eq!(r.status, Status::Nothing);
    }

    #[test]
    fn an_expired_budget_skips_the_next_profile_and_counts_a_retry() {
        let a = tempfile::tempdir().unwrap();
        let t = tempfile::tempdir().unwrap();
        two_empty_profiles(a.path());
        let calls = Arc::new(AtomicUsize::new(0));
        let reader = slow_then_empty(Duration::from_millis(150), calls.clone());
        let prefs = PrefStore::memory();
        let r = run_once_with(
            &prefs,
            &paths(a.path(), t.path()),
            Duration::from_millis(100),
            reader,
        );
        assert_eq!(r.status, Status::Retry);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "the second profile never starts"
        );
        assert_eq!(prefs.get(ATTEMPTS_KEY).as_deref(), Some("1"));
    }

    #[test]
    fn the_whole_import_never_exceeds_the_budget_plus_a_margin() {
        let a = tempfile::tempdir().unwrap();
        let t = tempfile::tempdir().unwrap();
        two_empty_profiles(a.path());
        let slow: Reader = Arc::new(|_: &Path, _: &Path| {
            std::thread::sleep(Duration::from_millis(300)); // sync-ok: a test reader
            Ok(LocalStorage::default())
        });
        let prefs = PrefStore::memory();
        let started = Instant::now();
        let r = run_once_with(
            &prefs,
            &paths(a.path(), t.path()),
            Duration::from_millis(400),
            slow,
        );
        let took = started.elapsed();
        assert!(
            took < Duration::from_millis(550),
            "two slow profiles took {took:?} against a 400 ms budget"
        );
        assert_eq!(r.status, Status::Retry);
    }

    #[test]
    fn a_generated_50_mb_leveldb_reads_inside_the_budget_or_times_out() {
        let a = tempfile::tempdir().unwrap();
        let t = tempfile::tempdir().unwrap();
        let ldb = a
            .path()
            .join("PrismTerminalStable")
            .join("Local Storage")
            .join("leveldb");
        fs::create_dir_all(&ldb).unwrap();
        let o = Options {
            create_if_missing: true,
            compressor: SnappyCompressor::ID,
            ..Options::default()
        };
        let mut db = DB::open(&ldb, o).unwrap();
        let mut seed: u32 = 1;
        for i in 0..50 {
            let mut v = vec![1u8];
            // 1 MiB with its prefix byte: the reader's per-value cap is 1 MiB, so a larger value is skipped.
            v.extend((0..(1usize << 20) - 1).map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 24) as u8
            }));
            db.put(&entry_key(&format!("prism.big{i}")), &v).unwrap();
        }
        db.flush().unwrap();
        db.close().unwrap();
        let prefs = PrefStore::memory();
        let r = run_once(
            &prefs,
            &paths(a.path(), t.path()),
            Duration::from_millis(1500),
        );
        assert!(
            matches!(r.status, Status::Imported | Status::Retry),
            "got {:?}",
            r.status
        );
        assert!(
            r.elapsed_ms < 1900,
            "the 50 MB read took {} ms against a 1500 ms budget",
            r.elapsed_ms
        );
    }

    #[test]
    fn a_quick_import_uses_almost_none_of_the_budget() {
        let a = tempfile::tempdir().unwrap();
        let t = tempfile::tempdir().unwrap();
        profile(
            a.path(),
            "PrismTerminalStable",
            &[("prism.term.theme", "dracula")],
        );
        let prefs = PrefStore::memory();
        let r = run_once(
            &prefs,
            &paths(a.path(), t.path()),
            Duration::from_millis(1500),
        );
        assert_eq!(r.status, Status::Imported);
        assert!(
            r.elapsed_ms < 500,
            "a quick import took {} ms",
            r.elapsed_ms
        );
    }
}
