//! The one place Flare asks macOS for Input Monitoring (rule 1137). The hook itself never asks (rule 1077): it
//! reads the permission with `CGPreflightListenEventAccess` and waits. A modifier binding (the default Ctrl+Cmd)
//! cannot work from another app without the permission, and a missing permission was silence, so the app asks
//! on a person's own act only: switching dictation (or "Dictate from any app") on, or pressing the dictation key
//! in Flare's window. macOS shows its "receive keystrokes" dialog once (it adds Flare to System Settings >
//! Privacy & Security > Input Monitoring, switched off); every later call returns at once and shows nothing.

use std::sync::atomic::{AtomicBool, Ordering};

/// Asked once per run: the system shows its dialog only the first time anyway.
static ASKED: AtomicBool = AtomicBool::new(false);

/// Asks for Input Monitoring once per run, on a worker (the call may wait on the system). True when it asked now.
pub fn ask_once() -> bool {
    if ASKED.swap(true, Ordering::SeqCst) {
        return false;
    }
    let spawned = std::thread::Builder::new()
        .name("flare-dictation-ask".into())
        .spawn(|| {
            // SAFETY: a plain request with no arguments; the system owns the dialog.
            unsafe { CGRequestListenEventAccess() };
        });
    spawned.is_ok()
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGRequestListenEventAccess() -> bool;
}
