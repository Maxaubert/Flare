//! The Chromium Local Storage reader (M7 Task 13).
//!
//! Prism Terminal keeps its settings in Chromium's Local Storage (a LevelDB folder). This reads
//! them from a COPY of that folder, made in a unique temp folder that is removed on every exit
//! path. Nothing under the source is ever written, created, renamed or locked: the only calls on
//! a path under the source are `read_dir`, `symlink_metadata` and `fs::copy` as the source.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusty_leveldb::{DB, LdbIterator, Options};

/// What the reader found.
#[derive(Debug, Default)]
pub struct LocalStorage {
    pub values: BTreeMap<String, String>,
    pub unreadable: Vec<(String, &'static str)>,
    pub files_copied: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ReadError {
    NoFolder,
    Copy(String),
    Open(String),
}

/// Every Prism Terminal entry key starts with the origin, a NUL and a 1 (Latin-1 name follows).
pub const ORIGIN_PREFIX: &[u8] = b"_file://\x00\x01";

const MAX_FILE: u64 = 64 * 1024 * 1024;
const MAX_TOTAL: u64 = 256 * 1024 * 1024;
const MAX_VALUE: usize = 1024 * 1024;
const MAX_ENTRIES: usize = 100_000;

/// Decode a stored value: first byte 1 is Latin-1, 0 is UTF-16LE.
pub fn decode_value(raw: &[u8]) -> Result<String, &'static str> {
    let Some((&tag, rest)) = raw.split_first() else {
        return Err("empty");
    };
    match tag {
        1 => Ok(rest.iter().map(|&b| b as char).collect()),
        0 => {
            if rest.len() % 2 != 0 {
                return Err("odd-utf16");
            }
            let units: Vec<u16> = rest
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .collect();
            String::from_utf16(&units).map_err(|_| "bad-utf16")
        }
        _ => Err("unknown-encoding"),
    }
}

/// The setting name of an entry key, or None for anything that is not ours to read.
pub fn key_name(raw_key: &[u8]) -> Option<String> {
    let name = raw_key.strip_prefix(ORIGIN_PREFIX)?;
    if name.is_empty() || name.len() > 128 || !name.iter().all(|b| (0x20..=0x7e).contains(b)) {
        return None;
    }
    Some(String::from_utf8_lossy(name).into_owned())
}

/// Removes its folder on drop, so every exit path (also a panic) leaves nothing behind.
struct Cleanup(Option<PathBuf>);

impl Cleanup {
    fn keep(&mut self) -> Option<PathBuf> {
        self.0.take()
    }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = fs::remove_dir_all(p); // sync-ok: runs on the import worker
        }
    }
}

fn unique_name(suffix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("flare-import-{}-{}{}", std::process::id(), nanos, suffix)
}

/// Copy the regular files of `src` (not LOCK) into `dst`. Returns the number copied.
fn copy_folder(src: &Path, dst: &Path) -> Result<usize, ReadError> {
    fs::create_dir_all(dst).map_err(|e| ReadError::Copy(e.to_string()))?; // sync-ok: runs on the import worker
    let rd = fs::read_dir(src).map_err(|e| ReadError::Copy(e.to_string()))?; // sync-ok: runs on the import worker
    let mut total = 0u64;
    let mut n = 0usize;
    for entry in rd {
        let entry = entry.map_err(|e| ReadError::Copy(e.to_string()))?;
        let name = entry.file_name();
        if name == "LOCK" {
            continue;
        }
        let meta =
            fs::symlink_metadata(entry.path()).map_err(|e| ReadError::Copy(e.to_string()))?; // sync-ok: runs on the import worker
        if !meta.is_file() {
            continue;
        }
        total += meta.len();
        if meta.len() > MAX_FILE || total > MAX_TOTAL {
            return Err(ReadError::Copy("too big".into()));
        }
        fs::copy(entry.path(), dst.join(&name)).map_err(|e| ReadError::Copy(e.to_string()))?; // sync-ok: runs on the import worker
        n += 1;
    }
    Ok(n)
}

fn open_and_read(dir: &Path) -> Result<LocalStorage, String> {
    let opts = Options {
        create_if_missing: false,
        ..Options::default()
    };
    let mut db = DB::open(dir, opts).map_err(|e| e.to_string())?;
    let mut it = db.new_iter().map_err(|e| e.to_string())?;
    let mut out = LocalStorage::default();
    let mut seen = 0usize;
    while it.advance() {
        let Some((k, v)) = it.current() else { continue };
        seen += 1;
        if seen > MAX_ENTRIES {
            break;
        }
        let Some(name) = key_name(&k) else { continue };
        if v.len() > MAX_VALUE {
            out.unreadable.push((name, "too-long"));
            continue;
        }
        match decode_value(&v) {
            Ok(s) => {
                out.values.insert(name, s);
            }
            Err(why) => out.unreadable.push((name, why)),
        }
    }
    Ok(out)
}

/// Read Prism Terminal's Local Storage from a copy. `pause` is called once (300 ms) before the
/// single retry when the first copy fails or will not open (a copy taken mid-write can be torn; a
/// file can be held or vanish while it is copied).
pub fn read_local_storage(
    profile: &Path,
    temp_root: &Path,
    pause: &dyn Fn(Duration),
) -> Result<LocalStorage, ReadError> {
    read_local_storage_in(profile, temp_root, pause, false).map(|(ls, _)| ls)
}

/// As `read_local_storage`; with `keep` the copy folder is left in place and returned (tests).
pub fn read_local_storage_in(
    profile: &Path,
    temp_root: &Path,
    pause: &dyn Fn(Duration),
    keep: bool,
) -> Result<(LocalStorage, Option<PathBuf>), ReadError> {
    let src = profile.join("Local Storage").join("leveldb");
    // sync-ok: runs on the import worker
    if !fs::metadata(&src).map(|m| m.is_dir()).unwrap_or(false) {
        return Err(ReadError::NoFolder);
    }
    match attempt(&src, temp_root, "", keep) {
        Ok(done) => Ok(done),
        Err(_) => {
            // A copy taken mid-write can be torn, and a file can be held or vanish (a compaction)
            // while it is copied: one more go after the pause.
            pause(Duration::from_millis(300));
            attempt(&src, temp_root, "-retry", keep)
        }
    }
}

/// One copy and read; the copy folder is removed on every path unless `keep`.
fn attempt(
    src: &Path,
    temp_root: &Path,
    suffix: &str,
    keep: bool,
) -> Result<(LocalStorage, Option<PathBuf>), ReadError> {
    let dir = temp_root.join(unique_name(suffix));
    let mut guard = Cleanup(Some(dir.clone()));
    let files = copy_folder(src, &dir)?;
    let mut ls = open_and_read(&dir).map_err(ReadError::Open)?;
    ls.files_copied = files;
    let kept = if keep { guard.keep() } else { None };
    Ok((ls, kept))
}

/// Writes a stand-in Prism Terminal profile for the e2e: `<appdata>/<folder>/Local Storage/leveldb`
/// holding `entries` as Latin-1 values under the page origin, as Chromium would. The import reads
/// it back through `read_local_storage`. Test support only (the e2e scenario `opacityAlpha` seeds
/// the old app's storage with it); nothing in the app calls it.
pub fn write_stand_in_profile(
    appdata: &Path,
    folder: &str,
    entries: &[(&str, &str)],
) -> Result<(), String> {
    use rusty_leveldb::CompressorId;
    use rusty_leveldb::compressor::SnappyCompressor;
    let ldb = appdata.join(folder).join("Local Storage").join("leveldb");
    fs::create_dir_all(&ldb).map_err(|e| e.to_string())?; // sync-ok: test support, off the app
    let options = Options {
        create_if_missing: true,
        compressor: SnappyCompressor::ID,
        ..Options::default()
    };
    let mut db = DB::open(&ldb, options).map_err(|e| e.to_string())?;
    for (name, value) in entries {
        let mut key = ORIGIN_PREFIX.to_vec();
        key.extend_from_slice(name.as_bytes());
        let mut raw = vec![1u8];
        raw.extend(value.chars().map(|c| c as u32 as u8));
        db.put(&key, &raw).map_err(|e| e.to_string())?;
    }
    db.flush().map_err(|e| e.to_string())?;
    db.close().map_err(|e| e.to_string())
}
#[cfg(test)]
mod tests {
    use super::*;
    use rusty_leveldb::CompressorId;
    use rusty_leveldb::compressor::SnappyCompressor;
    use std::cell::Cell;

    fn key(name: &str) -> Vec<u8> {
        let mut k = ORIGIN_PREFIX.to_vec();
        k.extend_from_slice(name.as_bytes());
        k
    }

    fn latin1(s: &str) -> Vec<u8> {
        let mut v = vec![1u8];
        v.extend(s.chars().map(|c| c as u32 as u8));
        v
    }

    fn utf16(s: &str) -> Vec<u8> {
        let mut v = vec![0u8];
        for u in s.encode_utf16() {
            v.extend_from_slice(&u.to_le_bytes());
        }
        v
    }

    /// A profile with `Local Storage/leveldb`; `fill` writes into the open db.
    fn profile(compact: bool, fill: impl FnOnce(&mut DB)) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let ldb = dir.path().join("Local Storage").join("leveldb");
        fs::create_dir_all(&ldb).unwrap();
        let o = Options {
            create_if_missing: true,
            compressor: SnappyCompressor::ID,
            ..Options::default()
        };
        let mut db = DB::open(&ldb, o).unwrap();
        fill(&mut db);
        if compact {
            db.flush().unwrap();
            db.compact_range(b"\x00", b"\xff\xff").unwrap();
        }
        db.close().unwrap();
        drop(db);
        dir
    }

    fn no_pause() -> impl Fn(Duration) {
        |_| panic!("no retry expected")
    }

    fn read(p: &Path, tmp: &Path) -> Result<LocalStorage, ReadError> {
        read_local_storage(p, tmp, &no_pause())
    }

    fn torn_profile() -> tempfile::TempDir {
        let bad = tempfile::tempdir().unwrap();
        let ldb = bad.path().join("Local Storage/leveldb");
        fs::create_dir_all(&ldb).unwrap();
        fs::write(ldb.join("CURRENT"), b"MANIFEST-000001\n").unwrap();
        fs::write(ldb.join("MANIFEST-000001"), b"junk junk junk").unwrap();
        bad
    }

    #[test]
    fn reads_latin1_and_utf16_values() {
        let p = profile(false, |db| {
            db.put(&key("a"), &latin1("pt-default")).unwrap();
            db.put(&key("b"), &utf16("Ærø")).unwrap();
        });
        let t = tempfile::tempdir().unwrap();
        let ls = read(p.path(), t.path()).unwrap();
        assert_eq!(ls.values["a"], "pt-default");
        assert_eq!(ls.values["b"], "Ærø");
    }

    #[test]
    fn a_stand_in_profile_reads_back_what_was_written() {
        let appdata = tempfile::tempdir().unwrap();
        write_stand_in_profile(
            appdata.path(),
            "PrismTerminal",
            &[("prism.term.opacity", "60"), ("prism.term.theme", "dracula")],
        )
        .unwrap();
        let t = tempfile::tempdir().unwrap();
        let ls = read(&appdata.path().join("PrismTerminal"), t.path()).unwrap();
        assert_eq!(ls.values["prism.term.opacity"], "60");
        assert_eq!(ls.values["prism.term.theme"], "dracula");
        assert_eq!(ls.values.len(), 2);
    }

    #[test]
    fn ignores_meta_and_foreign_keys() {
        let p = profile(false, |db| {
            db.put(&key("ok"), &latin1("1")).unwrap();
            db.put(b"META:file://", b"\x08\x01").unwrap();
            db.put(b"METAACCESS:file://", b"\x08\x01").unwrap();
            db.put(b"_http://x\x00\x01k", &latin1("no")).unwrap();
            db.put(b"_file://\x00\x00k\x00", &latin1("no")).unwrap();
        });
        let t = tempfile::tempdir().unwrap();
        let ls = read(p.path(), t.path()).unwrap();
        assert_eq!(ls.values.keys().collect::<Vec<_>>(), vec!["ok"]);
        assert!(ls.unreadable.is_empty());
    }

    #[test]
    fn a_deleted_key_is_gone() {
        let p = profile(false, |db| {
            db.put(&key("x"), &latin1("1")).unwrap();
            db.put(&key("y"), &latin1("2")).unwrap();
            db.delete(&key("x")).unwrap();
        });
        let t = tempfile::tempdir().unwrap();
        let ls = read(p.path(), t.path()).unwrap();
        assert!(!ls.values.contains_key("x"));
        assert_eq!(ls.values["y"], "2");
    }

    #[test]
    fn the_last_write_wins() {
        let p = profile(false, |db| {
            db.put(&key("x"), &latin1("old")).unwrap();
            db.put(&key("x"), &latin1("new")).unwrap();
        });
        let t = tempfile::tempdir().unwrap();
        assert_eq!(read(p.path(), t.path()).unwrap().values["x"], "new");
    }

    #[test]
    fn reads_a_log_only_folder_and_a_compacted_table() {
        let fill = |db: &mut DB| {
            db.put(&key("a"), &latin1("1")).unwrap();
            db.put(&key("b"), &utf16("two")).unwrap();
        };
        let log_only = profile(false, fill);
        let table = profile(true, fill);
        let has_ldb = fs::read_dir(table.path().join("Local Storage/leveldb"))
            .unwrap()
            .any(|e| e.unwrap().path().extension().is_some_and(|x| x == "ldb"));
        assert!(has_ldb, "the compacted fixture has a table");
        let t = tempfile::tempdir().unwrap();
        let a = read(log_only.path(), t.path()).unwrap().values;
        let b = read(table.path(), t.path()).unwrap().values;
        assert_eq!(a, b);
        assert_eq!(a.len(), 2);
    }

    #[test]
    fn never_copies_the_lock_file() {
        let p = profile(false, |db| db.put(&key("a"), &latin1("1")).unwrap());
        let lock = p.path().join("Local Storage/leveldb/LOCK");
        fs::write(&lock, b"OUR-MARKER-FROM-SOURCE").unwrap();
        let t = tempfile::tempdir().unwrap();
        let (ls, kept) = read_local_storage_in(p.path(), t.path(), &no_pause(), true).unwrap();
        let copy = kept.unwrap();
        if let Ok(copied) = fs::read(copy.join("LOCK")) {
            assert_ne!(copied, b"OUR-MARKER-FROM-SOURCE");
        }
        assert!(ls.files_copied > 0);
        let _ = fs::remove_dir_all(copy);
    }

    fn snapshot(dir: &Path) -> Vec<(String, u64, SystemTime, u64)> {
        let mut v: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                let m = e.metadata().unwrap();
                let bytes = fs::read(e.path()).unwrap();
                let mut h: u64 = 0xcbf29ce484222325;
                for b in bytes {
                    h ^= b as u64;
                    h = h.wrapping_mul(0x100000001b3);
                }
                (
                    e.file_name().to_string_lossy().into_owned(),
                    m.len(),
                    m.modified().unwrap(),
                    h,
                )
            })
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    #[test]
    fn never_writes_into_the_source_folder() {
        let p = profile(true, |db| db.put(&key("a"), &latin1("1")).unwrap());
        let src = p.path().join("Local Storage/leveldb");
        let before = snapshot(&src);
        let t = tempfile::tempdir().unwrap();
        read(p.path(), t.path()).unwrap();
        assert_eq!(before, snapshot(&src));
    }

    #[test]
    fn leaves_no_temp_folder_behind() {
        let p = profile(false, |db| db.put(&key("a"), &latin1("1")).unwrap());
        let t = tempfile::tempdir().unwrap();
        read(p.path(), t.path()).unwrap();
        assert_eq!(fs::read_dir(t.path()).unwrap().count(), 0);

        // an Open error
        let bad = torn_profile();
        let r = read_local_storage(bad.path(), t.path(), &|_| {});
        assert!(matches!(r, Err(ReadError::Open(_))));
        assert_eq!(fs::read_dir(t.path()).unwrap().count(), 0);

        // a panic inside the iteration (the guard drops while unwinding)
        let pp = p.path().to_path_buf();
        let tp = t.path().to_path_buf();
        let r = std::panic::catch_unwind(|| {
            let src = pp.join("Local Storage/leveldb");
            let first = tp.join(unique_name("-panic"));
            let _g = Cleanup(Some(first.clone()));
            copy_folder(&src, &first).unwrap();
            panic!("inside the iteration");
        });
        assert!(r.is_err());
        assert_eq!(fs::read_dir(t.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_missing_folder_is_no_folder() {
        let p = tempfile::tempdir().unwrap();
        let t = tempfile::tempdir().unwrap();
        assert_eq!(read(p.path(), t.path()).unwrap_err(), ReadError::NoFolder);
    }

    #[test]
    fn a_torn_manifest_is_one_retry_then_an_open_error() {
        let bad = torn_profile();
        let t = tempfile::tempdir().unwrap();
        let calls = Cell::new(0);
        let waited = Cell::new(Duration::ZERO);
        let r = read_local_storage(bad.path(), t.path(), &|d| {
            calls.set(calls.get() + 1);
            waited.set(d);
        });
        assert!(matches!(r, Err(ReadError::Open(_))));
        assert_eq!(calls.get(), 1);
        assert_eq!(waited.get(), Duration::from_millis(300));
    }

    /// A file Prism Terminal (or a scanner) holds with no sharing cannot be copied; one retry after the
    /// pause reads it once it is let go, and a lock held throughout is a Copy error, never a panic.
    #[cfg(windows)]
    #[test]
    fn a_file_locked_during_the_first_copy_is_one_retry() {
        use std::os::windows::fs::OpenOptionsExt;
        let p = profile(false, |db| db.put(&key("a"), &latin1("1")).unwrap());
        let ldb = p.path().join("Local Storage/leveldb");
        let log = fs::read_dir(&ldb)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.extension().is_some_and(|x| x == "log"))
            .expect("a log file");
        let open_locked = || {
            fs::OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(&log)
                .unwrap()
        };
        let t = tempfile::tempdir().unwrap();

        let held = std::cell::RefCell::new(Some(open_locked()));
        let calls = Cell::new(0);
        let ls = read_local_storage(p.path(), t.path(), &|_| {
            calls.set(calls.get() + 1);
            held.borrow_mut().take();
        })
        .unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(ls.values["a"], "1");
        assert_eq!(fs::read_dir(t.path()).unwrap().count(), 0);

        let _still = open_locked();
        let r = read_local_storage(p.path(), t.path(), &|_| {});
        assert!(matches!(r, Err(ReadError::Copy(_))), "{r:?}");
        assert_eq!(fs::read_dir(t.path()).unwrap().count(), 0);
    }

    #[test]
    fn odd_length_utf16_and_an_unknown_prefix_byte_are_unreadable_not_fatal() {
        let p = profile(false, |db| {
            db.put(&key("odd"), &[0u8, 0x41, 0x00, 0x42]).unwrap();
            db.put(&key("unk"), &[7u8, 1, 2]).unwrap();
            db.put(&key("lone"), &[0u8, 0x00, 0xD8]).unwrap();
            db.put(&key("fine"), &latin1("yes")).unwrap();
        });
        let t = tempfile::tempdir().unwrap();
        let ls = read(p.path(), t.path()).unwrap();
        assert_eq!(ls.values["fine"], "yes");
        let find = |n: &str| ls.unreadable.iter().find(|(k, _)| k == n).map(|(_, r)| *r);
        assert_eq!(find("odd"), Some("odd-utf16"));
        assert_eq!(find("unk"), Some("unknown-encoding"));
        assert_eq!(find("lone"), Some("bad-utf16"));
    }

    #[test]
    fn a_huge_value_is_skipped_not_fatal() {
        let p = profile(false, |db| {
            let mut big = vec![1u8];
            big.extend(std::iter::repeat_n(b'a', 2 * 1024 * 1024));
            db.put(&key("big"), &big).unwrap();
            db.put(&key("small"), &latin1("s")).unwrap();
        });
        let t = tempfile::tempdir().unwrap();
        let ls = read(p.path(), t.path()).unwrap();
        assert_eq!(ls.values["small"], "s");
        assert!(ls.unreadable.contains(&("big".to_string(), "too-long")));
    }

    #[test]
    fn key_name_accepts_only_printable_ascii_names() {
        assert_eq!(
            key_name(&key("prism.term.theme")).as_deref(),
            Some("prism.term.theme")
        );
        assert_eq!(key_name(&key("a b~")).as_deref(), Some("a b~"));
        assert_eq!(key_name(&key("")), None);
        assert_eq!(key_name(&key(&"x".repeat(128))).map(|s| s.len()), Some(128));
        assert_eq!(key_name(&key(&"x".repeat(129))), None);
        assert_eq!(key_name(&key("tab\there")), None);
        assert_eq!(key_name(&key("del\x7f")), None);
        let mut hi = ORIGIN_PREFIX.to_vec();
        hi.push(0xe9);
        assert_eq!(key_name(&hi), None);
        assert_eq!(key_name(b"META:file://"), None);
        assert_eq!(key_name(b"_http://x\x00\x01k"), None);
    }

    #[test]
    fn decode_value_cases() {
        assert_eq!(decode_value(b""), Err("empty"));
        assert_eq!(decode_value(&[1]), Ok(String::new()));
        assert_eq!(decode_value(&[2, 1]), Err("unknown-encoding"));
    }

    #[test]
    #[ignore]
    fn real_profile_copy_reads_the_theme() {
        let Ok(p) = std::env::var("FLARE_REAL_PT_LEVELDB") else {
            return;
        };
        let p = PathBuf::from(p);
        if let Ok(appdata) = std::env::var("APPDATA") {
            let a = appdata.to_lowercase().replace('/', "\\");
            let q = p.to_string_lossy().to_lowercase().replace('/', "\\");
            assert!(!q.starts_with(&a), "refusing a path under %APPDATA%");
        }
        let t = tempfile::tempdir().unwrap();
        let ls = read_local_storage(&p, t.path(), &|d| std::thread::sleep(d)).unwrap();
        println!("{} keys", ls.values.len());
        assert!(ls.values.contains_key("prism.term.theme"));
    }
}
