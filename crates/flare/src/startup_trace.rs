//! Start-up phase marks: when each named point of the start-up path was reached, counted from
//! the process creation time (so the loader's time before `main` is included).

use std::time::Duration;

/// One named point of the start-up path, in the order the app reaches them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Phase {
    Main,
    Args,
    Backend,
    Component,
    Profile,
    ShellSpawn,
    RenderSetup,
    FirstFrame,
    FirstPrompt,
    Usable,
}

const PHASES: usize = 10;

impl Phase {
    /// Every phase, in the order the app reaches them (the index of a phase is its position here).
    pub const ALL: [Phase; PHASES] = [
        Phase::Main,
        Phase::Args,
        Phase::Backend,
        Phase::Component,
        Phase::Profile,
        Phase::ShellSpawn,
        Phase::RenderSetup,
        Phase::FirstFrame,
        Phase::FirstPrompt,
        Phase::Usable,
    ];

    /// The name the log line and the phase table use.
    pub fn name(self) -> &'static str {
        match self {
            Phase::Main => "main",
            Phase::Args => "args",
            Phase::Backend => "backend",
            Phase::Component => "component",
            Phase::Profile => "profile",
            Phase::ShellSpawn => "shell_spawn",
            Phase::RenderSetup => "render_setup",
            Phase::FirstFrame => "first_frame",
            Phase::FirstPrompt => "first_prompt",
            Phase::Usable => "usable",
        }
    }
}

/// One slot per phase, holding nanoseconds since the process started (0 = not reached yet).
struct Marks([std::sync::atomic::AtomicU64; PHASES]);

impl Marks {
    const fn new() -> Self {
        Self([const { std::sync::atomic::AtomicU64::new(0) }; PHASES])
    }

    /// Records `ns` for `phase` unless it already has a mark (the first mark wins).
    fn set(&self, phase: Phase, ns: u64) {
        let _ = self.0[phase as usize].compare_exchange(
            0,
            ns.max(1),
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
        );
    }

    fn get(&self, phase: Phase) -> Option<u64> {
        match self.0[phase as usize].load(std::sync::atomic::Ordering::Acquire) {
            0 => None,
            ns => Some(ns),
        }
    }
}

static MARKS: Marks = Marks::new();

/// Records that `phase` was reached now. The first call per phase wins; later calls are ignored.
/// A phase already marked costs one atomic load, no clock read.
pub fn mark(phase: Phase) {
    if MARKS.get(phase).is_some() {
        return;
    }
    let ns = u64::try_from(since_process_start().as_nanos()).unwrap_or(u64::MAX);
    MARKS.set(phase, ns);
    if phase == Phase::FirstFrame {
        release_deferred();
    }
}

/// Whether `phase` has been reached.
pub fn is_marked(phase: Phase) -> bool {
    MARKS.get(phase).is_some()
}

/// Time since the process was created (the kernel's creation time against now), so it counts
/// the loader's time before `main`. Zero if the kernel refuses the query.
pub fn since_process_start() -> Duration {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    fn ticks(ft: &FILETIME) -> u64 {
        (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)
    }
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut creation = zero;
    let (mut exit, mut kernel, mut user) = (zero, zero, zero);
    // SAFETY: the pseudo-handle is always valid; every out pointer is a live local.
    let ok = unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    };
    if ok == 0 {
        return Duration::ZERO;
    }
    let mut now = zero;
    // SAFETY: `now` is a live local the call fills in.
    unsafe { GetSystemTimePreciseAsFileTime(&mut now) };
    between_filetimes(ticks(&creation), ticks(&now))
}

/// The marked phases in phase order, with the time each was reached.
#[allow(dead_code)] // the hook's `startup.trace` probe reads it (Task 11)
pub fn snapshot() -> Vec<(Phase, Duration)> {
    snapshot_of(&MARKS)
}

/// One log line: `startup: main 12 ms, args 13 ms, ...`.
pub fn line() -> String {
    line_of(&MARKS)
}

fn snapshot_of(marks: &Marks) -> Vec<(Phase, Duration)> {
    Phase::ALL
        .iter()
        .filter_map(|&p| marks.get(p).map(|ns| (p, Duration::from_nanos(ns))))
        .collect()
}

fn line_of(marks: &Marks) -> String {
    let parts: Vec<String> = snapshot_of(marks)
        .iter()
        .map(|(p, d)| format!("{} {} ms", p.name(), d.as_millis()))
        .collect();
    if parts.is_empty() {
        "startup: no phases marked".to_string()
    } else {
        format!("startup: {}", parts.join(", "))
    }
}

/// The elapsed time between two Windows FILETIMEs (100 ns units), zero when `now` is earlier.
fn between_filetimes(creation: u64, now: u64) -> Duration {
    Duration::from_nanos(now.saturating_sub(creation).saturating_mul(100))
}

// ---- M10 Task 63: start-up work the first frame does not need -------------------------------

/// One deferred item: its name (for the log) and the work.
pub type Deferred = (&'static str, Box<dyn FnOnce() + Send + 'static>);

thread_local! {
    /// Queued on the UI thread before the event loop starts (every caller is there), so no lock is needed.
    static DEFERRED: std::cell::RefCell<Vec<Deferred>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Queues `f` to run on a worker thread once the first frame is marked. Called on the UI thread
/// before the event loop starts. Once the first frame is marked, the work starts at once.
pub fn after_first_frame(name: &'static str, f: impl FnOnce() + Send + 'static) {
    if is_marked(Phase::FirstFrame) {
        start_worker(vec![(name, Box::new(f))]);
        return;
    }
    DEFERRED.with(|q| q.borrow_mut().push((name, Box::new(f))));
}

/// Takes everything queued on this thread, in the order it was queued.
pub fn take_deferred() -> Vec<Deferred> {
    DEFERRED.with(|q| std::mem::take(&mut *q.borrow_mut()))
}

/// Runs each item in order. An item that panics is logged; the rest still run.
pub fn run_deferred(items: Vec<Deferred>) {
    for (name, work) in items {
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).is_err() {
            log::warn!("deferred start-up work {name} panicked");
        }
    }
}

/// Starts one worker that runs `items`. A worker that cannot start logs it and the items are dropped.
fn start_worker(items: Vec<Deferred>) {
    let spawned = std::thread::Builder::new()
        .name("flare-deferred".into())
        .spawn(move || run_deferred(items));
    if let Err(e) = spawned {
        log::warn!("could not start the deferred start-up work: {e}");
    }
}

/// Called once when the first frame is marked: the queued items start on a worker.
fn release_deferred() {
    let items = take_deferred();
    if !items.is_empty() {
        start_worker(items);
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_mark_wins_and_later_marks_are_ignored() {
        let marks = Marks::new();
        marks.set(Phase::Backend, 410_000_000);
        marks.set(Phase::Backend, 999_000_000);
        assert_eq!(marks.get(Phase::Backend), Some(410_000_000));
        assert_eq!(marks.get(Phase::Main), None);
    }

    #[test]
    fn snapshot_is_in_phase_order_and_skips_unmarked() {
        let marks = Marks::new();
        marks.set(Phase::Usable, 1_500_000_000);
        marks.set(Phase::Main, 12_000_000);
        marks.set(Phase::Args, 13_000_000);
        let got = snapshot_of(&marks);
        assert_eq!(
            got,
            vec![
                (Phase::Main, Duration::from_millis(12)),
                (Phase::Args, Duration::from_millis(13)),
                (Phase::Usable, Duration::from_millis(1500)),
            ]
        );
    }

    #[test]
    fn since_process_start_includes_time_before_main() {
        // A process created 50 ms before "now" counts those 50 ms, not only the time after main.
        let now: u64 = 133_000_000_000_000_000;
        let creation = now - 500_000;
        assert_eq!(between_filetimes(creation, now), Duration::from_millis(50));
        assert_eq!(between_filetimes(now, creation), Duration::ZERO);
        // The real clock is never negative and is a sane process age.
        let age = since_process_start();
        assert!(age < Duration::from_secs(60 * 60 * 24), "age {age:?}");
    }

    #[test]
    fn line_lists_every_marked_phase_with_milliseconds() {
        let marks = Marks::new();
        marks.set(Phase::Main, 12_000_000);
        marks.set(Phase::Args, 13_500_000);
        assert_eq!(line_of(&marks), "startup: main 12 ms, args 13 ms");
        assert_eq!(line_of(&Marks::new()), "startup: no phases marked");
    }

    #[test]
    fn marks_are_lock_free() {
        let src = include_str!("startup_trace.rs");
        let needle = ["Mu", "tex"].concat();
        assert!(
            !src.contains(&needle),
            "startup_trace.rs must stay lock-free"
        );
    }

    #[test]
    fn every_phase_has_a_call_site() {
        let sources = [
            ("main.rs", include_str!("main.rs")),
            ("window.rs", include_str!("window.rs")),
            ("appctl.rs", include_str!("appctl.rs")),
        ];
        for phase in Phase::ALL {
            let needle = format!("Phase::{phase:?}");
            let found = sources.iter().any(|(_, s)| s.contains(&needle));
            assert!(found, "no call site marks {needle}");
        }
    }

    fn item(name: &'static str, f: impl FnOnce() + Send + 'static) -> Deferred {
        (name, Box::new(f))
    }

    #[test]
    fn deferred_work_runs_exactly_once_after_the_first_frame() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let ran = Arc::new(AtomicUsize::new(0));
        for name in ["first", "second"] {
            let ran = Arc::clone(&ran);
            DEFERRED.with(|q| {
                q.borrow_mut().push(item(name, move || {
                    ran.fetch_add(1, Ordering::SeqCst);
                }))
            });
        }
        // Nothing runs while the work is only queued (before the first frame).
        assert_eq!(ran.load(Ordering::SeqCst), 0);
        let items = take_deferred();
        assert_eq!(
            items.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
            vec!["first", "second"],
            "queued in order"
        );
        assert!(take_deferred().is_empty(), "taken once, never twice");
        run_deferred(items);
        assert_eq!(ran.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_deferred_item_that_panics_does_not_kill_the_rest() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        let ran = Arc::new(AtomicBool::new(false));
        let after = Arc::clone(&ran);
        run_deferred(vec![
            item("boom", || panic!("deliberate test panic")),
            item("after", move || after.store(true, Ordering::SeqCst)),
        ]);
        assert!(
            ran.load(Ordering::SeqCst),
            "the item after a panic still runs"
        );
    }

    #[test]
    fn the_help_index_is_not_built_at_start() {
        // The catalogue and its search index are built when the panel first opens (and dropped a
        // minute after it closes), not by a warm-up at start: about 2 MB of idle memory.
        let src = include_str!("window.rs");
        assert!(!src.contains("help_warm_work"));
        assert!(!src.contains("after_first_frame(\"help_index\""));
    }

    #[test]
    fn nothing_on_the_deferral_list_is_read_by_the_first_frame() {
        // The first frame is drawn by `render_now` and the rendering notifier. No deferred work may be
        // registered inside either, and the deferred names are registered before the event loop starts.
        let src = include_str!("window.rs");
        let block = |from: &str, to: &str| {
            let start = src
                .find(from)
                .unwrap_or_else(|| panic!("window.rs has {from}"));
            let end = src[start..]
                .find(to)
                .map(|i| start + i)
                .unwrap_or_else(|| panic!("window.rs has {to} after {from}"));
            &src[start..end]
        };
        let render = block("let render_now = {", "let notifier_result = {");
        let notifier = block("let notifier_result = {", "if let Err(e) = notifier_result");
        for scope in [render, notifier] {
            assert!(
                !scope.contains("after_first_frame("),
                "the first frame reads no deferred work"
            );
        }
        let run_loop = src
            .find("let result = app.run();")
            .expect("the event loop starts");
        for name in ["verb_reconcile"] {
            let call = src
                .find(&format!("after_first_frame(\"{name}\""))
                .unwrap_or_else(|| panic!("window.rs defers {name}"));
            assert!(
                call < run_loop,
                "{name} is queued before the event loop starts"
            );
        }
    }
}
