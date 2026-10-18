//! M9 Task 20: the native stall watchdog and the work timer (decision O16).
//!
//! Port of Prism Terminal's `core/main/stallWatch.ts` and `core/main/ipcTiming.ts`, in Flare's words.
//! The UI thread calls [`StallWatch::beat`] every 50 ms (one atomic store, no lock). A watchdog thread
//! calls [`StallWatch::step`] every tick; a second thread runs the file-system canary. Everything reads
//! a [`Clock`], so the tests drive a [`ManualClock`] and call `step` by hand.

use crate::summary::error_fields;
use crate::writer::LineSink;
use serde_json::{Map, Value, json};
use std::collections::{HashMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Calls kept for the "ended inside the window" part of `inflight`.
pub const RECENT_MAX: usize = 32;
/// Items shown in an `inflight` list.
pub const INFLIGHT_MAX: usize = 8;
/// A work item over this is `work-slow`.
pub const SLOW_MS: u64 = 500;
/// A sync work item over this is `work-slow`.
pub const SYNC_SLOW_MS: u64 = 100;
/// A frame over this is `frame-slow`.
pub const FRAME_SLOW_MS: u64 = 200;
/// Crumbs that ride along on a `frame-slow`.
pub const FRAME_CRUMBS: usize = 5;
/// Crumbs kept.
pub const CRUMB_MAX: usize = 50;
/// The "never beaten" and "nothing recorded" marker of the atomics.
const NONE: u64 = u64::MAX;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A monotonic clock in milliseconds.
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
}

/// The real clock: milliseconds since the first call.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        static START: OnceLock<Instant> = OnceLock::new();
        START.get_or_init(Instant::now).elapsed().as_millis() as u64
    }
}

/// A clock the test moves by hand.
#[derive(Default)]
pub struct ManualClock(AtomicU64);

impl ManualClock {
    pub fn new(start_ms: u64) -> Self {
        ManualClock(AtomicU64::new(start_ms))
    }
    pub fn set(&self, ms: u64) {
        self.0.store(ms, Ordering::SeqCst);
    }
    pub fn advance(&self, ms: u64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// How a work item counts. `Waits` waits on the user or a download (the folder picker, an update install,
/// a dictation download): never slow, never in `inflight`. `Opaque` carries typed text, the clipboard or
/// audio: only sizes are ever written about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkClass {
    Normal,
    Waits,
    Opaque,
}

/// One entry of an `inflight` list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inflight {
    pub name: String,
    pub ms: u64,
    pub done: bool,
}

impl Inflight {
    fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("name".to_owned(), Value::String(self.name.clone()));
        m.insert("ms".to_owned(), json!(self.ms));
        if self.done {
            m.insert("done".to_owned(), Value::Bool(true));
        }
        Value::Object(m)
    }
}

fn inflight_json(list: &[Inflight]) -> Value {
    Value::Array(list.iter().map(Inflight::to_json).collect())
}

#[derive(Default)]
struct TableState {
    next: u64,
    running: HashMap<u64, (String, u64, WorkClass)>,
    /// (name, start, end) of the last items to end.
    recent: VecDeque<(String, u64, u64)>,
}

/// The work running now and the work that just ended.
#[derive(Default)]
pub struct WorkTable {
    state: Mutex<TableState>,
}

impl WorkTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts an item; returns its id.
    pub fn begin(&self, name: &str, class: WorkClass, now: u64) -> u64 {
        let mut s = lock(&self.state);
        let id = s.next;
        s.next += 1;
        s.running.insert(id, (name.to_owned(), now, class));
        id
    }

    /// Ends an item; one that is not `Waits` is remembered for the window of `inflight`.
    pub fn end(&self, id: u64, now: u64) {
        let mut s = lock(&self.state);
        if let Some((name, start, class)) = s.running.remove(&id) {
            if class != WorkClass::Waits {
                s.recent.push_back((name, start, now));
                while s.recent.len() > RECENT_MAX {
                    s.recent.pop_front();
                }
            }
        }
    }

    /// The items running now and those that ENDED in the last `window_ms`, longest first, at most eight.
    pub fn inflight(&self, window_ms: u64, now: u64) -> Vec<Inflight> {
        let s = lock(&self.state);
        let mut out: Vec<Inflight> = s
            .running
            .values()
            .filter(|(_, _, c)| *c != WorkClass::Waits)
            .map(|(n, start, _)| Inflight {
                name: n.clone(),
                ms: now.saturating_sub(*start),
                done: false,
            })
            .collect();
        if window_ms > 0 {
            let from = now.saturating_sub(window_ms);
            out.extend(s.recent.iter().filter(|(_, _, end)| *end >= from).map(
                |(n, start, end)| Inflight {
                    name: n.clone(),
                    ms: end.saturating_sub(*start),
                    done: true,
                },
            ));
        }
        out.sort_by_key(|a| std::cmp::Reverse(a.ms));
        out.truncate(INFLIGHT_MAX);
        out
    }
}

/// An item the log itself owns is never timed.
fn is_own(name: &str) -> bool {
    name == "diag" || name.starts_with("diag:")
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic".to_owned()
    }
}

/// Times work items: slow ones and failing ones are logged; every one in verbose.
pub struct WorkTimer {
    table: Arc<WorkTable>,
    clock: Arc<dyn Clock>,
    sink: Arc<dyn LineSink>,
    verbose: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl WorkTimer {
    pub fn new(
        table: Arc<WorkTable>,
        clock: Arc<dyn Clock>,
        sink: Arc<dyn LineSink>,
        verbose: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Self {
        WorkTimer {
            table,
            clock,
            sink,
            verbose,
        }
    }

    /// Runs `f` as the work item `name`. A panic in `f` is logged as `work-error` and resumed unchanged.
    pub fn time<R>(&self, name: &str, sync: bool, class: WorkClass, f: impl FnOnce() -> R) -> R {
        if is_own(name) {
            return f();
        }
        let start = self.clock.now_ms();
        let id = self.table.begin(name, class, start);
        let result = catch_unwind(AssertUnwindSafe(f));
        let end = self.clock.now_ms();
        self.table.end(id, end);
        let ms = end.saturating_sub(start);
        let failed = result.as_ref().err().map(|p| panic_text(p.as_ref()));
        // The log never breaks the work: a panic while writing is swallowed.
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Some(msg) = &failed {
                let shown = if class == WorkClass::Opaque {
                    format!("({} chars)", msg.chars().count())
                } else {
                    msg.clone()
                };
                let stack = std::backtrace::Backtrace::force_capture().to_string();
                let mut fields = Map::new();
                fields.insert("name".to_owned(), Value::String(name.to_owned()));
                fields.insert("ms".to_owned(), json!(ms));
                fields.extend(error_fields(&shown, Some(&stack)));
                self.sink.emit("main", "work-error", 0, fields);
            }
            let limit = if sync { SYNC_SLOW_MS } else { SLOW_MS };
            if class != WorkClass::Waits && ms >= limit {
                let mut fields = Map::new();
                fields.insert("name".to_owned(), Value::String(name.to_owned()));
                fields.insert("ms".to_owned(), json!(ms));
                fields.insert("sync".to_owned(), Value::Bool(sync));
                self.sink.emit("main", "work-slow", 0, fields);
            } else if (self.verbose)() {
                let mut fields = Map::new();
                fields.insert("name".to_owned(), Value::String(name.to_owned()));
                fields.insert("ms".to_owned(), json!(ms));
                self.sink.emit("main", "work", 0, fields);
            }
        }));
        match result {
            Ok(v) => v,
            Err(payload) => resume_unwind(payload),
        }
    }
}

/// The UI thread's stack, as `module+0xoffset` frames (Task 22's sampler); `None` when it is off.
pub trait StackSampler: Send + Sync {
    fn sample(&self) -> Option<Vec<String>>;
}

type Crumb = (u64, String, Map<String, Value>);

/// The last 50 crumbs.
#[derive(Default)]
pub struct CrumbRing {
    items: Mutex<VecDeque<Crumb>>,
}

impl CrumbRing {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, at_ms: u64, name: &str, fields: Map<String, Value>) {
        let mut v = lock(&self.items);
        v.push_back((at_ms, name.to_owned(), fields));
        while v.len() > CRUMB_MAX {
            v.pop_front();
        }
    }

    /// The last `n` crumbs said at or before `at_ms`, oldest first, each with how long before `at_ms`.
    pub fn before(&self, at_ms: u64, n: usize) -> Vec<(String, u64)> {
        let v = lock(&self.items);
        let mut picked: Vec<(String, u64)> = v
            .iter()
            .filter(|(at, _, _)| *at <= at_ms)
            .map(|(at, name, _)| (name.clone(), at_ms - at))
            .collect();
        let skip = picked.len().saturating_sub(n);
        picked.drain(..skip);
        picked
    }
}

pub struct StallDeps {
    pub sink: Arc<dyn LineSink>,
    pub work: Arc<WorkTable>,
    pub clock: Arc<dyn Clock>,
    pub crumbs: Arc<CrumbRing>,
    pub canary: Arc<dyn Fn() + Send + Sync>,
    pub sampler: Option<Arc<dyn StackSampler>>,
    pub tick_ms: u64,
    pub lag_ms: u64,
    pub canary_ms: u64,
    pub canary_slow_ms: u64,
    pub stack_gap_ms: u64,
}

impl StallDeps {
    /// The defaults: 50 ms tick, 100 ms lag, a canary every 5 s (slow from 500 ms), a stack after 2 s.
    pub fn new(
        sink: Arc<dyn LineSink>,
        work: Arc<WorkTable>,
        clock: Arc<dyn Clock>,
        crumbs: Arc<CrumbRing>,
        canary: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        StallDeps {
            sink,
            work,
            clock,
            crumbs,
            canary,
            sampler: None,
            tick_ms: 50,
            lag_ms: 100,
            canary_ms: 5000,
            canary_slow_ms: 500,
            stack_gap_ms: 2000,
        }
    }
}

struct Inner {
    deps: StallDeps,
    /// The clock at the last beat, or NONE before the first.
    last_beat: AtomicU64,
    /// The longest gap between two beats since the last step.
    max_gap: AtomicU64,
    /// The beat a `ui-stack` was written for.
    stacked_for: AtomicU64,
    suspended: AtomicBool,
    hung: AtomicBool,
    hung_since: AtomicU64,
    canary_busy: AtomicBool,
    stopped: Mutex<bool>,
    wake: Condvar,
}

/// The watchdog. Cloning shares it.
#[derive(Clone)]
pub struct StallWatch {
    inner: Arc<Inner>,
}

impl StallWatch {
    pub fn new(deps: StallDeps) -> StallWatch {
        StallWatch {
            inner: Arc::new(Inner {
                deps,
                last_beat: AtomicU64::new(NONE),
                max_gap: AtomicU64::new(0),
                stacked_for: AtomicU64::new(NONE),
                suspended: AtomicBool::new(false),
                hung: AtomicBool::new(false),
                hung_since: AtomicU64::new(0),
                canary_busy: AtomicBool::new(false),
                stopped: Mutex::new(false),
                wake: Condvar::new(),
            }),
        }
    }

    /// The UI thread is alive. Atomics only: this never waits on anything.
    pub fn beat(&self) {
        let i = &self.inner;
        let now = i.deps.clock.now_ms();
        let prev = i.last_beat.swap(now, Ordering::SeqCst);
        if prev != NONE && !i.suspended.load(Ordering::SeqCst) {
            i.max_gap
                .fetch_max(now.saturating_sub(prev), Ordering::SeqCst);
        }
    }

    fn emit(&self, kind: &str, fields: Map<String, Value>) {
        let sink = &self.inner.deps.sink;
        // A watcher never takes the app down.
        let _ = catch_unwind(AssertUnwindSafe(|| sink.emit("main", kind, 0, fields)));
    }

    fn sample(&self) -> Option<String> {
        let s = self.inner.deps.sampler.as_ref()?;
        let frames = catch_unwind(AssertUnwindSafe(|| s.sample())).ok()??;
        if frames.is_empty() {
            None
        } else {
            Some(frames.join("\n"))
        }
    }

    /// One watchdog iteration.
    pub fn step(&self) {
        let i = &self.inner;
        if i.suspended.load(Ordering::SeqCst) {
            return;
        }
        let now = i.deps.clock.now_ms();
        let gap = i.max_gap.swap(0, Ordering::SeqCst);
        if gap >= i.deps.tick_ms + i.deps.lag_ms {
            let ms = gap - i.deps.tick_ms;
            let list = i.deps.work.inflight(gap + i.deps.tick_ms, now);
            let mut f = Map::new();
            f.insert("ms".to_owned(), json!(ms));
            f.insert("inflight".to_owned(), inflight_json(&list));
            self.emit("main-lag", f);
        }
        let last = i.last_beat.load(Ordering::SeqCst);
        if last != NONE
            && now.saturating_sub(last) >= i.deps.stack_gap_ms
            && i.stacked_for.swap(last, Ordering::SeqCst) != last
        {
            let list = i.deps.work.inflight(0, now);
            let mut f = Map::new();
            f.insert("ms".to_owned(), json!(now - last));
            if let Some(stack) = self.sample() {
                f.insert("stack".to_owned(), Value::String(stack));
            }
            f.insert("inflight".to_owned(), inflight_json(&list));
            self.emit("ui-stack", f);
        }
    }

    pub fn power_suspend(&self) {
        self.inner.suspended.store(true, Ordering::SeqCst);
    }

    /// The sleep is not a lag: the gap is dropped and the beat clock starts over.
    pub fn power_resume(&self) {
        let i = &self.inner;
        let now = i.deps.clock.now_ms();
        if i.last_beat.load(Ordering::SeqCst) != NONE {
            i.last_beat.store(now, Ordering::SeqCst);
        }
        i.max_gap.store(0, Ordering::SeqCst);
        i.suspended.store(false, Ordering::SeqCst);
    }

    /// The glue reports `IsHungAppWindow` transitions: `unresponsive`, then `responsive {ms}`.
    pub fn hung(&self, is_hung: bool) {
        let i = &self.inner;
        let now = i.deps.clock.now_ms();
        if is_hung {
            if !i.hung.swap(true, Ordering::SeqCst) {
                i.hung_since.store(now, Ordering::SeqCst);
                let mut f = Map::new();
                if let Some(stack) = self.sample() {
                    f.insert("stack".to_owned(), Value::String(stack));
                }
                f.insert(
                    "inflight".to_owned(),
                    inflight_json(&i.deps.work.inflight(0, now)),
                );
                self.emit("unresponsive", f);
            }
        } else if i.hung.swap(false, Ordering::SeqCst) {
            let since = i.hung_since.load(Ordering::SeqCst);
            let mut f = Map::new();
            f.insert("ms".to_owned(), json!(now.saturating_sub(since)));
            self.emit("responsive", f);
        }
    }

    /// A frame (update and paint) took `ms`.
    pub fn frame(&self, ms: u64) {
        if ms < FRAME_SLOW_MS {
            return;
        }
        let i = &self.inner;
        let started = i.deps.clock.now_ms().saturating_sub(ms);
        let crumbs: Vec<Value> = i
            .deps
            .crumbs
            .before(started, FRAME_CRUMBS)
            .into_iter()
            .map(|(a, ago)| json!({ "a": a, "ago": ago }))
            .collect();
        let mut f = Map::new();
        f.insert("ms".to_owned(), json!(ms));
        f.insert("crumbs".to_owned(), Value::Array(crumbs));
        self.emit("frame-slow", f);
    }

    /// One canary run, unless one is already running (then false). A call of `canary_slow_ms` or more
    /// is `fs-slow`.
    pub fn canary_once(&self) -> bool {
        let i = &self.inner;
        if i.canary_busy.swap(true, Ordering::SeqCst) {
            return false;
        }
        let t0 = i.deps.clock.now_ms();
        let _ = catch_unwind(AssertUnwindSafe(|| (i.deps.canary)()));
        let ms = i.deps.clock.now_ms().saturating_sub(t0);
        i.canary_busy.store(false, Ordering::SeqCst);
        if ms >= i.deps.canary_slow_ms && !i.suspended.load(Ordering::SeqCst) {
            let mut f = Map::new();
            f.insert("ms".to_owned(), json!(ms));
            self.emit("fs-slow", f);
        }
        true
    }

    /// Waits `ms` or until `stop`; true when stopped.
    fn pause(&self, ms: u64) -> bool {
        let i = &self.inner;
        let guard = lock(&i.stopped);
        if *guard {
            return true;
        }
        let (guard, _) = i
            .wake
            .wait_timeout(guard, Duration::from_millis(ms)) // sync-ok: the watchdog thread waits for its next tick
            .unwrap_or_else(PoisonError::into_inner);
        *guard
    }

    /// Starts thread `flare-watchdog` (a step every tick) and, under it, `flare-canary`. Joining the
    /// returned handle waits for both.
    pub fn spawn(self) -> JoinHandle<()> {
        let watch = self;
        std::thread::Builder::new()
            .name("flare-watchdog".to_owned())
            .spawn(move || {
                let c = watch.clone();
                let canary = std::thread::Builder::new()
                    .name("flare-canary".to_owned())
                    .spawn(move || {
                        while !c.pause(c.inner.deps.canary_ms) {
                            c.canary_once();
                        }
                    })
                    .ok();
                while !watch.pause(watch.inner.deps.tick_ms) {
                    let _ = catch_unwind(AssertUnwindSafe(|| watch.step()));
                }
                if let Some(h) = canary {
                    let _ = h.join(); // sync-ok: the watchdog thread ends its canary thread
                }
            })
            .expect("a thread can be spawned") // sync-ok: startup only
    }

    /// Ends both threads.
    pub fn stop(&self) {
        *lock(&self.inner.stopped) = true;
        self.inner.wake.notify_all();
    }
}
