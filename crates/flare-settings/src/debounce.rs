//! Debounced: a file saved on a delay, so a burst of reports writes once (Task 2).
//!
//! `save` only records the newest data and restarts the wait; one worker thread writes it through the
//! `AtomicWriter` when the wait runs out. `flush` (the close, the quit, the session end) writes at once.

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crate::atomic::AtomicWriter;

pub struct Debounced {
    inner: Arc<Inner>,
}

struct Inner {
    writer: AtomicWriter,
    delay: Duration,
    state: Mutex<State>,
    cv: Condvar,
}

#[derive(Default)]
struct State {
    pending: Option<String>,
    deadline: Option<Instant>,
    stop: bool,
}

impl Debounced {
    pub fn new(file: impl Into<PathBuf>, delay: Duration) -> Self {
        let inner = Arc::new(Inner {
            writer: AtomicWriter::new(file),
            delay,
            state: Mutex::new(State::default()),
            cv: Condvar::new(),
        });
        let worker = Arc::clone(&inner);
        // A thread that cannot start leaves saves pending until flush; the app never reaches that.
        let _ = thread::Builder::new()
            .name("flare-debounce".into())
            .spawn(move || worker_loop(&worker));
        Self { inner }
    }

    /// pending = data; the wait restarts (PT: clearTimeout then setTimeout).
    pub fn save(&self, data: String) {
        let mut st = lock(&self.inner.state);
        st.pending = Some(data);
        st.deadline = Some(Instant::now() + self.inner.delay);
        self.inner.cv.notify_all();
    }

    /// Pending data is written NOW with `save_now`; nothing pending leaves the file alone.
    pub fn flush(&self) {
        let data = {
            let mut st = lock(&self.inner.state);
            st.deadline = None;
            st.pending.take()
        };
        if let Some(data) = data
            && let Err(e) = self.inner.writer.save_now(&data)
        {
            log::warn!("could not write the pending save: {e}");
        }
    }

    /// The save still inside its delay (it IS the newest state: a load inside the 400 ms must see it).
    pub fn pending(&self) -> Option<String> {
        lock(&self.inner.state).pending.clone()
    }

    /// Waits for the background writes in flight. A save still inside its delay is not waited for.
    pub fn idle(&self) {
        self.inner.writer.idle();
    }
}

impl Drop for Debounced {
    /// Stops the worker and writes nothing: a save still inside its delay is lost on purpose.
    fn drop(&mut self) {
        lock(&self.inner.state).stop = true;
        self.inner.cv.notify_all();
    }
}

fn worker_loop(inner: &Inner) {
    let mut st = lock(&inner.state);
    loop {
        if st.stop {
            return;
        }
        match st.deadline {
            None => st = wait(&inner.cv, st),
            Some(deadline) => {
                let now = Instant::now();
                if now < deadline {
                    st = wait_timeout(&inner.cv, st, deadline - now);
                    continue;
                }
                st.deadline = None;
                if let Some(data) = st.pending.take() {
                    drop(st);
                    inner.writer.save(data);
                    st = lock(&inner.state);
                }
            }
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn wait<'a, T>(cv: &Condvar, g: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
    cv.wait(g).unwrap_or_else(PoisonError::into_inner) // sync-ok: the debounced writer thread waits on its own condvar
}

fn wait_timeout<'a, T>(cv: &Condvar, g: MutexGuard<'a, T>, d: Duration) -> MutexGuard<'a, T> {
    cv.wait_timeout(g, d) // sync-ok: the debounced writer thread waits on its own condvar
        .map(|(g, _)| g)
        .unwrap_or_else(|e| e.into_inner().0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_until(mut cond: impl FnMut() -> bool, max: Duration) -> bool {
        let start = Instant::now();
        while !cond() {
            if start.elapsed() > max {
                return false;
            }
            thread::sleep(Duration::from_millis(10));
        }
        true
    }

    const LONG: Duration = Duration::from_secs(60);

    #[test]
    fn debounced_flush_writes_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tabs.json");
        let d = Debounced::new(&file, LONG);
        d.save("a".to_string());
        d.flush();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "a");
        assert_eq!(d.pending(), None);
    }

    #[test]
    fn debounced_writes_the_last_save_after_the_delay() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tabs.json");
        let d = Debounced::new(&file, Duration::from_millis(20));
        d.save("a".to_string());
        d.save("b".to_string());
        assert!(!file.exists());
        assert!(wait_until(|| file.exists(), Duration::from_secs(2)));
        d.idle();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "b");
    }

    #[test]
    fn debounced_pending_is_the_newest_state() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tabs.json");
        std::fs::write(&file, "old").unwrap();
        let d = Debounced::new(&file, LONG);
        d.save("new".to_string());
        assert_eq!(d.pending(), Some("new".to_string()));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "old");
    }

    #[test]
    fn debounced_flush_with_nothing_pending_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tabs.json");
        let d = Debounced::new(&file, LONG);
        d.flush();
        assert!(!file.exists());
    }

    #[test]
    fn a_new_save_restarts_the_wait() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tabs.json");
        let d = Debounced::new(&file, Duration::from_millis(200));
        d.save("a".to_string());
        thread::sleep(Duration::from_millis(120));
        d.save("b".to_string());
        thread::sleep(Duration::from_millis(120));
        assert!(!file.exists());
        assert!(wait_until(|| file.exists(), Duration::from_secs(2)));
        d.idle();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "b");
    }

    #[test]
    fn dropping_a_debounced_file_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tabs.json");
        let d = Debounced::new(&file, LONG);
        d.save("a".to_string());
        drop(d);
        thread::sleep(Duration::from_millis(50));
        assert!(!file.exists());
    }
}
