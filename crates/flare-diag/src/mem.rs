//! M9 Task 21: the memory watch.
//!
//! Every five minutes (and once at start) a `memory` crumb with each kind of process's memory in MB. When the
//! app passes 1 GB, or the GPU process passes 2 GB, a `mem-high` line is written the first time, and again at
//! each doubling (2, 4, 8 GB...), each process on its own counter. Falling back under the threshold resets
//! that counter. A leak shows as a rising line long before it hurts. Source: Prism Terminal's
//! `core/main/memWatch.ts`.
//!
//! The sampling thread is a worker: it never blocks the UI thread, and a source that panics or fails is skipped.

use crate::writer::LineSink;
use serde_json::{Map, Value};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

pub const MEM_EVERY_MS: u64 = 5 * 60 * 1000;
pub const MEM_HIGH_APP_MB: u64 = 1024;
pub const MEM_HIGH_GPU_MB: u64 = 2048;

/// One reading, in MB. `gpu_mb` and `working_set_mb` are `None` where the platform gives no figure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Metrics {
    pub app_mb: u64,
    pub gpu_mb: Option<u64>,
    pub children_mb: u64,
    pub working_set_mb: Option<u64>,
}

/// Where a reading comes from (the glue fills it from the process list; tests fake it).
pub trait MetricsSource: Send + Sync {
    fn read(&self) -> Option<Metrics>;
}

/// Slots of the per-process threshold counters.
const APP: usize = 0;
const GPU: usize = 1;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

pub struct MemWatch {
    sink: Arc<dyn LineSink>,
    source: Arc<dyn MetricsSource>,
    /// The next size each process reports at, in MB: `[app, gpu]`.
    next: Mutex<[u64; 2]>,
    stopped: Mutex<bool>,
    wake: Condvar,
}

impl MemWatch {
    pub fn new(sink: Arc<dyn LineSink>, source: Arc<dyn MetricsSource>) -> Self {
        MemWatch {
            sink,
            source,
            next: Mutex::new([MEM_HIGH_APP_MB, MEM_HIGH_GPU_MB]),
            stopped: Mutex::new(false),
            wake: Condvar::new(),
        }
    }

    /// One reading now: the crumb, then the `mem-high` checks. A missing or panicking reading writes nothing.
    pub fn tick(&self) {
        let read = catch_unwind(AssertUnwindSafe(|| self.source.read()))
            .ok()
            .flatten();
        let Some(m) = read else {
            return;
        };
        let mut crumb = Map::new();
        crumb.insert("a".into(), Value::from("memory"));
        crumb.insert("appMb".into(), Value::from(m.app_mb));
        if let Some(g) = m.gpu_mb {
            crumb.insert("gpuMb".into(), Value::from(g));
        }
        crumb.insert("childrenMb".into(), Value::from(m.children_mb));
        if let Some(w) = m.working_set_mb {
            crumb.insert("workingSetMb".into(), Value::from(w));
        }
        self.sink.emit("app", "crumb", 0, crumb);

        self.check(APP, "app", MEM_HIGH_APP_MB, m.app_mb, &m);
        if let Some(gpu) = m.gpu_mb {
            self.check(GPU, "gpu", MEM_HIGH_GPU_MB, gpu, &m);
        }
    }

    /// Starts the watch on a worker thread: a tick now, then one every `MEM_EVERY_MS`, until `stop`.
    pub fn spawn(self: Arc<Self>) -> JoinHandle<()> {
        std::thread::spawn(move || {
            // sync-ok: the worker waits for its own five-minute period, never the UI thread
            self.run_with(|d| self.sleep_unless_stopped(d));
        })
    }

    /// Ends the worker: it wakes at once and writes no more ticks.
    pub fn stop(&self) {
        *lock(&self.stopped) = true;
        self.wake.notify_all();
    }

    /// The worker loop, with its wait injected: a tick, then `wait` for each period. `wait` returns false
    /// to end the loop (stopped, or a test that has seen enough).
    fn run_with(&self, mut wait: impl FnMut(Duration) -> bool) {
        if self.is_stopped() {
            return;
        }
        self.tick();
        loop {
            if !wait(Duration::from_millis(MEM_EVERY_MS)) || self.is_stopped() {
                return;
            }
            self.tick();
        }
    }

    fn is_stopped(&self) -> bool {
        *lock(&self.stopped)
    }

    /// True when the period ran out, false when `stop` was called first.
    fn sleep_unless_stopped(&self, d: Duration) -> bool {
        let guard = lock(&self.stopped);
        // sync-ok: a worker thread waits here, never the UI thread
        let (guard, _) = self
            .wake
            .wait_timeout_while(guard, d, |stopped| !*stopped)
            .unwrap_or_else(PoisonError::into_inner);
        !*guard
    }

    /// Checks one process against its counter. `value` is that process's size, `m` the whole reading.
    fn check(&self, slot: usize, proc: &str, base: u64, value: u64, m: &Metrics) {
        let Some(over) = self.crossed(slot, base, value) else {
            return;
        };
        let mut f = Map::new();
        f.insert("proc".into(), Value::from(proc));
        f.insert("appMb".into(), Value::from(m.app_mb));
        if let Some(g) = m.gpu_mb {
            f.insert("gpuMb".into(), Value::from(g));
        }
        f.insert("childrenMb".into(), Value::from(m.children_mb));
        f.insert("overMb".into(), Value::from(over));
        self.sink.emit("app", "mem-high", 0, f);
    }

    /// The threshold this reading passed, if it is a first crossing or a doubling; the counter moves up past
    /// the reading. A reading under the base resets the counter to the base.
    fn crossed(&self, slot: usize, base: u64, value: u64) -> Option<u64> {
        let mut next = lock(&self.next);
        if value < base {
            next[slot] = base;
            return None;
        }
        if value < next[slot] {
            return None;
        }
        let passed = next[slot];
        while next[slot] <= value {
            next[slot] *= 2;
        }
        Some(passed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicBool, Ordering};

    type Line = (String, Map<String, Value>);

    #[derive(Default)]
    struct Rec(Mutex<Vec<Line>>);

    impl Rec {
        fn lines(&self) -> Vec<Line> {
            lock(&self.0).clone()
        }
        fn kind(&self, kind: &str) -> Vec<Map<String, Value>> {
            self.lines()
                .into_iter()
                .filter(|(k, _)| k == kind)
                .map(|(_, f)| f)
                .collect()
        }
        /// The (proc, overMb) pairs of the `mem-high` lines, in order.
        fn highs(&self) -> Vec<(String, u64)> {
            self.kind("mem-high")
                .iter()
                .map(|f| {
                    (
                        f["proc"].as_str().unwrap_or_default().to_string(),
                        f["overMb"].as_u64().unwrap_or_default(),
                    )
                })
                .collect()
        }
    }

    impl LineSink for Rec {
        fn emit(&self, _src: &'static str, kind: &str, _at_ms: u64, fields: Map<String, Value>) {
            lock(&self.0).push((kind.to_string(), fields));
        }
    }

    /// A source the test sets; `panics` makes `read` panic instead.
    #[derive(Default)]
    struct Fake {
        value: Mutex<Option<Metrics>>,
        panics: AtomicBool,
    }

    impl Fake {
        fn set(&self, m: Option<Metrics>) {
            *lock(&self.value) = m;
        }
        fn set_app(&self, app: u64, gpu: Option<u64>) {
            self.set(Some(Metrics {
                app_mb: app,
                gpu_mb: gpu,
                children_mb: 13,
                working_set_mb: None,
            }));
        }
    }

    impl MetricsSource for Fake {
        fn read(&self) -> Option<Metrics> {
            if self.panics.load(Ordering::SeqCst) {
                panic!("test: the source fails");
            }
            *lock(&self.value)
        }
    }

    fn watch() -> (Arc<Rec>, Arc<Fake>, MemWatch) {
        let rec = Arc::new(Rec::default());
        let fake = Arc::new(Fake::default());
        let w = MemWatch::new(rec.clone(), fake.clone());
        (rec, fake, w)
    }

    #[test]
    fn a_crumb_with_each_kind_of_process_in_mb() {
        let (rec, fake, w) = watch();
        fake.set(Some(Metrics {
            app_mb: 300,
            gpu_mb: Some(350),
            children_mb: 13,
            working_set_mb: Some(410),
        }));
        w.tick();
        let crumbs = rec.kind("crumb");
        assert_eq!(crumbs.len(), 1);
        assert_eq!(crumbs[0]["a"], "memory");
        assert_eq!(crumbs[0]["appMb"], 300);
        assert_eq!(crumbs[0]["gpuMb"], 350);
        assert_eq!(crumbs[0]["childrenMb"], 13);
        assert_eq!(crumbs[0]["workingSetMb"], 410);
    }

    #[test]
    fn every_five_minutes_by_itself() {
        let (rec, fake, w) = watch();
        fake.set_app(100, None);
        let waits = RefCell::new(Vec::new());
        // Two periods run out, the third wait is cut short (the stop): ticks at start, 5 and 10 minutes.
        w.run_with(|d| {
            waits.borrow_mut().push(d);
            waits.borrow().len() < 3
        });
        assert_eq!(rec.kind("crumb").len(), 3);
        assert!(
            waits
                .borrow()
                .iter()
                .all(|d| *d == Duration::from_millis(MEM_EVERY_MS))
        );
    }

    #[test]
    fn mem_high_when_the_app_passes_1_gb_then_only_at_each_doubling() {
        let (rec, fake, w) = watch();
        fake.set_app(900, None);
        w.tick();
        assert!(rec.highs().is_empty());
        fake.set_app(1100, None);
        w.tick();
        fake.set_app(1500, None);
        w.tick();
        fake.set_app(2100, None);
        w.tick();
        fake.set_app(9800, None); // past 4 and 8 GB at once: one line, the next at 16 GB
        w.tick();
        fake.set_app(15900, None);
        w.tick();
        assert_eq!(
            rec.highs(),
            vec![
                ("app".to_string(), 1024),
                ("app".to_string(), 2048),
                ("app".to_string(), 4096),
            ]
        );
        fake.set_app(16500, None);
        w.tick();
        assert_eq!(rec.highs().len(), 4);
        assert_eq!(rec.highs()[3], ("app".to_string(), 16384));
        let first = &rec.kind("mem-high")[0];
        assert_eq!(first["appMb"], 1100);
    }

    #[test]
    fn gpu_passes_2_gb_on_its_own_counter() {
        let (rec, fake, w) = watch();
        fake.set_app(900, Some(1500));
        w.tick();
        assert!(rec.highs().is_empty());
        fake.set_app(1100, Some(1500));
        w.tick();
        assert_eq!(rec.highs(), vec![("app".to_string(), 1024)]);
        fake.set_app(1100, Some(2100));
        w.tick();
        assert_eq!(
            rec.highs(),
            vec![("app".to_string(), 1024), ("gpu".to_string(), 2048)]
        );
        fake.set_app(1100, Some(4100));
        w.tick();
        assert_eq!(rec.highs()[2], ("gpu".to_string(), 4096));
        assert_eq!(rec.highs().len(), 3);
    }

    #[test]
    fn a_missing_gpu_reading_and_a_source_that_panics_are_skipped() {
        let (rec, fake, w) = watch();
        fake.set_app(500, None);
        w.tick();
        let crumbs = rec.kind("crumb");
        assert!(!crumbs[0].contains_key("gpuMb"));
        assert!(!crumbs[0].contains_key("workingSetMb"));
        assert!(rec.highs().is_empty());
        // A missing gpu reading never touches the gpu counter: the app's crossing still lands.
        fake.set_app(1100, None);
        w.tick();
        assert_eq!(rec.highs(), vec![("app".to_string(), 1024)]);
        let before = rec.lines().len();
        fake.panics.store(true, Ordering::SeqCst);
        w.tick();
        assert_eq!(
            rec.lines().len(),
            before,
            "a panicking source writes nothing"
        );
        fake.panics.store(false, Ordering::SeqCst);
        fake.set(None);
        w.tick();
        assert_eq!(
            rec.lines().len(),
            before,
            "a missing reading writes nothing"
        );
    }

    #[test]
    fn falling_back_under_resets_the_counter() {
        let (rec, fake, w) = watch();
        fake.set_app(1100, None);
        w.tick();
        fake.set_app(2100, None);
        w.tick();
        assert_eq!(
            rec.highs(),
            vec![("app".to_string(), 1024), ("app".to_string(), 2048)]
        );
        fake.set_app(900, None);
        w.tick();
        fake.set_app(1100, None);
        w.tick();
        assert_eq!(
            rec.highs(),
            vec![
                ("app".to_string(), 1024),
                ("app".to_string(), 2048),
                ("app".to_string(), 1024),
            ]
        );
    }

    #[test]
    fn stop_ends_the_worker_thread() {
        let (_rec, fake, w) = watch();
        fake.set_app(100, None);
        let w = Arc::new(w);
        let handle = w.clone().spawn();
        w.stop();
        handle.join().expect("the worker ends after stop");
    }
}
