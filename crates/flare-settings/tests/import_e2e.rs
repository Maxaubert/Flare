//! The first-run import, end to end, on a copy of a Prism Terminal profile (M7 Task 50).
//!
//! Each test builds a Prism Terminal profile folder under a temp appdata with `rusty_leveldb`, runs
//! the real import (`run::run_once`) into a `PrefStore` over a real `FileBackend` (`settings.json`
//! in a temp profile, flushed and read back from disk), then reads the look with `LookStore`. No
//! window is opened, and nothing under a Prism Terminal folder is ever written.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use flare_settings::custom_theme::{parse_custom, serialize_custom};
use flare_settings::import::ldb;
use flare_settings::import::run::{self, ATTEMPTS_KEY, DONE_KEY, ImportPaths, Origin, Status};
use flare_settings::kv::{FileBackend, PrefStore};
use flare_settings::look::LookStore;
use flare_theme::colour::opaque;
use flare_theme::ground::DEFAULT_GROUND;
use flare_theme::preset::HOST_DEFAULT_THEME;
use flare_theme::resolve::resolve_saved_theme;
use rusty_leveldb::compressor::SnappyCompressor;
use rusty_leveldb::{CompressorId, DB, Options};

const CEILING: Duration = Duration::from_secs(10);

/// The Custom theme the owner's Stable profile holds (its shape, not its values, is the point).
const CUSTOM_JSON: &str = r##"{"bg":"#1a1b26","fg":"#c0caf5","cursor":"#c0caf5","selection":"#33467c","ansi":{"black":"#15161e","red":"#f7768e","green":"#9ece6a"},"accent":"#7aa2f7","seeThrough":0}"##;

/// Chromium's Local Storage key for one Prism Terminal setting: origin, NUL, a 1 (Latin-1), the name.
fn entry_key(name: &str) -> Vec<u8> {
    let mut k = b"_file://\x00\x01".to_vec();
    k.extend_from_slice(name.as_bytes());
    k
}

/// A value as Chromium stores it: a 1 byte (Latin-1), then the text.
fn latin1(s: &str) -> Vec<u8> {
    let mut v = vec![1u8];
    v.extend(s.bytes());
    v
}

/// A profile folder with `entries` (name, text) in its Local Storage. Returns the folder.
fn write_profile(appdata: &Path, folder: &str, entries: &[(&str, &str)]) -> PathBuf {
    let raw: Vec<(Vec<u8>, Vec<u8>)> = entries
        .iter()
        .map(|(k, v)| (entry_key(k), latin1(v)))
        .collect();
    write_profile_raw(appdata, folder, &raw)
}

/// As `write_profile`, with the raw key and value bytes (for a value no encoding reads).
fn write_profile_raw(appdata: &Path, folder: &str, entries: &[(Vec<u8>, Vec<u8>)]) -> PathBuf {
    let profile = appdata.join(folder);
    let ldb = profile.join("Local Storage").join("leveldb");
    fs::create_dir_all(&ldb).unwrap();
    let opts = Options {
        create_if_missing: true,
        compressor: SnappyCompressor::ID,
        ..Options::default()
    };
    let mut db = DB::open(&ldb, opts).unwrap();
    for (k, v) in entries {
        db.put(k, v).unwrap();
    }
    db.flush().unwrap();
    db.close().unwrap();
    profile
}

/// A profile whose manifest is junk: its copy will not open.
fn write_torn_profile(appdata: &Path, folder: &str) -> PathBuf {
    let profile = appdata.join(folder);
    let ldb = profile.join("Local Storage").join("leveldb");
    fs::create_dir_all(&ldb).unwrap();
    fs::write(ldb.join("CURRENT"), b"MANIFEST-000001\n").unwrap();
    fs::write(ldb.join("MANIFEST-000001"), b"junk junk junk").unwrap();
    profile
}

fn paths(appdata: &Path, temp: &Path) -> ImportPaths {
    ImportPaths {
        appdata: appdata.to_path_buf(),
        temp_root: temp.to_path_buf(),
    }
}

/// Flare's preference store over `<dir>/settings.json`. Writes wait an hour, so only `flush` writes,
/// and it writes synchronously: a test reads back exactly what its flush wrote (a zero delay lets the
/// debounce worker take a save that `flush` then cannot see).
fn open_store(dir: &Path) -> PrefStore {
    PrefStore::new(Arc::new(FileBackend::open(
        dir.join("settings.json"),
        Duration::from_secs(3600),
    )))
}

/// What `settings.json` holds on disk now.
fn saved(dir: &Path) -> BTreeMap<String, String> {
    let text = fs::read_to_string(dir.join("settings.json")).unwrap_or_default();
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(serde_json::Value::Object(obj)) => obj
            .into_iter()
            .filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_string())))
            .collect(),
        _ => BTreeMap::new(),
    }
}

/// Every file under `root`: its modified time and its bytes.
fn tree(root: &Path) -> BTreeMap<PathBuf, (SystemTime, Vec<u8>)> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, (SystemTime, Vec<u8>)>) {
        for entry in fs::read_dir(dir).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                walk(root, &p, out);
            } else {
                let modified = fs::metadata(&p).unwrap().modified().unwrap();
                let rel = p.strip_prefix(root).unwrap().to_path_buf();
                out.insert(rel, (modified, fs::read(&p).unwrap()));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

/// The owner's Stable key set, as Local Storage holds it: the retired keys and the dropped
/// microphone key included, so the import has to step over them.
fn stable_profile_entries() -> Vec<(&'static str, &'static str)> {
    vec![
        ("prism.term.theme", "custom"),
        ("prism.term.custom", CUSTOM_JSON),
        ("prism.term.fontPct", "130"),
        ("prism.term.seeThrough", "0"),
        ("prism.window.edges", "faint"),
        ("prism.window.tabLayout", "vertical"),
        ("prism.window.sidebar", "open"),
        ("prism.window.sidebarWidth", "265"),
        ("prism.newtab.mode", "ask"),
        (
            "prism.recentRoots",
            r#"["C:\\Users\\Admin\\Documents\\Claude"]"#,
        ),
        ("prism.help.shell", "powershell"),
        ("prism.dictation.enabled", "1"),
        ("prism.dictation.mode", "hold"),
        ("prism.dictation.pauseMedia", "0"),
        ("prism.dictation.model", "base"),
        ("prism.dictation.mic", "{}"),
        ("prism.window.tabStyle", "x"),
        ("prism.window.tabWidth", "240"),
        ("prism.window.titleBar", "1"),
    ]
}

#[test]
fn a_pt_stable_profile_becomes_a_flare_profile_before_the_first_paint() {
    let a = tempfile::tempdir().unwrap();
    let t = tempfile::tempdir().unwrap();
    let d = tempfile::tempdir().unwrap();
    write_profile(a.path(), "PrismTerminalStable", &stable_profile_entries());

    let prefs = open_store(d.path());
    let r = run::run_once(&prefs, &paths(a.path(), t.path()), CEILING);
    assert_eq!(r.status, Status::Imported);
    assert_eq!(r.origin, Some(Origin::Stable));
    prefs.flush();

    // Read back from disk, as the next start would.
    let on_disk = saved(d.path());
    let expect = [
        ("flare.term.theme", "custom"),
        ("flare.term.fontPct", "130"),
        ("flare.term.seeThrough", "0"),
        ("flare.window.edges", "faint"),
        ("flare.window.tabLayout", "vertical"),
        ("flare.window.sidebar", "open"),
        ("flare.newtab.mode", "ask"),
        ("flare.help.shell", "powershell"),
        ("flare.dictation.enabled", "1"),
        ("flare.dictation.mode", "hold"),
        ("flare.dictation.pauseMedia", "0"),
        ("flare.dictation.model", "base"),
    ];
    for (key, value) in expect {
        assert_eq!(on_disk.get(key).map(String::as_str), Some(value), "{key}");
    }
    let custom_text = on_disk.get("flare.term.custom").expect("custom saved");
    assert_eq!(
        custom_text,
        &serialize_custom(&parse_custom(CUSTOM_JSON).unwrap())
    );
    assert!(on_disk.contains_key("flare.recentRoots"));

    // None of the retired or dropped keys, under any name.
    for key in on_disk.keys() {
        for retired in [
            "tabStyle",
            "tabWidth",
            "titleBar",
            "dictation.mic",
            "prism.",
        ] {
            assert!(!key.contains(retired), "{key} must not be imported");
        }
    }

    let store = LookStore::new(open_store(d.path()));
    assert_eq!(store.theme_id(), "custom");
    assert_eq!(
        store.custom().expect("the Custom theme").palette,
        parse_custom(CUSTOM_JSON).unwrap().palette
    );
}

#[test]
fn the_applied_look_equals_prism_terminals() {
    let a = tempfile::tempdir().unwrap();
    let t = tempfile::tempdir().unwrap();
    let d = tempfile::tempdir().unwrap();
    write_profile(a.path(), "PrismTerminalStable", &stable_profile_entries());
    let prefs = open_store(d.path());
    run::run_once(&prefs, &paths(a.path(), t.path()), CEILING);
    prefs.flush();

    // The same values, set by hand as the owner's settings would hold them.
    let hand = PrefStore::memory();
    hand.set("flare.term.theme", "custom");
    hand.set(
        "flare.term.custom",
        &serialize_custom(&parse_custom(CUSTOM_JSON).unwrap()),
    );
    hand.set("flare.term.fontPct", "130");
    hand.set("flare.term.seeThrough", "0");
    hand.set("flare.window.edges", "faint");

    let imported = LookStore::new(open_store(d.path())).look(false);
    let by_hand = LookStore::new(hand).look(false);
    assert_eq!(imported.chrome.vars, by_hand.chrome.vars);
    assert_eq!(imported.term, by_hand.term);
    assert_eq!(imported, by_hand);
}

#[test]
fn tabs_are_never_imported() {
    let a = tempfile::tempdir().unwrap();
    let t = tempfile::tempdir().unwrap();
    let d = tempfile::tempdir().unwrap();
    let profile = write_profile(
        a.path(),
        "PrismTerminalStable",
        &[("prism.term.theme", "dracula")],
    );
    fs::write(profile.join("tabs.json"), br#"[{"title":"x"}]"#).unwrap();

    let prefs = open_store(d.path());
    let r = run::run_once(&prefs, &paths(a.path(), t.path()), CEILING);
    assert_eq!(r.status, Status::Imported);
    prefs.flush();
    assert!(!d.path().join("tabs.json").exists());

    // The copy holds only the Local Storage files (never LOCK, never tabs.json).
    let leveldb = profile.join("Local Storage").join("leveldb");
    let expected: Vec<String> = fs::read_dir(&leveldb)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n != "LOCK")
        .collect();
    let copy_root = tempfile::tempdir().unwrap();
    let (ls, kept) = ldb::read_local_storage_in(&profile, copy_root.path(), &|_| {}, true).unwrap();
    let kept = kept.expect("the copy is kept for this test");
    assert_eq!(ls.files_copied, expected.len());
    // Opening the copy adds its own LOCK and log files; everything else in it was copied.
    let copied: Vec<String> = fs::read_dir(&kept)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    for name in &expected {
        assert!(copied.contains(name), "{name} was not copied");
    }
    for name in &copied {
        assert!(
            expected.contains(name) || matches!(name.as_str(), "LOCK" | "LOG" | "LOG.old"),
            "{name} is not Local Storage"
        );
    }
    assert!(!copied.iter().any(|n| n == "tabs.json"));
}

#[test]
fn an_old_unmigrated_profile_with_acrylic_and_opacity_is_folded() {
    let a = tempfile::tempdir().unwrap();
    let t = tempfile::tempdir().unwrap();
    let d = tempfile::tempdir().unwrap();
    write_profile(
        a.path(),
        "PrismTerminalStable",
        &[
            ("prism.term.theme", "dracula"),
            ("prism.term.acrylic", "1"),
            ("prism.term.opacity", "60"),
        ],
    );
    let prefs = open_store(d.path());
    let r = run::run_once(&prefs, &paths(a.path(), t.path()), CEILING);
    assert_eq!(r.status, Status::Imported);
    prefs.flush();

    let on_disk = saved(d.path());
    assert_eq!(
        on_disk.get("flare.term.seeThrough").map(String::as_str),
        Some("40")
    );
    let ground = resolve_saved_theme("dracula", None, None).background;
    let expected = opaque(&ground, DEFAULT_GROUND);
    assert_eq!(expected.len(), 7, "the opaque ground is six digits");
    assert_eq!(
        on_disk.get("flare.window.background").map(String::as_str),
        Some(expected.as_str())
    );
    for key in on_disk.keys() {
        assert!(
            !key.contains("opacity") && !key.contains("acrylic"),
            "{key}"
        );
    }
}

#[test]
fn prism_terminals_folders_are_untouched() {
    let a = tempfile::tempdir().unwrap();
    let t = tempfile::tempdir().unwrap();
    let d = tempfile::tempdir().unwrap();
    write_profile(a.path(), "PrismTerminalStable", &stable_profile_entries());
    write_profile(
        a.path(),
        "PrismTerminal",
        &[("prism.term.theme", "pt-default")],
    );
    let before = tree(a.path());
    let prefs = open_store(d.path());
    run::run_once(&prefs, &paths(a.path(), t.path()), CEILING);
    prefs.flush();
    assert_eq!(before, tree(a.path()));
}

#[test]
fn main_is_used_when_there_is_no_stable_profile() {
    let a = tempfile::tempdir().unwrap();
    let t = tempfile::tempdir().unwrap();
    let d = tempfile::tempdir().unwrap();
    write_profile(
        a.path(),
        "PrismTerminal",
        &[("prism.term.theme", "dracula")],
    );
    let prefs = open_store(d.path());
    let r = run::run_once(&prefs, &paths(a.path(), t.path()), CEILING);
    assert_eq!(r.status, Status::Imported);
    assert_eq!(r.origin, Some(Origin::Main));
    prefs.flush();
    assert_eq!(
        saved(d.path()).get("flare.term.theme").map(String::as_str),
        Some("dracula")
    );
}

#[test]
fn a_garbage_local_storage_is_skipped_not_fatal_and_flare_starts_with_defaults() {
    // A profile whose copy will not open: the import asks again later, nothing is written, and
    // the look reads its defaults.
    let a = tempfile::tempdir().unwrap();
    let t = tempfile::tempdir().unwrap();
    let d = tempfile::tempdir().unwrap();
    write_torn_profile(a.path(), "PrismTerminalStable");
    let prefs = open_store(d.path());
    let r = run::run_once(&prefs, &paths(a.path(), t.path()), CEILING);
    assert_eq!(r.status, Status::Retry);
    assert_eq!(prefs.get(ATTEMPTS_KEY).as_deref(), Some("1"));
    assert_eq!(prefs.get(DONE_KEY), None);
    prefs.flush();
    let store = LookStore::new(open_store(d.path()));
    assert_eq!(store.theme_id(), HOST_DEFAULT_THEME);
    let _ = store.look(false);

    // A value no encoding reads is skipped and reported; the rest of the profile still imports.
    let b = tempfile::tempdir().unwrap();
    let u = tempfile::tempdir().unwrap();
    let e = tempfile::tempdir().unwrap();
    write_profile_raw(
        b.path(),
        "PrismTerminalStable",
        &[
            (entry_key("prism.window.edges"), b"\x09junk".to_vec()),
            (entry_key("prism.term.theme"), latin1("dracula")),
        ],
    );
    let prefs = open_store(e.path());
    let r = run::run_once(&prefs, &paths(b.path(), u.path()), CEILING);
    assert_eq!(r.status, Status::Imported);
    assert!(r.skipped.iter().any(|(k, _)| k == "prism.window.edges"));
    prefs.flush();
    let on_disk = saved(e.path());
    assert!(!on_disk.contains_key("flare.window.edges"));
    assert_eq!(
        on_disk.get("flare.term.theme").map(String::as_str),
        Some("dracula")
    );
}

#[test]
fn the_import_leaves_flares_own_existing_choices() {
    let a = tempfile::tempdir().unwrap();
    let t = tempfile::tempdir().unwrap();
    let d = tempfile::tempdir().unwrap();
    write_profile(a.path(), "PrismTerminalStable", &stable_profile_entries());
    let prefs = open_store(d.path());
    prefs.set("flare.term.theme", "nord");
    prefs.set("flare.window.edges", "solid");
    prefs.flush();

    let r = run::run_once(&prefs, &paths(a.path(), t.path()), CEILING);
    assert_eq!(r.status, Status::Imported);
    prefs.flush();
    let on_disk = saved(d.path());
    assert_eq!(
        on_disk.get("flare.term.theme").map(String::as_str),
        Some("nord")
    );
    assert_eq!(
        on_disk.get("flare.window.edges").map(String::as_str),
        Some("solid")
    );
    assert!(r.already_set.contains(&"flare.term.theme".to_string()));
    assert!(r.already_set.contains(&"flare.window.edges".to_string()));
    // Keys Flare did not hold are still written.
    assert_eq!(
        on_disk.get("flare.term.fontPct").map(String::as_str),
        Some("130")
    );
}

#[test]
fn a_second_run_changes_nothing() {
    let a = tempfile::tempdir().unwrap();
    let t = tempfile::tempdir().unwrap();
    let d = tempfile::tempdir().unwrap();
    write_profile(a.path(), "PrismTerminalStable", &stable_profile_entries());
    let prefs = open_store(d.path());
    let first = run::run_once(&prefs, &paths(a.path(), t.path()), CEILING);
    assert_eq!(first.status, Status::Imported);
    prefs.flush();
    let bytes_after_first = fs::read(d.path().join("settings.json")).unwrap();

    // The owner changes a setting, then the app starts again: the import does not run again.
    prefs.set("flare.term.fontPct", "110");
    prefs.flush();
    let bytes_before_second = fs::read(d.path().join("settings.json")).unwrap();
    assert_ne!(bytes_after_first, bytes_before_second);

    let again = open_store(d.path());
    let second = run::run_once(&again, &paths(a.path(), t.path()), CEILING);
    assert_eq!(second.status, Status::AlreadyDone);
    assert!(second.written.is_empty());
    again.flush();
    assert_eq!(
        saved(d.path())
            .get("flare.term.fontPct")
            .map(String::as_str),
        Some("110")
    );
}

/// Reads the owner's real Stable profile, a COPY named by `FLARE_REAL_PT_LEVELDB` (the path of a
/// copy of `PrismTerminalStable`'s folder). Run by hand: `cargo test ... -- --ignored --nocapture`.
#[test]
#[ignore]
fn the_real_stable_profile_copy_imports_in_under_300_ms() {
    let Some(folder) = std::env::var_os("FLARE_REAL_PT_LEVELDB") else {
        eprintln!("FLARE_REAL_PT_LEVELDB is not set; nothing to read");
        return;
    };
    let folder = PathBuf::from(folder);
    let appdata = folder
        .parent()
        .expect("the copy sits in a folder named PrismTerminalStable")
        .to_path_buf();
    let t = tempfile::tempdir().unwrap();
    let d = tempfile::tempdir().unwrap();
    let prefs = open_store(d.path());

    let started = Instant::now();
    let r = run::run_once(&prefs, &paths(&appdata, t.path()), CEILING);
    let elapsed = started.elapsed();
    prefs.flush();

    let keys = r.written.len() + r.already_set.len() + r.skipped.len() + r.retired + r.unknown;
    println!(
        "real import: status {:?}, origin {:?}, {} keys read, {} written, {} skipped, {} retired, {} unknown, {:?}",
        r.status,
        r.origin,
        keys,
        r.written.len(),
        r.skipped.len(),
        r.retired,
        r.unknown,
        elapsed
    );
    assert_eq!(r.status, Status::Imported);
    assert!(
        elapsed < Duration::from_millis(300),
        "the import took {elapsed:?}"
    );
}
