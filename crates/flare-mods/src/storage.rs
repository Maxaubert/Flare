//! A plugin's own store (plugins phase 5, design 7 and 13): one JSON object per plugin, at most [`MAX_BYTES`] as
//! written, kept in `<profile>\plugins-storage\<id>.json`.
//!
//! - `storage.get {key?}` gives one value (null when the key is not there) or the whole object; `storage.set {key,
//!   value}` sets one key (a null value removes it), `storage.set {value}` replaces the whole object. A set that would
//!   make the store larger than [`MAX_BYTES`] is refused with `too_large` and changes nothing.
//! - The object is read from the disk once, on the caller's thread (the control channel's worker, never the UI
//!   thread), and kept in memory; each plugin has its own lock, so a slow disk for one never holds another.
//! - A set is answered at once and written on a worker of its own: the first write right away, later ones at most
//!   every [`WRITE_EVERY`] (2 a second, design 13), each the newest state (sets in between are coalesced). A write
//!   goes to a `.tmp` file that is renamed over the old one, so the file is always a whole object (rule 36).
//! - A file that is not a JSON object (damaged, or edited by hand) reads as empty and is replaced by the next write.
//!
//! The size is kept as the compact JSON the file holds, entry by entry, so a set never serialises the whole store.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use flare_ctl::{CtlError, code};
use flare_settings::atomic::write_atomic;
use serde_json::{Map, Value};

/// The most a plugin's store may be, as the compact JSON written to its file (design 7).
pub const MAX_BYTES: usize = 1024 * 1024;
/// The longest key, in characters.
pub const KEY_MAX: usize = 128;
/// Writes of one plugin's store are at least this far apart; sets in between are coalesced into the next.
pub const WRITE_EVERY: Duration = Duration::from_millis(500);
/// A file bigger than this is not read (it was not written by Flare); the store starts empty.
const READ_MAX: u64 = 2 * MAX_BYTES as u64;

/// Every plugin's store. Shared by the dispatcher's workers; holds no secret.
pub struct Storage {
    dir: PathBuf,
    slots: Mutex<HashMap<String, Arc<Mutex<Slot>>>>,
    write_every: Duration,
}

#[derive(Default)]
struct Slot {
    /// The object, once read from the disk.
    map: Option<Map<String, Value>>,
    /// The compact JSON size of each entry (`"key":value`), and of the whole object.
    sizes: HashMap<String, usize>,
    bytes: usize,
    /// A change is not on the disk yet.
    dirty: bool,
    /// A writer thread owns the writes of this store.
    writing: bool,
    last_write: Option<Instant>,
    /// What the last write said when it failed.
    error: Option<String>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn bad(m: impl Into<String>) -> CtlError {
    CtlError::new(code::BAD_PARAMS, m)
}

/// The compact JSON size of `"key":value`.
fn entry_size(key: &str, value: &Value) -> usize {
    let k = serde_json::to_string(key).map_or(key.len() + 2, |s| s.len());
    let v = serde_json::to_string(value).map_or(0, |s| s.len());
    k + 1 + v
}

/// The compact JSON size of an object whose entries are `sizes`: the braces and a comma between two entries.
fn object_size(entries: usize, sum: usize) -> usize {
    2 + sum + entries.saturating_sub(1)
}

/// A key: 1 to [`KEY_MAX`] characters, no control character.
fn check_key(key: &str) -> Result<(), CtlError> {
    let n = key.chars().count();
    if n == 0 || n > KEY_MAX || key.chars().any(char::is_control) {
        return Err(bad(format!(
            "key is 1 to {KEY_MAX} characters with no control character"
        )));
    }
    Ok(())
}

fn too_large(bytes: usize) -> CtlError {
    CtlError::new(
        code::TOO_LARGE,
        format!(
            "a plugin's storage holds at most {} KB; this would make it {} KB",
            MAX_BYTES / 1024,
            bytes.div_ceil(1024)
        ),
    )
}

impl Slot {
    /// Reads the store from `file` the first time it is needed (on the caller's thread, a worker).
    fn load(&mut self, file: &Path) {
        if self.map.is_some() {
            return;
        }
        let map = read_object(file);
        self.sizes = map
            .iter()
            .map(|(k, v)| (k.clone(), entry_size(k, v)))
            .collect();
        self.bytes = object_size(self.sizes.len(), self.sizes.values().sum());
        self.map = Some(map);
    }
}

/// The object in `file`: empty when there is none, it is too big or it is not a JSON object.
fn read_object(file: &Path) -> Map<String, Value> {
    match std::fs::metadata(file) {
        // sync-ok: the control channel's worker, never the UI thread
        Ok(m) if m.len() > READ_MAX => {
            log::warn!("a plugin's storage file is too big to read; it starts empty");
            return Map::new();
        }
        Ok(_) => {}
        Err(_) => return Map::new(),
    }
    let text = match std::fs::read_to_string(file) {
        // sync-ok: the control channel's worker, never the UI thread
        Ok(t) => t,
        Err(e) => {
            log::warn!("a plugin's storage file could not be read: {e}");
            return Map::new();
        }
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(m)) => m,
        _ => {
            log::warn!("a plugin's storage file is not a JSON object; it starts empty");
            Map::new()
        }
    }
}

impl Storage {
    /// The stores kept in `dir` (one file per plugin).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            slots: Mutex::new(HashMap::new()),
            write_every: WRITE_EVERY,
        }
    }

    /// A test's own spacing of the writes.
    pub fn with_write_every(mut self, every: Duration) -> Self {
        self.write_every = every;
        self
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The file of the plugin's store (the id is a manifest id: no path in it).
    pub fn file_of(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    fn slot(&self, id: &str) -> Arc<Mutex<Slot>> {
        Arc::clone(lock(&self.slots).entry(id.to_string()).or_default())
    }

    /// `storage.get`: the value of `key` (null when it is not there), or the whole object.
    pub fn get(&self, id: &str, key: Option<&str>) -> Result<Value, CtlError> {
        if let Some(k) = key {
            check_key(k)?;
        }
        let slot = self.slot(id);
        let mut s = lock(&slot);
        s.load(&self.file_of(id));
        let map = s.map.as_ref().expect("loaded");
        Ok(match key {
            Some(k) => map.get(k).cloned().unwrap_or(Value::Null),
            None => Value::Object(map.clone()),
        })
    }

    /// `storage.set`: `key` to `value` (null removes it), or with no key the whole object (`value` must be one).
    /// Refused with `too_large`, changing nothing, when the store would pass [`MAX_BYTES`]. The write follows on a
    /// worker.
    pub fn set(&self, id: &str, key: Option<&str>, value: Value) -> Result<(), CtlError> {
        if let Some(k) = key {
            check_key(k)?;
        }
        let slot = self.slot(id);
        {
            let mut s = lock(&slot);
            s.load(&self.file_of(id));
            match key {
                Some(k) => {
                    let old = s.sizes.get(k).copied();
                    let sum: usize = s.sizes.values().sum::<usize>() - old.unwrap_or(0);
                    let count = s.sizes.len() - usize::from(old.is_some());
                    if value.is_null() {
                        if old.is_none() {
                            return Ok(());
                        }
                        s.map.as_mut().expect("loaded").remove(k);
                        s.sizes.remove(k);
                        s.bytes = object_size(count, sum);
                    } else {
                        let size = entry_size(k, &value);
                        let bytes = object_size(count + 1, sum + size);
                        if bytes > MAX_BYTES {
                            return Err(too_large(bytes));
                        }
                        s.map.as_mut().expect("loaded").insert(k.to_string(), value);
                        s.sizes.insert(k.to_string(), size);
                        s.bytes = bytes;
                    }
                }
                None => {
                    let Value::Object(map) = value else {
                        return Err(bad(
                            "with no key, value is the whole store and must be an object",
                        ));
                    };
                    for k in map.keys() {
                        check_key(k)?;
                    }
                    let sizes: HashMap<String, usize> = map
                        .iter()
                        .map(|(k, v)| (k.clone(), entry_size(k, v)))
                        .collect();
                    let bytes = object_size(sizes.len(), sizes.values().sum());
                    if bytes > MAX_BYTES {
                        return Err(too_large(bytes));
                    }
                    s.map = Some(map);
                    s.sizes = sizes;
                    s.bytes = bytes;
                }
            }
            s.dirty = true;
            if s.writing {
                // The writer that runs takes this change in its next write.
                return Ok(());
            }
            s.writing = true;
        }
        self.start_writer(id, slot);
        Ok(())
    }

    /// The bytes the plugin's store takes now, as its file would hold it (tests and the probe).
    pub fn bytes_of(&self, id: &str) -> usize {
        let slot = self.slot(id);
        let mut s = lock(&slot);
        s.load(&self.file_of(id));
        s.bytes
    }

    /// Whether every change of the plugin's store is on the disk (or failed to get there).
    pub fn is_idle(&self, id: &str) -> bool {
        let slot = self.slot(id);
        let s = lock(&slot);
        !s.writing
    }

    /// What the last write of the plugin's store said, when it failed.
    pub fn last_error(&self, id: &str) -> Option<String> {
        let slot = self.slot(id);
        let s = lock(&slot);
        s.error.clone()
    }

    /// The plugin's store is let go from memory (the next call reads its file again).
    pub fn forget(&self, id: &str) {
        let slot = self.slot(id);
        let mut s = lock(&slot);
        if !s.writing {
            *s = Slot::default();
        }
    }

    /// One writer per store, on its own thread: it writes the newest state, at most every `write_every`, until
    /// nothing is left to write.
    fn start_writer(&self, id: &str, slot: Arc<Mutex<Slot>>) {
        let file = self.file_of(id);
        let dir = self.dir.clone();
        let every = self.write_every;
        let job = {
            let slot = Arc::clone(&slot);
            move || {
                loop {
                    let wait = {
                        let s = lock(&slot);
                        s.last_write
                            .map_or(Duration::ZERO, |t| every.saturating_sub(t.elapsed()))
                    };
                    if !wait.is_zero() {
                        std::thread::sleep(wait); // sync-ok: the storage writer's own thread
                    }
                    let data = {
                        let mut s = lock(&slot);
                        if !s.dirty {
                            s.writing = false;
                            return;
                        }
                        s.dirty = false;
                        serde_json::to_string(s.map.as_ref().unwrap_or(&Map::new()))
                            .unwrap_or_else(|_| "{}".into())
                    };
                    let _ = std::fs::create_dir_all(&dir); // sync-ok: the storage writer's own thread
                    let result = write_atomic(&file, &data);
                    let mut s = lock(&slot);
                    s.last_write = Some(Instant::now());
                    s.error = result.err().map(|e| {
                        log::warn!("a plugin's storage was not written: {e}");
                        e.to_string()
                    });
                }
            }
        };
        if let Err(e) = std::thread::Builder::new()
            .name("flare-plugin-storage".into())
            .spawn(job)
        {
            log::warn!("could not start the plugin storage writer: {e}");
            let mut s = lock(&slot);
            s.writing = false;
            s.error = Some(e.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn wait_idle(s: &Storage, id: &str) {
        let t = Instant::now();
        while !s.is_idle(id) {
            assert!(
                t.elapsed() < Duration::from_secs(5),
                "the write never ended"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn on_disk(s: &Storage, id: &str) -> Value {
        serde_json::from_str(&std::fs::read_to_string(s.file_of(id)).unwrap()).unwrap()
    }

    #[test]
    fn a_key_is_set_read_back_removed_and_written_as_one_object() {
        let dir = tempfile::tempdir().unwrap();
        let s = Storage::new(dir.path().join("plugins-storage"));
        assert_eq!(s.get("todo", Some("a")).unwrap(), Value::Null);
        assert_eq!(s.get("todo", None).unwrap(), json!({}));
        s.set("todo", Some("a"), json!([1, 2])).unwrap();
        s.set("todo", Some("b"), json!({"x": "y"})).unwrap();
        assert_eq!(s.get("todo", Some("a")).unwrap(), json!([1, 2]));
        s.set("todo", Some("a"), Value::Null).unwrap();
        assert_eq!(s.get("todo", None).unwrap(), json!({"b": {"x": "y"}}));
        wait_idle(&s, "todo");
        assert_eq!(on_disk(&s, "todo"), json!({"b": {"x": "y"}}));
        // No temp file is left beside it.
        let names: Vec<String> = std::fs::read_dir(s.dir())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["todo.json"]);
        // Another run reads it back from the file.
        let again = Storage::new(dir.path().join("plugins-storage"));
        assert_eq!(again.get("todo", Some("b")).unwrap(), json!({"x": "y"}));
        // Each plugin has its own store.
        assert_eq!(again.get("other", None).unwrap(), json!({}));
    }

    #[test]
    fn the_size_is_the_compact_json_the_file_holds() {
        let dir = tempfile::tempdir().unwrap();
        let s = Storage::new(dir.path());
        assert_eq!(s.bytes_of("p"), 2);
        s.set("p", Some("a"), json!("x\"y")).unwrap();
        s.set("p", Some("é"), json!({"n": [1, 2.5, null]})).unwrap();
        s.set("p", Some("a"), json!(true)).unwrap();
        let whole = serde_json::to_string(&s.get("p", None).unwrap()).unwrap();
        assert_eq!(s.bytes_of("p"), whole.len());
        s.set("p", Some("a"), Value::Null).unwrap();
        let whole = serde_json::to_string(&s.get("p", None).unwrap()).unwrap();
        assert_eq!(s.bytes_of("p"), whole.len());
        s.set("p", None, json!({"k": 1, "l": "two"})).unwrap();
        assert_eq!(s.bytes_of("p"), r#"{"k":1,"l":"two"}"#.len());
        wait_idle(&s, "p");
    }

    #[test]
    fn a_set_past_one_megabyte_is_too_large_and_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let s = Storage::new(dir.path());
        let half = "x".repeat(600 * 1024);
        s.set("p", Some("a"), json!(half)).unwrap();
        let e = s.set("p", Some("b"), json!(half)).unwrap_err();
        assert_eq!(e.code, code::TOO_LARGE);
        assert!(e.message.contains("1024 KB"), "{}", e.message);
        assert_eq!(s.get("p", Some("b")).unwrap(), Value::Null);
        // Replacing the same key with one as big still fits; a whole store too big is refused the same way.
        s.set("p", Some("a"), json!(half.clone() + "y")).unwrap();
        let e = s
            .set("p", None, json!({"a": half, "b": "x".repeat(600 * 1024)}))
            .unwrap_err();
        assert_eq!(e.code, code::TOO_LARGE);
        // Exactly at the limit is allowed: `{"k":"..."}` of MAX_BYTES.
        let fill = "z".repeat(MAX_BYTES - r#"{"k":""}"#.len());
        s.set("q", Some("k"), json!(fill)).unwrap();
        assert_eq!(s.bytes_of("q"), MAX_BYTES);
        assert_eq!(
            s.set("q", Some("l"), json!(1)).unwrap_err().code,
            code::TOO_LARGE
        );
        wait_idle(&s, "p");
        wait_idle(&s, "q");
        assert_eq!(
            on_disk(&s, "p")["a"].as_str().unwrap().len(),
            600 * 1024 + 1
        );
        assert!(on_disk(&s, "p").get("b").is_none());
    }

    #[test]
    fn bad_keys_and_a_whole_store_that_is_not_an_object_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let s = Storage::new(dir.path());
        for k in ["", "a\nb", "\u{1b}[x"] {
            assert_eq!(
                s.set("p", Some(k), json!(1)).unwrap_err().code,
                code::BAD_PARAMS,
                "{k:?}"
            );
            assert_eq!(s.get("p", Some(k)).unwrap_err().code, code::BAD_PARAMS);
        }
        let long = "k".repeat(KEY_MAX + 1);
        assert_eq!(
            s.set("p", Some(&long), json!(1)).unwrap_err().code,
            code::BAD_PARAMS
        );
        assert!(s.set("p", Some(&"k".repeat(KEY_MAX)), json!(1)).is_ok());
        for v in [json!([1]), json!("x"), json!(null)] {
            assert_eq!(s.set("p", None, v).unwrap_err().code, code::BAD_PARAMS);
        }
        assert_eq!(
            s.set("p", None, json!({"a\u{7}": 1})).unwrap_err().code,
            code::BAD_PARAMS
        );
        wait_idle(&s, "p");
    }

    #[test]
    fn sets_close_together_are_coalesced_and_the_file_ends_with_the_newest_state() {
        let dir = tempfile::tempdir().unwrap();
        let s = Storage::new(dir.path()).with_write_every(Duration::from_millis(150));
        for i in 0..40 {
            s.set("p", Some("n"), json!(i)).unwrap();
        }
        // The first write may be out at once; the rest wait for the spacing and land as one.
        assert!(!s.is_idle("p"));
        wait_idle(&s, "p");
        assert_eq!(on_disk(&s, "p"), json!({"n": 39}));
        assert!(s.last_error("p").is_none());
    }

    #[test]
    fn a_damaged_or_foreign_file_reads_as_empty_and_the_next_set_replaces_it_whole() {
        let dir = tempfile::tempdir().unwrap();
        let s = Storage::new(dir.path());
        std::fs::write(s.file_of("p"), "{not json").unwrap();
        std::fs::write(s.file_of("q"), "[1,2]").unwrap();
        assert_eq!(s.get("p", None).unwrap(), json!({}));
        assert_eq!(s.get("q", None).unwrap(), json!({}));
        s.set("p", Some("a"), json!(1)).unwrap();
        wait_idle(&s, "p");
        assert_eq!(on_disk(&s, "p"), json!({"a": 1}));
    }

    #[test]
    fn a_failed_write_is_kept_as_the_last_error_and_the_store_in_memory_stays() {
        let dir = tempfile::tempdir().unwrap();
        // The store's folder is a file: no write can land.
        let blocked = dir.path().join("blocked");
        std::fs::write(&blocked, "x").unwrap();
        let s = Storage::new(&blocked);
        s.set("p", Some("a"), json!(1)).unwrap();
        wait_idle(&s, "p");
        assert!(s.last_error("p").is_some());
        assert_eq!(s.get("p", Some("a")).unwrap(), json!(1));
    }
}
