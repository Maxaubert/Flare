//! The window menu on macOS (port phase 3): not applicable. The Alt+Space system menu (move, size,
//! minimise, maximise, close) has no Mac counterpart on the window: those commands live in the
//! Mac menu bar's Window menu (minimise, zoom, full screen) and on the traffic lights, and the
//! window is moved and sized by AppKit itself. There is no Alt key-up that opens a menu either
//! (Option never does), so there is nothing to swallow. These stay no-ops for the shared call
//! sites.

use std::sync::atomic::{AtomicBool, Ordering};

static ALT_UP_SWALLOWED: AtomicBool = AtomicBool::new(false);

/// No system menu to show.
pub fn show_at_top_left(_hwnd: isize) {}

/// No guard to install: false.
pub fn install_alt_guard(_hwnd: isize) -> bool {
    false
}

/// Kept for the same call sites; nothing on macOS reads it (no Alt menu to keep shut).
pub fn swallow_next_alt_up() {
    ALT_UP_SWALLOWED.store(true, Ordering::SeqCst);
}

/// Whether a swallow is pending.
pub fn alt_swallow_pending() -> bool {
    ALT_UP_SWALLOWED.load(Ordering::SeqCst)
}
