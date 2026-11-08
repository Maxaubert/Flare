//! The window menu off Windows. macOS: phase 3 of the port plan (the Mac menu bar replaces the
//! Alt+Space system menu, and there is no Alt key-up to swallow), so these are no-ops.

use std::sync::atomic::{AtomicBool, Ordering};

static ALT_UP_SWALLOWED: AtomicBool = AtomicBool::new(false);

/// No system menu to show.
pub fn show_at_top_left(_hwnd: isize) {}

/// No guard to install: false.
pub fn install_alt_guard(_hwnd: isize) -> bool {
    false
}

/// Kept for the same call sites; nothing reads it until phase 3.
pub fn swallow_next_alt_up() {
    ALT_UP_SWALLOWED.store(true, Ordering::SeqCst);
}

/// Whether a swallow is pending.
pub fn alt_swallow_pending() -> bool {
    ALT_UP_SWALLOWED.load(Ordering::SeqCst)
}
