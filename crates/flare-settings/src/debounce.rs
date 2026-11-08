//! Debounced: a file saved on a delay, so a burst of reports writes once (Task 2).
//!
//! `save` only records the newest data and restarts the wait; one worker thread writes it through the
//! `AtomicWriter` when the wait runs out. `flush` (the close, the quit, the session end) writes at once.

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crate::atomic::AtomicWriter;

/// The longest a flush waits for a write already in flight.
const FLUSH_WAIT: Duration = Duration::from_secs(2);

pub struct Debounced {
    inner: Arc<Inner>,
}

struct Inner {
    writer: AtomicWriter,
    delay: Duration,
    /// Test seam: the worker sleeps this long between taking the pending data and handing it to the writer.
    gap: Duration,
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
        Self::with_gap(file, delay, Duration::ZERO)
    }

    /// Test seam: the writer's background job sleeps `pause` between the temp file and the rename.
    #[cfg(test)]
    fn with_writer_pause(file: impl Into<PathBuf>, delay: Duration, pause: Duration) -> Self {
        Self::build(AtomicWriter::with_pause(file, pause), delay, Duration::ZERO)
    }

    /// Test seam: the worker waits `gap` after it took the pending data, before it hands it to the writer.
    pub fn with_gap(file: impl Into<PathBuf>, delay: Duration, gap: Duration) -> Self {
        Self::build(AtomicWriter::new(file), delay, gap)
    }

    fn build(writer: AtomicWriter, delay: Duration, gap: Duration) -> Self {
        let inner = Arc::new(Inner {
            writer,
            delay,
            gap,
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
        // The number is taken under the state lock, where the worker also numbers its save: a save the
        // worker took before this flush gets a lower number and can never land over it (audit L17).
        let (data, n) = {
            let mut st = lock(&self.inner.state);
            st.deadline = None;
            match st.pending.take() {
                Some(d) => (Some(d), self.inner.writer.reserve_now()),
                None => (None, 0),
            }
        };
        if let Some(data) = data
            && let Err(e) = self.inner.writer.save_now_numbered(n, &data)
        {
            log::warn!("could not write the pending save: {e}");
        }
        // A save the worker already handed to the background writer is let finish: the quit ends
        // the process, and a write killed before its rename leaves the previous file.
        if !self.inner.writer.idle_within(FLUSH_WAIT) {
            log::warn!("a save in flight did not finish within the quit wait");
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
                    // Handed over under the state lock (`save` never blocks on the disk), so its number
                    // is ordered against a flush's.
                    if !inner.gap.is_zero() {
                        thread::sleep(inner.gap); // sync-ok: test seam only, zero in the app
                    }
                    inner.writer.save(data);
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
    fn flush_waits_for_a_save_already_in_the_background_writer() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        let d = Debounced::with_writer_pause(
            &file,
            Duration::from_millis(10),
            Duration::from_millis(300),
        );
        d.save("new".to_string());
        // The worker writes it after the delay and hands it to the slow background writer.
        assert!(wait_until(|| d.pending().is_none(), Duration::from_secs(5)));
        thread::sleep(Duration::from_millis(60));
        d.flush();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "new",
            "the quit must not end the process with the write still in flight"
        );
    }

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
    fn a_flush_is_never_overwritten_by_a_save_the_worker_already_took() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tabs.json");
        let d = Debounced::with_gap(&file, Duration::from_millis(10), Duration::from_millis(250));
        d.save("old".to_string());
        thread::sleep(Duration::from_millis(100)); // the worker has taken "old" and is inside its gap
        d.save("new".to_string());
        d.flush();
        d.idle();
        thread::sleep(Duration::from_millis(400));
        d.idle();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "new");
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
        // The claim: the file is written no sooner than `delay` after the LAST save. It is checked
        // against measured times, not against sleeps (a busy runner stretched a 120 ms sleep past the
        // first save's deadline and the old fixed-sleep check failed). A run where the second save
        // itself landed after the first one's deadline proves nothing either way and is tried again.
        let delay = Duration::from_millis(200);
        for _ in 0..5 {
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("tabs.json");
            let d = Debounced::new(&file, delay);
            let first = Instant::now();
            d.save("a".to_string());
            thread::sleep(Duration::from_millis(100));
            let second = Instant::now();
            d.save("b".to_string());
            if first.elapsed() >= delay {
                continue; // the first save may legitimately have come due: inconclusive
            }
            // The first save's deadline passes (it would have written by now without the restart) ...
            assert!(wait_until(|| file.exists(), Duration::from_secs(5)));
            let seen = Instant::now();
            // ... yet the write came only after the wait the second save started.
            assert!(
                seen.duration_since(second) >= delay,
                "written {:?} after the second save, inside its {delay:?} wait",
                seen.duration_since(second)
            );
            d.idle();
            assert_eq!(std::fs::read_to_string(&file).unwrap(), "b");
            return;
        }
        panic!("the runner never let the second save land inside the first one's wait");
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
