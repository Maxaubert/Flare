//! The UI timers the app owns, by name, for the watchdog and the e2e probes (filled by the task that needs it).
//!
//! A named timer is one `slint::Timer` per name on the UI thread: setting a name again replaces the timer
//! (so a caller never holds two), clearing it stops it, and `names()` lists the ones still running. The
//! dictation controller (M8 Task 22) keeps its key tick, the partial tick, the warm-up, the picker's thirty
//! seconds and the widget's hide here.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;

thread_local! {
    static TIMERS: RefCell<HashMap<&'static str, slint::Timer>> = RefCell::new(HashMap::new());
}

/// Starts (or restarts) the timer `name`: once after `ms`, or every `ms` when `repeat`.
pub fn set(name: &'static str, ms: u64, repeat: bool, f: impl FnMut() + 'static) {
    let timer = slint::Timer::default();
    let mode = if repeat {
        slint::TimerMode::Repeated
    } else {
        slint::TimerMode::SingleShot
    };
    timer.start(mode, Duration::from_millis(ms), f);
    // The old timer (if any) stops when it is dropped here.
    TIMERS.with(|t| t.borrow_mut().insert(name, timer));
}

/// Stops and forgets the timer `name`.
pub fn clear(name: &'static str) {
    TIMERS.with(|t| t.borrow_mut().remove(name));
}

/// Whether the timer `name` is running.
#[allow(dead_code)] // the e2e probes read it (Task 27)
pub fn running(name: &str) -> bool {
    TIMERS.with(|t| t.borrow().get(name).is_some_and(slint::Timer::running))
}

/// The names of the timers running now, sorted.
#[allow(dead_code)] // the e2e probes read it (Task 27)
pub fn names() -> Vec<&'static str> {
    TIMERS.with(|t| {
        let mut v: Vec<&'static str> = t
            .borrow()
            .iter()
            .filter(|(_, timer)| timer.running())
            .map(|(name, _)| *name)
            .collect();
        v.sort_unstable();
        v
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_holds_one_timer_and_clear_forgets_it() {
        crate::ui_harness::ensure_platform();
        assert!(names().is_empty());
        set("dictation.test-a", 60_000, false, || {});
        set("dictation.test-a", 60_000, true, || {});
        set("dictation.test-b", 60_000, false, || {});
        assert_eq!(names(), ["dictation.test-a", "dictation.test-b"]);
        assert!(running("dictation.test-a"));
        clear("dictation.test-a");
        assert!(!running("dictation.test-a"));
        assert_eq!(names(), ["dictation.test-b"]);
        clear("dictation.test-b");
        assert!(names().is_empty());
    }
}
