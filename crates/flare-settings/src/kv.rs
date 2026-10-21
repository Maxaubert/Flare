//! Backend, MemoryBackend, FileBackend, PrefStore, EnumPref, ChangeList (Task 3).
//!
//! The preference store is the seam M7's settings pages sit on. A store that cannot write (blocked,
//! full) loses the choice, never the app: reads fall back to the default and writes are dropped, but
//! every listener of the key still hears the change.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crate::debounce::Debounced;

/// Where the values live. `set` may drop the write (a blocked store).
pub trait Backend: Send + Sync {
    fn get(&self, key: &str) -> Option<String>;
    fn set(&self, key: &str, value: &str);
    fn remove(&self, key: &str);
    /// FileBackend writes its pending state now; the others have nothing to write.
    fn flush(&self) {}
}

/// Values in memory: tests, and the default store until the file is opened.
#[derive(Default)]
pub struct MemoryBackend {
    map: Mutex<BTreeMap<String, String>>,
}

impl MemoryBackend {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Backend for MemoryBackend {
    fn get(&self, key: &str) -> Option<String> {
        lock(&self.map).get(key).cloned()
    }
    fn set(&self, key: &str, value: &str) {
        lock(&self.map).insert(key.to_string(), value.to_string());
    }
    fn remove(&self, key: &str) {
        lock(&self.map).remove(key);
    }
}

/// A store that refuses everything (the blocked localStorage of Prism Terminal's tests).
pub struct BlockedBackend;

impl Backend for BlockedBackend {
    fn get(&self, _key: &str) -> Option<String> {
        None
    }
    fn set(&self, _key: &str, _value: &str) {}
    fn remove(&self, _key: &str) {}
}

/// Values in `settings.json`, a JSON object of strings. Writes are debounced.
pub struct FileBackend {
    map: Mutex<BTreeMap<String, String>>,
    file: Debounced,
}

impl FileBackend {
    /// Reads `file` once. Missing, junk or non-string values read as empty. Writes wait `delay`
    /// (400 ms in the app) and are written at once by `flush`.
    pub fn open(file: impl Into<PathBuf>, delay: Duration) -> Self {
        let file = file.into();
        let map = read_map(&file);
        Self {
            map: Mutex::new(map),
            file: Debounced::new(file, delay),
        }
    }

    /// Hands the whole map to the debounced writer. The caller holds the map lock, so two writers
    /// never reorder their saves.
    fn persist(&self, map: &BTreeMap<String, String>) {
        if let Ok(text) = serde_json::to_string(map) {
            self.file.save(text);
        }
    }
}

fn read_map(file: &Path) -> BTreeMap<String, String> {
    let text = match std::fs::read_to_string(file) {
        // sync-ok: the startup read of the settings file
        Ok(t) => t,
        Err(_) => return BTreeMap::new(),
    };
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(serde_json::Value::Object(obj)) => obj
            .into_iter()
            .filter_map(|(k, v)| match v {
                serde_json::Value::String(s) => Some((k, s)),
                _ => None,
            })
            .collect(),
        _ => BTreeMap::new(),
    }
}

impl Backend for FileBackend {
    fn get(&self, key: &str) -> Option<String> {
        lock(&self.map).get(key).cloned()
    }
    fn set(&self, key: &str, value: &str) {
        let mut map = lock(&self.map);
        map.insert(key.to_string(), value.to_string());
        self.persist(&map);
    }
    fn remove(&self, key: &str) {
        let mut map = lock(&self.map);
        map.remove(key);
        self.persist(&map);
    }
    fn flush(&self) {
        self.file.flush();
    }
}

type Callback = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct Entries {
    next_id: u64,
    list: Vec<(u64, String, Callback)>,
}

/// The listeners of a store: each one hears its own key only.
#[derive(Clone, Default)]
struct ChangeList {
    entries: Arc<Mutex<Entries>>,
}

impl ChangeList {
    fn add(&self, key: &str, cb: Callback) -> Subscription {
        let id = {
            let mut e = lock(&self.entries);
            e.next_id += 1;
            let id = e.next_id;
            e.list.push((id, key.to_string(), cb));
            id
        };
        let entries = Arc::clone(&self.entries);
        Subscription::new(move || {
            lock(&entries).list.retain(|(i, _, _)| *i != id);
        })
    }

    /// Runs the callbacks of `key` outside the lock, so a listener may read or set the store.
    fn notify(&self, key: &str) {
        let hits: Vec<Callback> = lock(&self.entries)
            .list
            .iter()
            .filter(|(_, k, _)| k == key)
            .map(|(_, _, cb)| Arc::clone(cb))
            .collect();
        for cb in hits {
            cb();
        }
    }
}

/// A listener's subscription. Dropping it (or calling `cancel`) stops the callbacks.
pub struct Subscription {
    cancel: Option<Box<dyn FnOnce() + Send>>,
}

impl Subscription {
    pub(crate) fn new(cancel: impl FnOnce() + Send + 'static) -> Self {
        Self {
            cancel: Some(Box::new(cancel)),
        }
    }

    pub fn cancel(self) {
        drop(self);
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(f) = self.cancel.take() {
            f();
        }
    }
}

/// A string key/value store with change listeners. Cheap to clone; clones share the store.
#[derive(Clone)]
pub struct PrefStore {
    backend: Arc<dyn Backend>,
    listeners: ChangeList,
}

impl PrefStore {
    pub fn new(backend: Arc<dyn Backend>) -> Self {
        Self {
            backend,
            listeners: ChangeList::default(),
        }
    }

    pub fn memory() -> Self {
        Self::new(Arc::new(MemoryBackend::new()))
    }

    pub fn get(&self, key: &str) -> Option<String> {
        self.backend.get(key)
    }

    /// The backend write (it may be dropped), then every listener of `key`.
    pub fn set(&self, key: &str, value: &str) {
        self.backend.set(key, value);
        self.listeners.notify(key);
    }

    pub fn remove(&self, key: &str) {
        self.backend.remove(key);
        self.listeners.notify(key);
    }

    pub fn on_change(&self, key: &str, cb: impl Fn() + Send + Sync + 'static) -> Subscription {
        self.listeners.add(key, Arc::new(cb))
    }

    pub fn flush(&self) {
        self.backend.flush();
    }
}

/// A setting that is one of a closed set of words. Anything else reads as `fallback`, and junk is
/// never stored.
pub struct EnumPref {
    pub key: &'static str,
    pub allowed: &'static [&'static str],
    pub fallback: &'static str,
}

impl EnumPref {
    pub fn valid(&self, v: Option<&str>) -> &'static str {
        match v {
            Some(v) => self
                .allowed
                .iter()
                .copied()
                .find(|a| *a == v)
                .unwrap_or(self.fallback),
            None => self.fallback,
        }
    }

    pub fn get(&self, s: &PrefStore) -> &'static str {
        self.valid(s.get(self.key).as_deref())
    }

    pub fn set(&self, s: &PrefStore, v: &str) {
        s.set(self.key, self.valid(Some(v)));
    }
}

/// On or off. `default_on`: everything but "0" reads on; otherwise only "1" reads on.
pub fn bool_pref_get(s: &PrefStore, key: &str, default_on: bool) -> bool {
    let raw = s.get(key);
    if default_on {
        raw.as_deref() != Some("0")
    } else {
        raw.as_deref() == Some("1")
    }
}

pub fn bool_pref_set(s: &PrefStore, key: &str, on: bool) {
    s.set(key, if on { "1" } else { "0" });
}

/// Subscribes one callback to several keys; the subscription stops them all.
pub(crate) fn on_any_change(
    s: &PrefStore,
    keys: &[&str],
    cb: impl Fn() + Send + Sync + 'static,
) -> Subscription {
    let cb: Arc<dyn Fn() + Send + Sync> = Arc::new(cb);
    let subs: Vec<Subscription> = keys
        .iter()
        .map(|k| {
            let cb = Arc::clone(&cb);
            s.on_change(k, move || cb())
        })
        .collect();
    Subscription::new(move || drop(subs))
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const LAYOUT: &str = "flare.window.tabLayout";

    #[test]
    fn a_set_value_is_read_back_and_removed() {
        let s = PrefStore::memory();
        s.set("flare.x", "1");
        assert_eq!(s.get("flare.x"), Some("1".to_string()));
        s.remove("flare.x");
        assert_eq!(s.get("flare.x"), None);
    }

    #[test]
    fn keys_reach_the_file_byte_for_byte() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        let s = PrefStore::new(Arc::new(FileBackend::open(
            &file,
            Duration::from_millis(20),
        )));
        s.set(LAYOUT, "vertical");
        s.flush();
        let text = std::fs::read_to_string(&file).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v, serde_json::json!({ LAYOUT: "vertical" }));
        assert_eq!(text, r#"{"flare.window.tabLayout":"vertical"}"#);
        let again = PrefStore::new(Arc::new(FileBackend::open(
            &file,
            Duration::from_millis(20),
        )));
        assert_eq!(again.get(LAYOUT), Some("vertical".to_string()));
    }

    #[test]
    fn a_blocked_store_reads_the_default_drops_writes_and_still_announces() {
        let s = PrefStore::new(Arc::new(BlockedBackend));
        let hits = Arc::new(AtomicUsize::new(0));
        let h = Arc::clone(&hits);
        let _sub = s.on_change(LAYOUT, move || {
            h.fetch_add(1, Ordering::SeqCst);
        });
        crate::tab_layout::set_tab_layout(&s, crate::tab_layout::TabLayout::Vertical);
        assert_eq!(
            crate::tab_layout::tab_layout(&s),
            crate::tab_layout::TabLayout::Horizontal
        );
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn listeners_hear_their_own_key_only_and_stop_when_cancelled() {
        let s = PrefStore::memory();
        let hits = Arc::new(AtomicUsize::new(0));
        let h = Arc::clone(&hits);
        let sub = s.on_change("flare.a", move || {
            h.fetch_add(1, Ordering::SeqCst);
        });
        s.set("flare.b", "x");
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        s.set("flare.a", "x");
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        sub.cancel();
        s.set("flare.a", "y");
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn file_backend_reads_junk_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        for junk in ["not json", "[1]", r#"{"a":5}"#] {
            std::fs::write(&file, junk).unwrap();
            let b = FileBackend::open(&file, Duration::from_millis(20));
            assert_eq!(b.get("a"), None, "junk {junk:?}");
        }
    }

    #[test]
    fn an_enum_pref_reads_unknown_as_the_fallback_and_never_stores_junk() {
        let s = PrefStore::memory();
        crate::tab_layout::set_tab_layout_word(&s, "left");
        assert_eq!(s.get(LAYOUT), Some("horizontal".to_string()));
    }

    #[test]
    fn a_bool_pref_default_on_reads_everything_but_zero_as_on() {
        let s = PrefStore::memory();
        assert!(bool_pref_get(&s, "flare.b", true));
        s.set("flare.b", "0");
        assert!(!bool_pref_get(&s, "flare.b", true));
        s.set("flare.b", "x");
        assert!(bool_pref_get(&s, "flare.b", true));
    }

    #[test]
    fn a_bool_pref_default_off_reads_only_one_as_on() {
        let s = PrefStore::memory();
        assert!(!bool_pref_get(&s, "flare.b", false));
        s.set("flare.b", "1");
        assert!(bool_pref_get(&s, "flare.b", false));
        s.set("flare.b", "x");
        assert!(!bool_pref_get(&s, "flare.b", false));
    }

    #[test]
    fn bool_pref_set_stores_one_or_zero() {
        let s = PrefStore::memory();
        bool_pref_set(&s, "flare.b", true);
        assert_eq!(s.get("flare.b"), Some("1".to_string()));
        bool_pref_set(&s, "flare.b", false);
        assert_eq!(s.get("flare.b"), Some("0".to_string()));
    }

    #[test]
    fn enum_pref_valid_maps_allowed_words_and_falls_back() {
        let p = EnumPref {
            key: "flare.k",
            allowed: &["a", "b"],
            fallback: "a",
        };
        assert_eq!(p.valid(Some("b")), "b");
        assert_eq!(p.valid(Some("c")), "a");
        assert_eq!(p.valid(None), "a");
        assert_eq!(p.get(&PrefStore::memory()), "a");
    }
}
