//! M9 Task 18: the asynchronous log writer (batches, rotation at 2 MB keeping four, a queue cap).
//!
//! `<userData>\logs\diag.jsonl`, one JSON object per line. A `write` only pushes a finished line on a
//! queue; one thread ("flare-diag") appends the queue every 250 ms as ONE write, so a burst is one write
//! and the order is the order the lines were said in. At 2 MB the file rotates to `.1` through `.4`. At
//! quit and on a panic `flush_sync` writes the queue on the calling thread, after any batch the writer
//! had already taken, so the line just before a crash lands.
//!
//! A LOGGER THAT FAILS IS SILENT. Nothing here panics into its caller or blocks it on the disk: the first
//! failure queues ONE `logger-error` line (it lands if a later write works) and the rest are dropped
//! quietly. A rotation that fails (the file is held open without share-delete) does not stop the log: the
//! batch is appended to the live file anyway and the rotation is tried again 30 s later.
//!
//! The only text a caller can put on a line is its `fields` (a `Map<String, Value>` that `clean_fields`
//! caps and flattens), a kind and a source: there is no raw-text API (regression rule 110).

use crate::summary::{clean_fields, error_fields};
use serde_json::{Map, Value};
use std::collections::VecDeque;
use std::fs::{self, OpenOptions}; // sync-ok: an import; every call below says why it is allowed
use std::io::{self, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const DIAG_FILE: &str = "diag.jsonl";
pub const DIAG_MAX_BYTES: u64 = 2 * 1024 * 1024;
/// Rotated files kept beside the live one: `.1` (newest) to `.4`.
pub const DIAG_KEEP: u32 = 4;
/// The queue holds at most this many lines; past it the NEWEST are dropped and counted.
pub const QUEUE_MAX: usize = 2000;
/// How long a failed rotation waits before it is tried again (uptime ms).
pub const ROTATE_RETRY_MS: u64 = 30_000;
pub const FLUSH_MS: u64 = 250;

/// Every line's own keys, in this order, before its fields.
const LINE_KEYS: [&str; 4] = ["t", "up", "src", "k"];

type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;
type Hook = Arc<dyn Fn() + Send + Sync>;

pub struct DiagLogOptions {
    pub dir: PathBuf,
    pub state_file: Option<PathBuf>,
    pub now: Clock,
    pub uptime: Clock,
    pub max_bytes: u64,
    pub flush_ms: u64,
    /// Test hook: the writer thread calls it right after it took a batch (None in production).
    #[doc(hidden)]
    pub after_take: Option<Hook>,
    /// Test hook: called once per file append (None in production).
    #[doc(hidden)]
    pub on_file_write: Option<Hook>,
}

impl DiagLogOptions {
    /// The state file is `<dir>/../diag.json`; real clocks; the constants.
    pub fn new(dir: PathBuf) -> Self {
        DiagLogOptions {
            state_file: None,
            dir,
            now: Arc::new(epoch_ms),
            uptime: Arc::new(process_uptime_ms),
            max_bytes: DIAG_MAX_BYTES,
            flush_ms: FLUSH_MS,
            after_take: None,
            on_file_write: None,
        }
    }
}

fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn process_uptime_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Where lines go (the sinks of the other diag modules take this, so they are testable without files).
pub trait LineSink: Send + Sync {
    fn emit(&self, src: &'static str, kind: &str, at_ms: u64, fields: Map<String, Value>);
}

#[derive(Default)]
struct QState {
    lines: VecDeque<String>,
    dropped: u64,
    /// A batch the writer thread took and has not yet seen land; `flush_sync` writes it first.
    inflight: Option<String>,
    stop: bool,
}

struct Io {
    size: u64,
    rotate_after: u64,
    dir_ready: bool,
}

struct Inner {
    null: bool,
    dir: PathBuf,
    file: PathBuf,
    state_file: PathBuf,
    max_bytes: u64,
    flush_ms: u64,
    now: Clock,
    uptime: Clock,
    after_take: Option<Hook>,
    on_file_write: Option<Hook>,
    q: Mutex<QState>,
    cv: Condvar,
    io: Mutex<Io>,
    verbose: AtomicBool,
    any_failed: AtomicBool,
    reported: AtomicBool,
    closed: AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A logger never panics into its caller.
fn guarded<R: Default>(f: impl FnOnce() -> R) -> R {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_default()
}

/// Stops the thread and writes what is left once the last handle goes.
struct Guard {
    inner: Arc<Inner>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        guarded(|| self.inner.drain());
        lock(&self.inner.q).stop = true;
        self.inner.cv.notify_all();
    }
}

#[derive(Clone)]
pub struct DiagLog {
    inner: Arc<Inner>,
    _guard: Option<Arc<Guard>>,
}

/// Holds the writer's io lock (a test makes the disk "stuck" with it).
#[doc(hidden)]
pub struct IoHold<'a>(#[allow(dead_code)] MutexGuard<'a, Io>);

impl DiagLog {
    /// Reads the verbose state and the live file's size; spawns the thread "flare-diag". An empty
    /// folder gives the null log.
    pub fn start(opts: DiagLogOptions) -> DiagLog {
        if opts.dir.as_os_str().is_empty() {
            return DiagLog::null();
        }
        let file = opts.dir.join(DIAG_FILE);
        let state_file = opts.state_file.unwrap_or_else(|| {
            opts.dir
                .parent()
                .map_or_else(|| PathBuf::from("diag.json"), |p| p.join("diag.json"))
        });
        let verbose = read_verbose(&state_file);
        let size = file_size(&file);
        let inner = Arc::new(Inner {
            null: false,
            dir: opts.dir,
            file,
            state_file,
            max_bytes: opts.max_bytes,
            flush_ms: opts.flush_ms,
            now: opts.now,
            uptime: opts.uptime,
            after_take: opts.after_take,
            on_file_write: opts.on_file_write,
            q: Mutex::new(QState::default()),
            cv: Condvar::new(),
            io: Mutex::new(Io {
                size,
                rotate_after: 0,
                dir_ready: false,
            }),
            verbose: AtomicBool::new(verbose),
            any_failed: AtomicBool::new(false),
            reported: AtomicBool::new(false),
            closed: AtomicBool::new(false),
        });
        let worker = inner.clone();
        let spawned = std::thread::Builder::new()
            .name("flare-diag".into())
            .spawn(move || run(&worker));
        if spawned.is_err() {
            inner.any_failed.store(true, Ordering::SeqCst);
        }
        DiagLog {
            _guard: Some(Arc::new(Guard {
                inner: inner.clone(),
            })),
            inner,
        }
    }

    /// Writes nothing, creates nothing, `verbose()` false.
    pub fn null() -> DiagLog {
        DiagLog {
            inner: Arc::new(Inner {
                null: true,
                dir: PathBuf::new(),
                file: PathBuf::new(),
                state_file: PathBuf::new(),
                max_bytes: DIAG_MAX_BYTES,
                flush_ms: FLUSH_MS,
                now: Arc::new(epoch_ms),
                uptime: Arc::new(process_uptime_ms),
                after_take: None,
                on_file_write: None,
                q: Mutex::new(QState::default()),
                cv: Condvar::new(),
                io: Mutex::new(Io {
                    size: 0,
                    rotate_after: 0,
                    dir_ready: true,
                }),
                verbose: AtomicBool::new(false),
                any_failed: AtomicBool::new(false),
                reported: AtomicBool::new(false),
                closed: AtomicBool::new(false),
            }),
            _guard: None,
        }
    }

    pub fn write(&self, src: &'static str, kind: &str, fields: Map<String, Value>) {
        self.write_at(src, kind, (self.inner.now)(), fields);
    }

    /// A line whose own clock said when: `at_ms` is its epoch ms (`up` moves back by its age).
    pub fn write_at(&self, src: &'static str, kind: &str, at_ms: u64, fields: Map<String, Value>) {
        guarded(|| self.inner.write_at(src, kind, at_ms, &fields));
    }

    pub fn verbose(&self) -> bool {
        self.inner.verbose.load(Ordering::SeqCst)
    }

    /// Remembers the switch in the state file (atomically) and logs a `verbose {on}` line.
    pub fn set_verbose(&self, on: bool) {
        guarded(|| {
            if self.inner.null || self.inner.verbose.swap(on, Ordering::SeqCst) == on {
                return;
            }
            self.inner.save_verbose(on);
            let mut f = Map::new();
            f.insert("on".into(), Value::Bool(on));
            self.write("main", "verbose", f);
        });
    }

    /// Blocks the CALLER until everything queued before this call is on disk (tests; never the UI thread).
    pub fn flush(&self) {
        guarded(|| self.inner.drain());
    }

    /// The quit and panic path: writes the queue on the calling thread, in order, after any batch the
    /// writer had already taken.
    pub fn flush_sync(&self) {
        guarded(|| self.inner.drain());
    }

    pub fn failed(&self) -> bool {
        self.inner.any_failed.load(Ordering::SeqCst)
    }

    /// Writes what is left and ignores later writes.
    pub fn close(&self) {
        guarded(|| {
            self.inner.drain();
            self.inner.closed.store(true, Ordering::SeqCst);
        });
    }

    pub fn dir(&self) -> &Path {
        &self.inner.dir
    }

    pub fn file(&self) -> PathBuf {
        self.inner.file.clone()
    }

    /// Lines waiting in the queue (tests).
    #[doc(hidden)]
    pub fn queued(&self) -> usize {
        lock(&self.inner.q).lines.len()
    }

    /// Lines dropped at the cap and not yet reported (tests).
    #[doc(hidden)]
    pub fn dropped(&self) -> u64 {
        lock(&self.inner.q).dropped
    }

    /// The queued lines (tests).
    #[doc(hidden)]
    pub fn queued_lines(&self) -> Vec<String> {
        lock(&self.inner.q).lines.iter().cloned().collect()
    }

    /// Holds the io lock until dropped: a stuck disk (tests).
    #[doc(hidden)]
    pub fn hold_io(&self) -> IoHold<'_> {
        IoHold(lock(&self.inner.io))
    }

    /// Panics on another thread while it holds the queue lock (tests).
    #[doc(hidden)]
    pub fn poison_queue_for_test(&self) {
        let inner = self.inner.clone();
        let t = std::thread::spawn(move || {
            let _held = inner.q.lock();
            panic!("test: poison the queue lock");
        });
        let _ = t.join(); // sync-ok: test helper, waits for its own thread
    }
}

impl LineSink for DiagLog {
    fn emit(&self, src: &'static str, kind: &str, at_ms: u64, fields: Map<String, Value>) {
        if at_ms == 0 {
            self.write(src, kind, fields);
        } else {
            self.write_at(src, kind, at_ms, fields);
        }
    }
}

impl Inner {
    fn line(
        &self,
        src: &str,
        kind: &str,
        at_ms: u64,
        up: u64,
        fields: &Map<String, Value>,
    ) -> String {
        let mut m = Map::new();
        m.insert("t".into(), Value::String(format_iso(at_ms)));
        m.insert("up".into(), Value::from(up));
        m.insert("src".into(), Value::String(src.to_string()));
        m.insert("k".into(), Value::String(kind.to_string()));
        for (k, v) in clean_fields(fields, 0) {
            if !LINE_KEYS.contains(&k.as_str()) {
                m.insert(k, v);
            }
        }
        let mut s = Value::Object(m).to_string();
        s.push('\n');
        s
    }

    fn write_at(&self, src: &str, kind: &str, at_ms: u64, fields: &Map<String, Value>) {
        if self.null || self.closed.load(Ordering::SeqCst) {
            return;
        }
        {
            // Past the cap: counted without building the line.
            let mut q = lock(&self.q);
            if q.lines.len() >= QUEUE_MAX {
                q.dropped += 1;
                return;
            }
        }
        let now = (self.now)();
        let at = if at_ms == 0 { now } else { at_ms };
        let up = (self.uptime)().saturating_sub(now.saturating_sub(at));
        let line = self.line(src, kind, at, up, fields);
        let mut q = lock(&self.q);
        if q.lines.len() >= QUEUE_MAX {
            q.dropped += 1;
            return;
        }
        q.lines.push_back(line);
        let first = q.lines.len() == 1;
        drop(q);
        if first {
            self.cv.notify_one();
        }
    }

    /// The first failure queues ONE `logger-error` at the front; later ones are silent.
    fn fail(&self, msg: &str) {
        self.any_failed.store(true, Ordering::SeqCst);
        if self.reported.swap(true, Ordering::SeqCst) {
            return;
        }
        let now = (self.now)();
        let line = self.line(
            "main",
            "logger-error",
            now,
            (self.uptime)(),
            &error_fields(msg, None),
        );
        lock(&self.q).lines.push_front(line);
        self.cv.notify_one();
    }

    /// Everything waiting, in order: a batch the writer took first, then the queue, then the count of
    /// what was dropped.
    fn take_text(&self, q: &mut QState) -> String {
        let mut text = q.inflight.take().unwrap_or_default();
        for l in q.lines.drain(..) {
            text.push_str(&l);
        }
        if q.dropped > 0 {
            let mut f = Map::new();
            f.insert("n".into(), Value::from(q.dropped));
            q.dropped = 0;
            let now = (self.now)();
            text.push_str(&self.line("main", "logger-dropped", now, (self.uptime)(), &f));
        }
        text
    }

    /// Writes everything queued on the calling thread (`flush`, `flush_sync`, the last handle's drop).
    fn drain(&self) {
        if self.null {
            return;
        }
        let mut io = lock(&self.io);
        let text = {
            let mut q = lock(&self.q);
            self.take_text(&mut q)
        };
        if !text.is_empty() {
            self.write_batch(&mut io, &text);
        }
    }

    fn write_batch(&self, io: &mut Io, text: &str) {
        if let Err(e) = self.try_write(io, text) {
            io.dir_ready = false;
            self.fail(&e.to_string());
        }
    }

    fn try_write(&self, io: &mut Io, text: &str) -> io::Result<()> {
        if !io.dir_ready {
            fs::create_dir_all(&self.dir)?; // sync-ok: the writer thread or a quit flush, never the UI
            io.dir_ready = true;
        }
        let bytes = text.len() as u64;
        self.rotate_if_due(io, bytes);
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.file)?;
        if let Some(h) = &self.on_file_write {
            h();
        }
        f.write_all(text.as_bytes())?;
        io.size += bytes;
        Ok(())
    }

    fn rotate_if_due(&self, io: &mut Io, incoming: u64) {
        let due = io.size != 0
            && io.size + incoming > self.max_bytes
            && (self.uptime)() >= io.rotate_after;
        if !due {
            return;
        }
        let numbered = |n: u32| PathBuf::from(format!("{}.{n}", self.file.display()));
        let result = (|| -> io::Result<()> {
            match fs::remove_file(numbered(DIAG_KEEP)) {
                // sync-ok: rotation runs on the writer thread or a quit flush
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
            for n in (1..DIAG_KEEP).rev() {
                // that one may not exist yet
                let _ = fs::rename(numbered(n), numbered(n + 1)); // sync-ok: writer thread rotation
            }
            match fs::rename(&self.file, numbered(1)) {
                // sync-ok: writer thread rotation
                Ok(()) => Ok(()),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e),
            }
        })();
        match result {
            Ok(()) => io.size = 0,
            Err(e) => {
                io.rotate_after = (self.uptime)() + ROTATE_RETRY_MS;
                self.fail(&e.to_string());
            }
        }
    }

    fn save_verbose(&self, on: bool) {
        let attempt = || -> io::Result<()> {
            if let Some(parent) = self.state_file.parent() {
                fs::create_dir_all(parent)?; // sync-ok: a Settings click, rare
            }
            let text = if on {
                "{\"verbose\":true}"
            } else {
                "{\"verbose\":false}"
            };
            flare_settings::atomic::write_atomic(&self.state_file, text)
        };
        if let Err(e) = attempt() {
            self.fail(&e.to_string());
        }
    }
}

/// The writer thread: wait for a line, give a burst `flush_ms` to gather, take it all, write it.
fn run(inner: &Arc<Inner>) {
    loop {
        match catch_unwind(AssertUnwindSafe(|| step(inner))) {
            Ok(true) => {}
            Ok(false) => return,
            Err(_) => std::thread::sleep(Duration::from_millis(50)), // sync-ok: the writer thread backs off
        }
    }
}

/// One batch. False when the log is stopping.
fn step(inner: &Inner) -> bool {
    let mut q = lock(&inner.q);
    while q.lines.is_empty() && !q.stop {
        q = inner.cv.wait(q).unwrap_or_else(PoisonError::into_inner); // sync-ok: the writer thread idles here
    }
    if q.stop {
        return false;
    }
    let (mut q, _) = inner
        .cv
        .wait_timeout_while(q, Duration::from_millis(inner.flush_ms), |q| !q.stop)
        .unwrap_or_else(PoisonError::into_inner);
    if q.stop {
        return false;
    }
    let text = inner.take_text(&mut q);
    q.inflight = Some(text);
    drop(q);
    if let Some(h) = &inner.after_take {
        h();
    }
    let mut io = lock(&inner.io);
    // Still ours? A `flush_sync` that ran first has written it already.
    let batch = lock(&inner.q).inflight.take();
    if let Some(text) = batch.filter(|t| !t.is_empty()) {
        inner.write_batch(&mut io, &text);
    }
    true
}

fn read_verbose(state_file: &Path) -> bool {
    let Ok(text) = fs::read_to_string(state_file) else {
        // sync-ok: startup read of the verbose switch
        return false;
    };
    serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| v.get("verbose").and_then(Value::as_bool))
        .unwrap_or(false)
}

fn file_size(file: &Path) -> u64 {
    fs::metadata(file).map_or(0, |m| m.len()) // sync-ok: startup, once
}

/// `2023-11-14T22:13:20.123Z` from epoch milliseconds (days-to-civil, no dependency).
pub fn format_iso(at_ms: u64) -> String {
    let ms = at_ms % 1000;
    let secs = at_ms / 1000;
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (h, mi, s) = (rem / 3600, rem % 3600 / 60, rem % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{ms:03}Z")
}

static MAIN: Mutex<Option<DiagLog>> = Mutex::new(None);

/// The running app's log, for code deep in the app (a shell spawned, a shell gone); the null log until
/// the host sets one.
pub fn main() -> DiagLog {
    lock(&MAIN).clone().unwrap_or_else(DiagLog::null)
}

/// Task 31 sets it at start; `None` puts the null log back.
pub fn set_main(log: Option<DiagLog>) {
    *lock(&MAIN) = log;
}
