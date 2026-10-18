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
}
