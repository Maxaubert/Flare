//! The test seams that live below the app crate (the dictation models folder, the engine folder, the
//! NVIDIA probe answer, the plugin template folder). Audit 2026-10-20 (L13, L25, L31, L40, L42, L44, L52):
//! a release build read them from plain env, so a stray variable in the owner's environment could change
//! the real app. Now they read as absent unless the gate is open, and the gate opens only for `--e2e`
//! (the app's `e2e::init`) or an in-process scene runner that says so.
//!
//! This file holds the ONLY `std::env` read of a seam name; `crates/flare/tests/names.rs` fails on any other.

use std::ffi::OsString;
use std::sync::atomic::{AtomicBool, Ordering};

static OPEN: AtomicBool = AtomicBool::new(false);

/// Opens or closes the gate (main, once, from the `--e2e` flag; a scene runner for its fixed answers).
pub fn set_open(on: bool) {
    OPEN.store(on, Ordering::Relaxed);
}

/// Whether the gate is open.
pub fn is_open() -> bool {
    OPEN.load(Ordering::Relaxed)
}

/// The seam `name` given a gate state: None unless `open`.
pub fn read_if(open: bool, name: &str) -> Option<OsString> {
    if !open {
        return None;
    }
    std::env::var_os(name)
}

/// The seam variable `name`, None unless the gate is open.
pub fn var_os(name: &str) -> Option<OsString> {
    read_if(is_open(), name)
}

/// The seam variable `name` as text, None unless the gate is open (or not valid text).
pub fn var(name: &str) -> Option<String> {
    var_os(name).and_then(|v| v.into_string().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_gate_reads_every_seam_as_absent() {
        let name = ["FLARE_SEAM", "_GATE_TEST"].concat();
        // SAFETY: a variable only this test reads; no other thread touches it.
        unsafe { std::env::set_var(&name, "yes") };
        assert_eq!(read_if(false, &name), None);
        assert_eq!(read_if(true, &name), Some(OsString::from("yes")));
    }
}
