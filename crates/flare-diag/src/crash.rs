//! M9 Task 19: the panic hook and the gone (crash) record.
//!
//! Port of Prism Terminal's `core/main/crashHooks.ts`. OBSERVE ONLY: the hook writes the panic to the log
//! and then calls the hook that was there before, so the default message and the unwind or abort are
//! untouched. A panic is written AT ONCE (`flush_sync`), because the thread may be about to end and the
//! 250 ms batch would lose the line that explains it. The same panic over and over is one line per 10 s
//! with a count (the error gate, `gate.rs`), at most ten lines per kind per 10 s.
//!
//! The hook never panics: a panic raised while a panic hook runs aborts the process in Rust, even when it
//! is caught, so the sinks must not panic (the writer recovers poisoned locks and guards its own calls).
//! The body runs under `catch_unwind` anyway, and a re-entrant call goes straight to the previous hook.

use crate::gate::{DEFAULT_BUDGET, DEFAULT_EVERY_MS, DiagGate};
use crate::summary::error_fields;
use crate::writer::{DiagLog, LineSink};
use serde_json::{Map, Value};
use std::backtrace::Backtrace;
use std::cell::Cell;
use std::panic::{AssertUnwindSafe, PanicHookInfo, catch_unwind, set_hook, take_hook};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Instant;

/// Flushes the log's queue on the calling thread (the panic path).
pub type Flush = Arc<dyn Fn() + Send + Sync>;
type PrevHook = Arc<dyn Fn(&PanicHookInfo<'_>) + Send + Sync>;
type Gate = DiagGate<Map<String, Value>>;

thread_local! {
    /// Set while the hook is writing, so a panic raised inside it goes straight to the previous hook.
    static IN_HOOK: Cell<bool> = const { Cell::new(false) };
}

/// The panic hook this crate installed. `uninstall` takes it off again.
pub struct CrashHooks {
    prev: PrevHook,
    sink: Arc<dyn LineSink>,
    flush: Flush,
    gate: Arc<Mutex<Gate>>,
}

/// What the installed hook needs, moved into it.
struct HookState {
    prev: PrevHook,
    sink: Arc<dyn LineSink>,
    flush: Flush,
    gate: Arc<Mutex<Gate>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Monotonic milliseconds for the repeat gate.
fn now_ms() -> i64 {
    static START: OnceLock<Instant> = OnceLock::new();
    let ms = START.get_or_init(Instant::now).elapsed().as_millis();
    i64::try_from(ms).unwrap_or(i64::MAX)
}

fn opt_str(v: Option<&str>) -> Value {
    v.map_or(Value::Null, |s| Value::String(s.to_owned()))
}

fn payload_text(p: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_owned()
    }
}

fn record(h: &HookState, info: &PanicHookInfo<'_>) {
    let msg = payload_text(info.payload());
    let loc = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()));
    let thread = std::thread::current().name().map(str::to_owned);
    let backtrace = Backtrace::force_capture().to_string();
    let fields = panic_fields(&msg, loc.as_deref(), thread.as_deref(), &backtrace);
    let key = format!("panic|{}|{}", msg, loc.as_deref().unwrap_or(""));
    let offered = lock(&h.gate).offer("panic", &key, fields, now_ms());
    if let Some((mut line, repeats)) = offered {
        if let Some(n) = repeats {
            line.insert("repeats".into(), Value::from(n));
        }
        h.sink.emit("main", "panic", 0, line);
        (h.flush)();
    }
}

fn hook_body(h: &HookState, info: &PanicHookInfo<'_>) {
    if IN_HOOK.with(Cell::get) {
        (h.prev)(info);
        return;
    }
    IN_HOOK.with(|f| f.set(true));
    let _ = catch_unwind(AssertUnwindSafe(|| record(h, info)));
    IN_HOOK.with(|f| f.set(false));
    (h.prev)(info);
}

impl CrashHooks {
    /// Installs the hook over the log: panics are written to it, then the previous hook runs.
    pub fn install(log: DiagLog) -> CrashHooks {
        let flush_log = log.clone();
        Self::install_with(Arc::new(log), Arc::new(move || flush_log.flush_sync()))
    }

    /// The same, over any sink and flush (the tests use a sink that panics).
    #[doc(hidden)]
    pub fn install_with(sink: Arc<dyn LineSink>, flush: Flush) -> CrashHooks {
        let prev: PrevHook = Arc::from(take_hook());
        let gate = Arc::new(Mutex::new(DiagGate::new(DEFAULT_EVERY_MS, DEFAULT_BUDGET)));
        let state = HookState {
            prev: prev.clone(),
            sink: sink.clone(),
            flush: flush.clone(),
            gate: gate.clone(),
        };
        set_hook(Box::new(move |info| hook_body(&state, info)));
        CrashHooks {
            prev,
            sink,
            flush,
            gate,
        }
    }

    /// Takes the hook off again: the previous one is back, and the lines the gate still holds are written.
    pub fn uninstall(self) {
        let _ = take_hook();
        let prev = self.prev.clone();
        set_hook(Box::new(move |info| prev(info)));
        let held = lock(&self.gate).sweep(now_ms());
        for (mut line, n) in held {
            line.insert("repeats".into(), Value::from(n));
            self.sink.emit("main", "panic", 0, line);
        }
        (self.flush)();
    }
}

/// The fields of a `panic` line: msg, loc (`file:line:col`), thread, origin `panic`, and the stack (the
/// backtrace, capped at 2000 characters).
pub fn panic_fields(
    msg: &str,
    location: Option<&str>,
    thread: Option<&str>,
    backtrace: &str,
) -> Map<String, Value> {
    let mut m = error_fields(msg, Some(backtrace));
    m.insert("loc".into(), opt_str(location));
    m.insert("thread".into(), opt_str(thread));
    m.insert("origin".into(), Value::String("panic".into()));
    m
}

/// The fields of a `gone` line: type (the kind of process or thing), reason, exitCode, name.
pub fn gone_fields(
    kind: &str,
    reason: &str,
    exit_code: Option<i32>,
    name: &str,
) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("type".into(), Value::String(kind.to_owned()));
    m.insert("reason".into(), Value::String(reason.to_owned()));
    m.insert(
        "exitCode".into(),
        exit_code.map_or(Value::Null, Value::from),
    );
    m.insert("name".into(), Value::String(name.to_owned()));
    m
}

/// Writes a `gone` line and flushes it at once: a process that went may take the app with it.
pub fn land_gone(log: &DiagLog, kind: &str, reason: &str, exit_code: Option<i32>, name: &str) {
    log.write("main", "gone", gone_fields(kind, reason, exit_code, name));
    log.flush_sync();
}

/// The fields of the window's hang lines: `unresponsive` has none; `responsive` says how long it lasted.
pub fn hang_fields(responsive_after_ms: Option<u64>) -> Map<String, Value> {
    let mut m = Map::new();
    if let Some(ms) = responsive_after_ms {
        m.insert("ms".into(), Value::from(ms));
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_same_panic_over_and_over_is_one_line_per_10_s_with_a_count() {
        let mut gate: Gate = DiagGate::new(DEFAULT_EVERY_MS, DEFAULT_BUDGET);
        let key = "panic|boom|a.rs:1:1";
        let mut written = Vec::new();
        for t in (0..5000).step_by(250) {
            let line = panic_fields("boom", Some("a.rs:1:1"), Some("main"), "bt");
            if let Some((l, repeats)) = gate.offer("panic", key, line, t) {
                written.push((t, l.get("msg").cloned(), repeats));
            }
        }
        assert_eq!(written.len(), 1, "the first copy only");
        assert_eq!(written[0], (0, Some(json!("boom")), None));
        let again = gate.offer(
            "panic",
            key,
            panic_fields("boom", Some("a.rs:1:1"), Some("main"), "bt"),
            10_000,
        );
        let (_, repeats) = again.expect("a new window writes");
        assert_eq!(repeats, Some(19));
    }

    #[test]
    fn panic_fields_shape_and_stack_cap() {
        let long = "x".repeat(5000);
        let m = panic_fields("boom", Some("a.rs:1:2"), None, &long);
        assert_eq!(m.get("origin"), Some(&json!("panic")));
        assert_eq!(m.get("loc"), Some(&json!("a.rs:1:2")));
        assert_eq!(m.get("thread"), Some(&Value::Null));
        let stack = m.get("stack").and_then(Value::as_str).expect("a stack");
        assert!(stack.chars().count() <= 2000 + 20, "the stack is capped");
    }

    #[test]
    fn logs_a_shell_or_the_gpu_that_went() {
        assert_eq!(
            gone_fields("shell", "exited", Some(1), "pwsh"),
            json!({"type": "shell", "reason": "exited", "exitCode": 1, "name": "pwsh"})
                .as_object()
                .cloned()
                .expect("an object")
        );
        assert_eq!(
            gone_fields("gpu", "crashed", None, "")["exitCode"],
            Value::Null
        );
    }

    #[test]
    fn logs_unresponsive_then_responsive_with_how_long() {
        assert!(hang_fields(None).is_empty());
        assert_eq!(
            hang_fields(Some(3500)),
            json!({"ms": 3500}).as_object().cloned().expect("an object")
        );
    }
}
