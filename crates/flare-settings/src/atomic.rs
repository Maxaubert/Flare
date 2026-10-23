//! write_atomic and AtomicWriter (Task 2).
//!
//! A file is always the old one or the new one, never half of either: the data goes to a sibling
//! `.tmp` file, which is then renamed over the real one. The background writer keeps the UI thread
//! off the disk; the synchronous path is for the close, the quit and the session end only.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

/// Writes `<file>.tmp` then renames it over `file` (Windows replaces in one step): the file is always the
/// old one or the new one, never half of either.
pub fn write_atomic(file: &Path, data: &str) -> io::Result<()> {
    let tmp = suffixed(file, ".tmp");
    fs_write(&tmp, data)?; // sync-ok: only the startup read and the quit flush call this on a live path
    if let Err(e) = rename_file(&tmp, file) {
        let _ = remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// Writes off the UI thread. Every save has a number; a rename happens only when nothing newer has landed.
pub struct AtomicWriter {
    inner: Arc<Inner>,
}

struct Inner {
    file: PathBuf,
    queue: Mutex<Queue>,
    queue_cv: Condvar,
    /// The number of the newest write that is on disk. Locked across every rename.
    landed: Mutex<u64>,
    /// Test seam: the background job sleeps this long between the temp write and the rename.
    pause: Duration,
}

#[derive(Default)]
struct Queue {
    /// The newest save waiting for the background thread, with its number.
    next: Option<(String, u64)>,
    /// True while a background thread owns the queue.
    running: bool,
    /// The last number handed out (saves and `save_now` share it).
    seq: u64,
}

impl AtomicWriter {
    pub fn new(file: impl Into<PathBuf>) -> Self {
        Self::with_pause(file, Duration::ZERO)
    }

    /// Test seam: the background job sleeps `pause` between writing the temp file and renaming it.
    pub fn with_pause(file: impl Into<PathBuf>, pause: Duration) -> Self {
        Self {
            inner: Arc::new(Inner {
                file: file.into(),
                queue: Mutex::new(Queue::default()),
                queue_cv: Condvar::new(),
                landed: Mutex::new(0),
                pause,
            }),
        }
    }

    /// Never blocks. A background thread writes `<file>.tmp`, then renames it only if no newer write has
    /// landed (`landed` is locked across the check and the rename). Of a burst only the newest is written.
    /// An io error is swallowed (the previous file stays; the next save tries again), one `log::warn!`.
    pub fn save(&self, data: String) {
        let spawn = {
            let mut q = lock(&self.inner.queue);
            q.seq += 1;
            let n = q.seq;
            q.next = Some((data, n));
            if q.running {
                false
            } else {
                q.running = true;
                true
            }
        };
        if spawn {
            let inner = Arc::clone(&self.inner);
            let started = thread::Builder::new()
                .name("flare-atomic-write".into())
                .spawn(move || run(&inner));
            if let Err(e) = started {
                log::warn!("could not start the save thread: {e}");
                lock(&self.inner.queue).running = false;
                self.inner.queue_cv.notify_all();
            }
        }
    }

    /// Synchronous: `<file>.now.tmp` then rename; sets `landed` to its own number, so an older job in
    /// flight never lands over it. The close, quit and session-end path.
    pub fn save_now(&self, data: &str) -> io::Result<()> {
        let n = self.reserve_now();
        self.save_now_numbered(n, data)
    }

    /// Takes the number of a synchronous write and drops what waits in the queue (it is older). A caller
    /// that orders writes under its own lock reserves here, under that lock, and writes after.
    pub fn reserve_now(&self) -> u64 {
        let mut q = lock(&self.inner.queue);
        q.seq += 1;
        // Anything waiting is older than this write; it must not land over it.
        q.next = None;
        q.seq
    }

    /// The write of a number from `reserve_now`.
    pub fn save_now_numbered(&self, n: u64, data: &str) -> io::Result<()> {
        let tmp = suffixed(&self.inner.file, ".now.tmp");
        write_tmp(&tmp, data, fs_write)?; // sync-ok: close, quit and session end, the process is about to end
        let mut landed = lock(&self.inner.landed);
        if let Err(e) = rename_file(&tmp, &self.inner.file) {
            let _ = remove_file(&tmp);
            return Err(e);
        }
        *landed = n;
        Ok(())
    }

    /// Waits until the background queue is empty, for at most `max`; true when it is empty. The quit
    /// uses this: a write in flight is let finish, but a stuck disk never holds the quit.
    pub fn idle_within(&self, max: Duration) -> bool {
        let end = std::time::Instant::now() + max;
        let mut q = lock(&self.inner.queue);
        while q.running {
            let left = end.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return false;
            }
            q = self
                .inner
                .queue_cv
                .wait_timeout(q, left) // sync-ok: the quit, bounded by `max`
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }

    /// Waits until the background queue is empty (tests and the quit).
    pub fn idle(&self) {
        let mut q = lock(&self.inner.queue);
        while q.running {
            q = wait(&self.inner.queue_cv, q);
        }
    }
}

/// The background thread: writes the newest waiting save until none is left, then lets go of the queue.
fn run(inner: &Inner) {
    loop {
        let job = {
            let mut q = lock(&inner.queue);
            match q.next.take() {
                Some(job) => job,
                None => {
                    q.running = false;
                    inner.queue_cv.notify_all();
                    return;
                }
            }
        };
        let (data, n) = job;
        let tmp = suffixed(&inner.file, ".tmp");
        if let Err(e) = write_tmp(&tmp, &data, fs_write) {
            log::warn!("could not write {}: {e}", inner.file.display());
            continue;
        }
        if !inner.pause.is_zero() {
            thread::sleep(inner.pause); // sync-ok: test seam only, zero in the app
        }
        let mut landed = lock(&inner.landed);
        if n > *landed {
            match rename_file(&tmp, &inner.file) {
                Ok(()) => *landed = n,
                Err(e) => {
                    log::warn!("could not replace {}: {e}", inner.file.display());
                    let _ = remove_file(&tmp);
                }
            }
        } else {
            // A newer write already landed; this temp file is not needed.
            let _ = remove_file(&tmp);
        }
    }
}

/// Writes the temp file with `write`; a failed write (a full disk leaves a partial file) removes the
/// temp file again, so a failure never leaves a `.tmp` behind (audit 2026-10-20, L20).
fn write_tmp(
    tmp: &Path,
    data: &str,
    write: impl FnOnce(&Path, &str) -> io::Result<()>,
) -> io::Result<()> {
    let r = write(tmp, data);
    if r.is_err() {
        let _ = remove_file(tmp);
    }
    r
}

/// What `file` held when it was opened (settings.json, tabs.json).
#[derive(Debug, PartialEq, Eq)]
pub enum Opened {
    /// No such file: a first run. Writing the file later is safe.
    Missing,
    Text(String),
    /// The file exists but could not be read (locked, denied, not UTF-8). `moved_aside` is true when it
    /// was renamed to `<file>.bad`, so the next save cannot overwrite it; false when even that failed,
    /// and the caller must keep its store read-only for the session (audit 2026-10-20, M3).
    Unreadable {
        moved_aside: bool,
    },
}

/// The startup read: only NotFound (or a blank file) reads as empty. A read error, or text that is not
/// valid JSON, moves the file to `<file>.bad` (logged), or reports that it could not, and never reads
/// as empty. A leading BOM is dropped.
pub fn open_text(file: &Path) -> Opened {
    match std::fs::read_to_string(file) {
        // sync-ok: the startup read of a store file
        Ok(t) => {
            // An editor that saves UTF-8 with a BOM must not make a good file read as junk.
            let t = match t.strip_prefix('\u{feff}') {
                Some(rest) => rest.to_string(),
                None => t,
            };
            if t.trim().is_empty() || serde_json::from_str::<serde_json::Value>(&t).is_ok() {
                Opened::Text(t)
            } else {
                // Text that does not parse is the user's data, not an empty store: it is kept
                // as `.bad` before the first save can replace it.
                set_aside(file, "is not valid JSON")
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Opened::Missing,
        Err(e) => set_aside(file, &format!("could not be read: {e}")),
    }
}

/// Moves `file` to `<file>.bad` (logged); `moved_aside` says whether that worked.
fn set_aside(file: &Path, why: &str) -> Opened {
    let bad = suffixed(file, ".bad");
    let moved_aside = rename_file(file, &bad).is_ok();
    log::warn!(
        "{} {why}; {}",
        file.display(),
        if moved_aside {
            "kept as .bad"
        } else {
            "left alone and not written this session"
        }
    );
    Opened::Unreadable { moved_aside }
}

/// `<file><suffix>`, for example `tabs.json` and `.tmp` give `tabs.json.tmp`.
fn suffixed(file: &Path, suffix: &str) -> PathBuf {
    let mut s = file.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

// Every disk call in this module goes through these three helpers; each runs on the caller's thread,
// which is the worker thread or a sync path (see the callers' docs).
fn fs_write(path: &Path, data: &str) -> io::Result<()> {
    std::fs::write(path, data) // sync-ok: worker thread, or the close, quit and startup paths
}

fn rename_file(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::rename(from, to) // sync-ok: worker thread, or the close, quit and startup paths
}

fn remove_file(path: &Path) -> io::Result<()> {
    std::fs::remove_file(path) // sync-ok: worker thread, or the close, quit and startup paths
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn wait<'a, T>(cv: &Condvar, g: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
    cv.wait(g).unwrap_or_else(PoisonError::into_inner) // sync-ok: the atomic writer waits on its own worker, never the UI
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_atomic_replaces_and_leaves_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tabs.json");
        std::fs::write(&file, r#"{"old":true}"#).unwrap();
        write_atomic(&file, r#"{"new":true}"#).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), r#"{"new":true}"#);
        assert!(!dir.path().join("tabs.json.tmp").exists());
    }

    #[test]
    fn write_atomic_creates_a_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("window.json");
        assert!(!file.exists());
        write_atomic(&file, "{}").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{}");
    }

    #[test]
    fn save_returns_before_the_file_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tabs.json");
        let w = AtomicWriter::with_pause(&file, Duration::from_millis(150));
        w.save(r#"{"a":1}"#.to_string());
        assert!(!file.exists());
        w.idle();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), r#"{"a":1}"#);
        assert!(!dir.path().join("tabs.json.tmp").exists());
    }

    #[test]
    fn of_a_burst_the_newest_stays() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tabs.json");
        let w = AtomicWriter::new(&file);
        for n in 1..=20 {
            w.save(format!(r#"{{"n":{n}}}"#));
        }
        w.idle();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), r#"{"n":20}"#);
    }

    #[test]
    fn save_now_is_on_disk_and_an_older_save_never_lands_over_it() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tabs.json");
        let w = AtomicWriter::with_pause(&file, Duration::from_millis(100));
        w.save(r#"{"old":true}"#.to_string());
        w.save_now(r#"{"close":true}"#).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), r#"{"close":true}"#);
        w.idle();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), r#"{"close":true}"#);
    }

    #[test]
    fn a_save_after_save_now_lands() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tabs.json");
        let w = AtomicWriter::new(&file);
        w.save_now(r#"{"close":true}"#).unwrap();
        w.save(r#"{"later":true}"#.to_string());
        w.idle();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), r#"{"later":true}"#);
    }

    #[test]
    fn no_temp_files_remain() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tabs.json");
        let w = AtomicWriter::new(&file);
        for n in 1..=10 {
            w.save(format!(r#"{{"n":{n}}}"#));
        }
        w.save_now(r#"{"close":true}"#).unwrap();
        w.idle();
        assert!(!dir.path().join("tabs.json.tmp").exists());
        assert!(!dir.path().join("tabs.json.now.tmp").exists());
    }

    #[test]
    fn a_failed_write_is_quiet_and_the_next_save_works() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("missing");
        let file = folder.join("tabs.json");
        let w = AtomicWriter::new(&file);
        w.save("x".to_string());
        w.idle();
        std::fs::create_dir(&folder).unwrap();
        w.save("x".to_string());
        w.idle();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "x");
    }

    #[test]
    fn a_bom_is_dropped_and_broken_text_is_kept_as_bad() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        std::fs::write(&file, "\u{feff}{\"a\":\"1\"}").unwrap();
        assert_eq!(open_text(&file), Opened::Text("{\"a\":\"1\"}".to_string()));
        assert!(file.exists(), "a good file stays where it is");
        std::fs::write(&file, "{\"a\":").unwrap();
        assert_eq!(open_text(&file), Opened::Unreadable { moved_aside: true });
        assert_eq!(
            std::fs::read_to_string(dir.path().join("settings.json.bad")).unwrap(),
            "{\"a\":"
        );
        assert_eq!(open_text(&file), Opened::Missing);
    }
}
