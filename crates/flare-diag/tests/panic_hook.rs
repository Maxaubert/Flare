//! M9 Task 19: the panic hook against real files. The hook is process-global, so these tests run one at
//! a time (a static lock), each one setting and taking back its own hook.

use flare_diag::crash::CrashHooks;
use flare_diag::writer::{DIAG_FILE, DiagLog, DiagLogOptions};
use serde_json::Value;
use std::fs;
use std::panic::{set_hook, take_hook};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

fn log_in(dir: &Path) -> DiagLog {
    let mut o = DiagLogOptions::new(dir.join("logs"));
    o.flush_ms = 3_600_000; // the hook writes at once; nothing waits for the batch
    DiagLog::start(o)
}

fn live(dir: &Path) -> PathBuf {
    dir.join("logs").join(DIAG_FILE)
}

fn lines(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).expect("a line is JSON"))
        .collect()
}

/// A previous hook that only raises a flag, so the test can see it ran.
fn flag_hook() -> Arc<AtomicBool> {
    let ran = Arc::new(AtomicBool::new(false));
    let r = ran.clone();
    set_hook(Box::new(move |_| r.store(true, Ordering::SeqCst)));
    ran
}

#[test]
fn logs_a_panic_by_observing_only_and_writes_it_at_once() {
    let _g = serial();
    let dir = tempfile::tempdir().expect("a temp folder");
    let previous_ran = flag_hook();
    let log = log_in(dir.path());
    let hooks = CrashHooks::install(log.clone());

    let t = std::thread::Builder::new()
        .name("crash-test".into())
        .spawn(|| panic!("kaput for the log"))
        .expect("a thread");
    assert!(t.join().is_err(), "the panic still unwinds the thread");

    // Written before the thread unwound: the file has the line with no further flush.
    let found = lines(&live(dir.path()));
    let panic_line = found
        .iter()
        .find(|l| l["k"] == "panic")
        .expect("a panic line");
    assert_eq!(panic_line["src"], "main");
    assert_eq!(panic_line["msg"], "kaput for the log");
    assert_eq!(panic_line["origin"], "panic");
    assert_eq!(panic_line["thread"], "crash-test");
    assert!(
        panic_line["loc"]
            .as_str()
            .is_some_and(|s| s.contains("panic_hook.rs"))
    );
    assert!(
        previous_ran.load(Ordering::SeqCst),
        "the previous hook still ran"
    );

    hooks.uninstall();
    let _ = take_hook();
    log.close();
}

#[test]
fn takes_its_hook_off_again() {
    let _g = serial();
    let dir = tempfile::tempdir().expect("a temp folder");
    let previous_ran = flag_hook();
    let log = log_in(dir.path());
    let hooks = CrashHooks::install(log.clone());
    hooks.uninstall();
    previous_ran.store(false, Ordering::SeqCst);

    let t = std::thread::spawn(|| panic!("after the hook was taken off"));
    assert!(t.join().is_err());

    assert!(previous_ran.load(Ordering::SeqCst), "the original hook ran");
    log.flush_sync();
    let kinds: Vec<Value> = lines(&live(dir.path()))
        .iter()
        .map(|l| l["k"].clone())
        .collect();
    assert!(
        !kinds.iter().any(|k| k == "panic"),
        "no panic line after uninstall: {kinds:?}"
    );
    let _ = take_hook();
    log.close();
}

/// A broken log (its queue lock poisoned by an earlier panic) must not stop the hook: the line still
/// lands, the previous hook runs, and the process does not abort. A panic raised INSIDE the hook would
/// abort the process (std), so the writer never panics there; it recovers poisoned locks instead.
#[test]
fn a_broken_log_does_not_turn_the_panic_into_an_abort() {
    let _g = serial();
    let dir = tempfile::tempdir().expect("a temp folder");
    let previous_ran = flag_hook();
    let log = log_in(dir.path());
    log.poison_queue_for_test();
    let hooks = CrashHooks::install(log.clone());

    let t = std::thread::spawn(|| panic!("the first panic"));
    assert!(
        t.join().is_err(),
        "the thread's own panic is what the join sees"
    );
    assert!(previous_ran.load(Ordering::SeqCst), "the previous hook ran");
    assert!(
        lines(&live(dir.path())).iter().any(|l| l["k"] == "panic"),
        "the panic line still lands"
    );

    hooks.uninstall();
    let _ = take_hook();
    log.close();
}
